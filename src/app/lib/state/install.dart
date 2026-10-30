import 'dart:async';

import 'package:flutter/foundation.dart';

import '../platform/install_ports.dart';
import '../src/rust/api/install.dart';
import '../src/rust/api/setup.dart';
import '../src/rust/api/store.dart';
import 'app_state.dart';

/// Which screen an install shows. The steps themselves are Rust's: this
/// follows what the run reports and carries the owner's answers back.
enum InstallStage {
  /// Reading the APK's image and this build's identity.
  loading,

  /// What it takes, and how the feeder finds this phone.
  before,

  /// How feeders list this phone, when it has no name yet.
  naming,

  /// The Wi-Fi Bluetooth setup gives the replacement.
  wifi,

  /// The DNS record, its check and the wait for the feeder.
  record,

  /// A feeder connected: is it the owner's?
  confirm,

  /// Stock downloads the replacement from this phone.
  sending,

  /// This phone becomes the replacement's owner.
  claim,

  /// The backup, the promotion and the confirmation.
  finishing,
  done,
  stopped,
}

/// Where the checklist is.
enum CheckState { waiting, running, done }

/// What the listener after an uncertain request has heard.
enum Listening {
  /// Not started: the stop is not an uncertain one.
  idle,
  running,

  /// Stock came back: it did not install. A new run may ask again only once
  /// the request is closed.
  stockStayed,

  /// Nothing, for the whole wait.
  silent,

  /// The listener could not run.
  failed,
}

/// One install, from the first screen to its end.
class InstallFlow extends ChangeNotifier {
  InstallFlow(this._book, this._ports);

  /// What this phone calls the feeder until the owner names it.
  static const defaultFeederName = 'Feeder';

  final FeederBook _book;
  final InstallPorts _ports;

  InstallStage stage = InstallStage.loading;

  ImageCheck? image;
  BuildCheck? build;

  /// An earlier run left an install to pick up from bank 0.
  bool pending = false;
  InstallPaths? _paths;

  // The record.
  String? phoneIp;
  String brokerName = '';
  DnsResult? dns;
  bool checking = false;

  WaitReason? waitReason;

  /// Stock connected but its reports were lost: restart it once more.
  bool restartAgain = false;

  // The feeder check.
  String? device;
  String? deviceAddress;
  int? _recognizeAsk;

  // Sending.
  int percent = 0;
  bool restarting = false;
  DateTime? _sendingSince;

  // The claim.
  /// A bench image's recovery window, waiting for the owner.
  int? _windowAsk;
  bool get windowAsked => _windowAsk != null;
  List<int> code = const [];
  DateTime? deadline;
  bool pressDispense = false;

  // The checklist.
  /// The feeder restarting onto the owner's Wi-Fi, after Bluetooth setup.
  CheckState joining = CheckState.waiting;
  CheckState copying = CheckState.waiting;
  double copied = 0;
  CheckState checked = CheckState.waiting;
  CheckState promoting = CheckState.waiting;
  double written = 0;
  CheckState confirming = CheckState.waiting;
  bool sessionDropped = false;

  /// A checked copy of stock.
  String? backupPath;

  // The end.
  String? _identity;
  String? _address;
  PlansOutcome? plans;
  int plansConverted = 0;
  InstallEnd_Stopped? stop;

  // After an uncertain request.
  Listening listening = Listening.idle;
  Duration listened = Duration.zero;
  bool stockSeen = false;
  String? listenerDetail;

  WifiCredentials? _home;
  bool _calibrate = false;
  bool _withoutPlans = false;
  bool _forget = false;

  InstallRun? _run;
  StreamSubscription<InstallUpdate>? _updates;
  InstallLink? _link;
  InstallListener? _listener;
  StreamSubscription<ListenUpdate>? _listenUpdates;
  StreamSubscription<void>? _heard;
  bool _holding = false;
  bool _disposed = false;

  /// How long a listener listens, for "3 of 20 min".
  Duration get listenWait => _ports.listenWait;

  /// How the APK's image pairs, once it is known to be ready.
  ImageProvisioning? get provisioning => switch (image) {
    ImageCheck_Ready(:final provisioning) => provisioning,
    _ => null,
  };

  /// A run is going: leaving means stopping it.
  bool get running => _run != null;

  /// Why this install cannot start, in the order the owner can act on.
  bool get mayStart =>
      image is ImageCheck_Ready && build is BuildCheck_Reviewable;

  /// A rough time left in stock's download, from how fast it has gone.
  Duration? get timeLeft {
    final since = _sendingSince;
    if (since == null || percent < 5 || percent >= 100) return null;
    final elapsed = DateTime.now().difference(since);
    return elapsed * ((100 - percent) / percent);
  }

  /// Read what decides whether this phone can install at all.
  Future<void> load() async {
    final paths = await _ports.paths();
    _paths = paths;
    image = await _ports.prepareImage(paths);
    build = _ports.checkBuild();
    pending = _ports.pending(paths);
    phoneIp = await _ports.phoneAddress();
    _go(InstallStage.before);
  }

  /// On from the first screen: the phone's name if it has none, then the
  /// Wi-Fi a consumer image needs, then the record.
  Future<bool> begin() async {
    final bluetooth = provisioning == ImageProvisioning.consumer;
    if (!await _ports.askPermissions(bluetooth: bluetooth)) return false;
    phoneIp = await _ports.phoneAddress();
    _nextAfter(InstallStage.before);
    return true;
  }

  Future<void> setLabel(String label) async {
    await _book.setLabel(label);
    _nextAfter(InstallStage.naming);
  }

  void setHome(WifiCredentials? home) {
    _home = home;
    _nextAfter(InstallStage.wifi);
  }

  void back(InstallStage to) => _go(to);

  void _nextAfter(InstallStage from) {
    final consumer = provisioning == ImageProvisioning.consumer;
    final next = switch (from) {
      InstallStage.before when _book.label.isEmpty => InstallStage.naming,
      InstallStage.before ||
      InstallStage.naming => consumer ? InstallStage.wifi : InstallStage.record,
      _ => InstallStage.record,
    };
    _go(next);
  }

  /// Look the record up from this phone. When it points here the run starts
  /// at once; otherwise the owner fixes it, or goes on anyway: this phone may
  /// ask another resolver than the feeder does, so stock arriving at the
  /// broker is the real check.
  Future<void> checkRecord(String name) async {
    final ip = phoneIp;
    if (ip == null || checking) return;
    brokerName = name.trim();
    checking = true;
    _notify();
    final result = await _ports.checkDns(brokerName, ip);
    checking = false;
    dns = result;
    _notify();
    if (result is DnsResult_Points) await startRun();
  }

  /// Start the run from stock.
  Future<void> startRun() async {
    final ip = phoneIp;
    if (ip == null || brokerName.isEmpty) return;
    await _start(InstallStart.dns(name: brokerName), ip);
  }

  /// Pick up from bank 0: the replacement runs, and only its owner is
  /// missing, or the backup and the promotion.
  Future<void> resume() async {
    final ip = phoneIp ?? await _ports.phoneAddress() ?? '';
    stop = null;
    listening = Listening.idle;
    _stopListening();
    _go(InstallStage.claim);
    await _start(const InstallStart.bank0(), ip);
  }

  /// The stop's way on: from the start, or from bank 0, as the run said.
  Future<void> again({
    bool calibrate = false,
    bool withoutPlans = false,
    bool forget = false,
  }) async {
    _calibrate = calibrate;
    _withoutPlans = withoutPlans;
    _forget = forget;
    final resumeFrom = stop?.resume;
    if (resumeFrom == Resume.fromBank0) {
      await resume();
      return;
    }
    stop = null;
    listening = Listening.idle;
    waitReason = null;
    restartAgain = false;
    dns = null;
    _go(InstallStage.record);
  }

  Future<void> _start(InstallStart start, String ip) async {
    final paths = _paths;
    if (paths == null) return;
    final run = _ports.newRun(
      InstallSpec(
        start: start,
        phoneIp: ip,
        home: _home,
        label: _book.label,
        feederName: defaultFeederName,
        knownFeeders: [for (final feeder in _book.feeders) feeder.identity],
        storePath: paths.store,
        imageDir: paths.image,
        dataDir: paths.data,
        calibrateClock: _calibrate,
        withoutStockPlans: _withoutPlans,
        forgetEnrollment: _forget,
      ),
    );
    _run = run;
    await _hold('Keep the feeder plugged in.');
    _updates = run.run().listen(
      (update) => unawaited(_update(run, update)),
      onError: (Object error) {
        _ended(
          InstallEnd.stopped(
            case_: const StopCase.other(),
            resume: Resume.nothing,
            detail: '$error',
          ),
        );
      },
    );
    _notify();
  }

  Future<void> _hold(String text) async {
    _holding = true;
    await _ports.holdProcess(text);
  }

  Future<void> _release() async {
    if (!_holding) return;
    _holding = false;
    await _ports.releaseProcess();
  }

  Future<void> _update(InstallRun run, InstallUpdate update) async {
    if (_disposed) return;
    switch (update) {
      case InstallUpdate_Event(:final event):
        await _event(event);
      case InstallUpdate_Ask(:final id, :final ask):
        await _ask(run, id, ask);
      case InstallUpdate_Ended(:final end):
        _ended(end);
    }
    _notify();
  }

  Future<void> _ask(InstallRun run, int id, InstallAsk ask) async {
    void answer(InstallAnswer answer) => run.answer(id: id, answer: answer);

    switch (ask) {
      // Asked here only once the owner looked at the record.
      case InstallAsk_SetOverride():
        answer(const InstallAnswer.done());
      // The screen says to restart it; the broker is already listening.
      case InstallAsk_RestartFeeder():
        answer(const InstallAnswer.done());
      case InstallAsk_RecognizeFeeder(:final device, :final address):
        this.device = device;
        deviceAddress = address;
        _recognizeAsk = id;
        stage = InstallStage.confirm;
      case InstallAsk_OpenRecoveryWindow():
        _windowAsk = id;
        stage = InstallStage.claim;
      case InstallAsk_OpenLink():
        stage = InstallStage.claim;
        try {
          _link = await _ports.openLink(
            onBytes: (bytes) => run.notified(bytes: bytes),
            onLost: run.linkLost,
          );
          answer(const InstallAnswer.done());
        } catch (error) {
          answer(InstallAnswer.failed(reason: '$error'));
        }
      case InstallAsk_WriteLink(:final pieces):
        try {
          await _link?.write(pieces);
          answer(const InstallAnswer.done());
        } catch (error) {
          answer(InstallAnswer.failed(reason: '$error'));
        }
      case InstallAsk_CloseLink():
        try {
          await _link?.close();
        } catch (_) {
          // A link the feeder dropped has nothing left to close.
        }
        _link = null;
        answer(const InstallAnswer.done());
      case InstallAsk_Locate(:final identity):
        final address = await _ports.locate(identity);
        answer(InstallAnswer.located(address: address));
    }
  }

  Future<void> _event(InstallEvent event) async {
    switch (event) {
      case InstallEvent_DnsChecked(:final result):
        dns = result;
      case InstallEvent_Waiting(:final why):
        waitReason = why;
      case InstallEvent_VersionLost():
        restartAgain = true;
      case InstallEvent_Requesting():
        stage = InstallStage.sending;
        _sendingSince = DateTime.now();
        await _hold('This phone is serving the firmware.');
      case InstallEvent_StockProgress(:final percent):
        stage = InstallStage.sending;
        this.percent = percent;
      case InstallEvent_StockRestarting():
        stage = InstallStage.sending;
        restarting = true;
      case InstallEvent_SetupStarting():
        stage = InstallStage.claim;
        await _hold('The feeder is waiting for its owner.');
      case InstallEvent_Code(:final code, :final seconds):
        stage = InstallStage.claim;
        this.code = code;
        deadline = DateTime.now().add(Duration(seconds: seconds));
        _windowAsk = null;
      case InstallEvent_PressDispense():
        pressDispense = true;
      case InstallEvent_KeyKnown():
        stage = InstallStage.finishing;
      case InstallEvent_Paired(
        :final identity,
        :final address,
        :final rebooting,
      ):
        stage = InstallStage.finishing;
        _identity = identity
            .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
            .join();
        _address = address ?? _address;
        joining = rebooting ? CheckState.running : CheckState.waiting;
      case InstallEvent_Joined(:final address):
        stage = InstallStage.finishing;
        _address = address;
        joining = CheckState.done;
      case InstallEvent_ReplacementAnswers():
        stage = InstallStage.finishing;
        pressDispense = false;
      case InstallEvent_ReadingBackup():
        copying = CheckState.running;
        await _hold('Copying the original firmware.');
      case InstallEvent_BackupRead(:final bytes, :final total):
        copying = CheckState.running;
        copied = total == 0 ? 0 : bytes.toDouble() / total;
      case InstallEvent_BackupKept(:final path):
        backupPath = path;
        copying = CheckState.done;
        checked = CheckState.done;
      case InstallEvent_StockPlans(:final converted):
        plansConverted = converted;
      case InstallEvent_Promoting():
        copying = CheckState.done;
        checked = CheckState.done;
        promoting = CheckState.running;
        await _hold('Making Wellfed permanent.');
      case InstallEvent_Written(written: final done, :final length):
        written = length == 0 ? 0 : done / length;
      case InstallEvent_Activated():
        copying = CheckState.done;
        checked = CheckState.done;
        promoting = CheckState.done;
        confirming = CheckState.running;
      case InstallEvent_Installed():
        confirming = CheckState.done;
      case InstallEvent_Plans(:final result):
        plans = result;
      case InstallEvent_SessionDropped():
        sessionDropped = true;
      case InstallEvent_StockServerStarted() ||
          InstallEvent_AwaitingStock() ||
          InstallEvent_StockServerEnded() ||
          InstallEvent_NoSetupFeeder() ||
          InstallEvent_WithoutStockPlans() ||
          InstallEvent_PuttingPlans():
        break;
      case InstallEvent_Note(:final text):
        debugPrint('install: $text');
    }
  }

  /// The feeder enrolled this phone, and Rust saved it in the app's file
  /// before going on. Home hears of it only here, as the run ends: a session
  /// of its own would only get in the install's way.
  Future<void> _settle() async {
    final identity = _identity;
    if (identity == null) return;
    await _book.remember(
      KnownFeeder(
        identity: identity,
        name: defaultFeederName,
        address: _address,
      ),
      paired: true,
    );
  }

  void _ended(InstallEnd end) {
    _run = null;
    unawaited(_updates?.cancel());
    _updates = null;
    unawaited(_settle());
    switch (end) {
      case InstallEnd_Done():
        stage = InstallStage.done;
        unawaited(_release());
      case final InstallEnd_Stopped stopped:
        stop = stopped;
        stage = InstallStage.stopped;
        _windowAsk = null;
        _recognizeAsk = null;
        if (stopped.case_ is StopCase_Uncertain) {
          unawaited(listen());
        } else {
          unawaited(_release());
        }
    }
    _notify();
  }

  /// The owner's word on the feeder that connected.
  void recognize({required bool yes}) {
    final id = _recognizeAsk;
    if (id == null) return;
    _recognizeAsk = null;
    _run?.answer(
      id: id,
      answer: InstallAnswer.yes(yes: yes),
    );
    if (yes) {
      stage = InstallStage.sending;
      _sendingSince = DateTime.now();
    }
    _notify();
  }

  /// A bench image's recovery window is open.
  void windowOpened() {
    final id = _windowAsk;
    if (id == null) return;
    _windowAsk = null;
    _run?.answer(id: id, answer: const InstallAnswer.done());
    _notify();
  }

  /// Listen for what became of an uncertain request. Sends the feeder
  /// nothing: only what it does next can say.
  Future<void> listen() async {
    final paths = _paths;
    if (paths == null) return;
    _stopListening();
    listening = Listening.running;
    listened = Duration.zero;
    stockSeen = false;
    await _hold('Listening for the feeder. Leave it plugged in.');
    final listener = _ports.newListener(paths);
    _listener = listener;
    _listenUpdates = listener.run().listen(
      _heardFromListener,
      onError: (Object error) {
        listenerDetail = '$error';
        _listenEnded(Listening.failed);
      },
    );
    // A consumer image has no network until setup gives it one, so it is
    // only heard advertising setup. Hearing it means it installed.
    _heard = _ports.setupHeard().listen(
      (_) => unawaited(resume()),
      onError: (Object _) {},
    );
    _notify();
  }

  void _heardFromListener(ListenUpdate update) {
    switch (update) {
      case ListenUpdate_Tick(:final seconds, :final stock):
        listened = Duration(seconds: seconds);
        stockSeen = stock;
        _notify();
      case ListenUpdate_Replacement():
        unawaited(resume());
      case ListenUpdate_StockStayed(:final resolved):
        stop = InstallEnd_Stopped(
          case_: const StopCase.uncertain(),
          resume: resolved ? Resume.fromStart : Resume.nothing,
          detail: stop?.detail ?? '',
        );
        _listenEnded(Listening.stockStayed);
      case ListenUpdate_Silent():
        _listenEnded(Listening.silent);
      case ListenUpdate_Failed(:final detail):
        listenerDetail = detail;
        _listenEnded(Listening.failed);
      case ListenUpdate_Stopped():
        break;
    }
  }

  void _listenEnded(Listening end) {
    listening = end;
    unawaited(_heard?.cancel());
    _heard = null;
    unawaited(_release());
    _notify();
  }

  void _stopListening() {
    _listener?.stop();
    _listener = null;
    unawaited(_listenUpdates?.cancel());
    _listenUpdates = null;
    unawaited(_heard?.cancel());
    _heard = null;
  }

  /// End the run through its cleanup; the end says what it left.
  void stopRun() => _run?.stop();

  /// The feeder's name on this phone, and where it last answered.
  Future<void> finish(String name) async {
    final identity = _identity;
    if (identity == null) return;
    await _book.remember(
      KnownFeeder(identity: identity, name: name, address: _address),
    );
  }

  Future<void> saveBackup() async {
    final path = backupPath;
    if (path != null) await _ports.shareBackup(path);
  }

  void _go(InstallStage next) {
    stage = next;
    _notify();
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _run?.stop();
    unawaited(_updates?.cancel());
    _stopListening();
    unawaited(_link?.close());
    unawaited(_release());
    super.dispose();
  }
}
