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

  /// The reset route: hold Reset, which erases the feeder's settings.
  reset,

  /// The reset route: join the feeder's network, send the Wi-Fi, come back.
  handover,

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

/// Where the handover is.
enum Handover {
  /// The broker starts; nothing to press yet.
  starting,

  /// Waiting for the owner to join the feeder's network.
  join,

  /// Android's picker is up.
  joining,

  /// The frame goes out, and this phone comes back.
  sending,

  /// Waiting for the feeder to reach this phone.
  waiting,
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

  /// This listen is for a request an earlier run left, not one this run sent.
  bool listeningForEarlier = false;

  /// What an earlier run left on the feeder.
  PendingInstall pending = PendingInstall.nothing;
  InstallPaths? _paths;

  // The handover.
  Handover handover = Handover.starting;
  int? _joinAsk;

  /// Android's picker ended with no network: the feeder's network is not up
  /// yet, or the owner cancelled.
  bool joinFailed = false;

  /// The owner said they held Reset, so the feeder needs no second hold.
  bool _feederReset = false;

  /// This phone's broker heard the feeder after a frame: stock kept it, and
  /// has no setup network left.
  bool _frameTaken = false;
  String? _frameAddress;

  String? phoneIp;

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
  List<int>? _identityBytes;
  String? _address;
  InstallEnd_Stopped? stop;

  // After an uncertain request.
  Listening listening = Listening.idle;
  Duration listened = Duration.zero;
  bool stockSeen = false;
  String? listenerDetail;

  WifiCredentials? _home;
  bool _forget = false;

  /// The owner gave a new Wi-Fi for a feeder that never joined: the run goes
  /// through Bluetooth setup again, and the claim screen says how to get the
  /// feeder there.
  bool settingUpAgain = false;

  /// Android refused Bluetooth to a pick-up that may need it.
  bool bluetoothRefused = false;
  bool _permitted = false;

  InstallRun? _run;
  StreamSubscription<InstallUpdate>? _updates;
  InstallLink? _link;
  InstallListener? _listener;
  StreamSubscription<ListenUpdate>? _listenUpdates;
  StreamSubscription<void>? _heard;
  bool _holding = false;
  Timer? _holdHintTimer;

  /// This process's sockets go over the feeder's setup network.
  bool _onFeederNetwork = false;

  /// Android refused the foreground service, so only staying on screen keeps
  /// the broker alive.
  bool holdRefused = false;

  /// A pick-up asked for the phone's name or the Wi-Fi first: it goes on once
  /// it has them.
  bool _resumeAfterWifi = false;
  String? _resumeAddress;
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

  /// The run looks for a feeder advertising Bluetooth setup, rather than for
  /// one already on the owner's Wi-Fi.
  bool searchingBluetooth = false;

  /// Bluetooth has found nothing for [setupHintAfter]: a feeder that is not
  /// advertising by itself takes the owner's hold.
  bool setupHint = false;

  static const setupHintAfter = Duration(seconds: 20);

  /// What the run waits for now, when nothing on the screen says so already:
  /// for a long wait, so a screen is never just still.
  String? get waitWords => switch (stage) {
    InstallStage.claim
        when !searchingBluetooth && code.isEmpty && !windowAsked =>
      "Still looking for the feeder on your Wi-Fi. It's found once it has "
          'restarted and joined, which can take a few minutes.',
    InstallStage.sending when restarting =>
      'Waiting for the feeder to restart into Wellfed and come back. This can '
          'take a few minutes.',
    InstallStage.sending when percent >= 100 =>
      'The feeder has the firmware and is installing it. It restarts on its '
          'own, which can take a few minutes.',
    InstallStage.sending =>
      'Waiting for the feeder to ask for the next part of the firmware.',
    InstallStage.finishing when joining == CheckState.running =>
      'Waiting for the feeder to restart and join your Wi-Fi. This can take a '
          'few minutes.',
    InstallStage.finishing when !pressDispense =>
      'Waiting for the feeder to answer on your Wi-Fi.',
    _ => null,
  };

  /// A rough time left in stock's download, from how fast it has gone.
  Duration? get timeLeft {
    final since = _sendingSince;
    if (since == null || percent < 5 || percent >= 100) return null;
    final elapsed = DateTime.now().difference(since);
    return elapsed * ((100 - percent) / percent);
  }

  void _searchBluetooth() {
    searchingBluetooth = true;
    if (setupHint || _holdHintTimer != null) return;
    _holdHintTimer = Timer(setupHintAfter, () {
      setupHint = true;
      _notify();
    });
  }

  void _foundOverBluetooth() {
    _holdHintTimer?.cancel();
    _holdHintTimer = null;
    setupHint = false;
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
  /// Wi-Fi, then the Reset hold.
  Future<bool> begin() async {
    if (!await _permissions()) return false;
    phoneIp = await _ports.phoneAddress();
    _nextAfter(InstallStage.before);
    return true;
  }

  /// What the install needs from Android. Every way into a run or a listener
  /// comes through here, since a fresh launch can take any of them first.
  Future<bool> _permissions() async {
    if (_permitted) return true;
    final bluetooth = provisioning == ImageProvisioning.consumer;
    _permitted = await _ports.askPermissions(bluetooth: bluetooth);
    bluetoothRefused = !_permitted;
    return _permitted;
  }

  Future<void> setLabel(String label) async {
    await _book.setLabel(label);
    if (_resumeAfterWifi) {
      _resumeAfterWifi = false;
      unawaited(resume(address: _resumeAddress));
      return;
    }
    _nextAfter(InstallStage.naming);
  }

  void setHome(WifiCredentials? home) {
    _home = home;
    if (_resumeAfterWifi) {
      _resumeAfterWifi = false;
      unawaited(resume(address: _resumeAddress));
      return;
    }
    if (_feederReset) {
      unawaited(startHandover());
      return;
    }
    _nextAfter(InstallStage.wifi);
  }

  /// Back out of a screen. A pick-up that asked for it ends here: Continue from
  /// the first screen is the owner's choice of a new request.
  void back(InstallStage to) {
    _resumeAfterWifi = false;
    _resumeAddress = null;
    settingUpAgain = false;
    final paths = _paths;
    if (to == InstallStage.before && paths != null) {
      pending = _ports.pending(paths);
    }
    _go(to);
  }

  void _nextAfter(InstallStage from) {
    final next = switch (from) {
      InstallStage.before when _book.label.isEmpty => InstallStage.naming,
      InstallStage.before || InstallStage.naming => InstallStage.wifi,
      _ => InstallStage.reset,
    };
    _go(next);
  }

  /// The owner held Reset: start the broker, then the handover. A feeder that
  /// kept an earlier run's frame only has to reach this phone again.
  Future<void> startHandover() async {
    if (_run != null) return;
    phoneIp = await _ports.phoneAddress();
    final ip = phoneIp;
    if (ip == null) {
      _notify();
      return;
    }
    // Stock kept the address the frame named, so it looks nowhere else; a new
    // frame is the only way to tell it this one, and that takes another hold.
    if (_frameTaken && ip != _frameAddress) {
      _frameTaken = false;
      _feederReset = false;
      _go(InstallStage.reset);
      return;
    }
    _feederReset = true;
    joinFailed = false;
    handover = _frameTaken ? Handover.waiting : Handover.starting;
    _go(InstallStage.handover);
    await _start(
      _frameTaken ? InstallStart.frameTaken : InstallStart.reset,
      ip,
    );
  }

  /// Join the feeder's network. The owner may try again: the picker comes up
  /// empty until the feeder's network is up.
  Future<void> joinFeeder() async {
    final ask = _joinAsk;
    final run = _run;
    if (ask == null || run == null || handover == Handover.joining) return;
    handover = Handover.joining;
    joinFailed = false;
    _notify();
    try {
      await _ports.joinFeederNetwork();
    } catch (_) {
      handover = Handover.join;
      joinFailed = true;
      _notify();
      return;
    }
    // The run ended while the picker was up: nothing will ever ask to leave.
    if (_disposed || !identical(_run, run)) {
      await _ports.leaveFeederNetwork();
      return;
    }
    _joinAsk = null;
    _onFeederNetwork = true;
    handover = Handover.sending;
    _notify();
    run.answer(id: ask, answer: const InstallAnswer.done());
  }

  /// Back on the Wi-Fi: the frame named this phone's address, so another one
  /// would leave the feeder looking in the wrong place.
  Future<void> _leaveFeeder(void Function(InstallAnswer) answer) async {
    final address = await _ports.leaveFeederNetwork();
    _onFeederNetwork = false;
    if (address == phoneIp && address != null) {
      handover = Handover.waiting;
      answer(const InstallAnswer.done());
      return;
    }
    answer(
      InstallAnswer.failed(
        reason:
            'This phone came back to its Wi-Fi at ${address ?? "no address"}, '
            'but told the feeder $phoneIp.',
      ),
    );
  }

  /// Pick up from bank 0: the replacement runs, and only its owner is
  /// missing, or the backup and the promotion. With `address`, it answers
  /// there. A consumer image that never paired needs its Wi-Fi for Bluetooth
  /// setup, which only an earlier run on this screen would have asked for.
  Future<void> resume({String? address}) async {
    if (_run != null) return;
    if (!await _permissions()) {
      _notify();
      return;
    }
    _stopListening();
    listening = Listening.idle;
    if (_book.label.isEmpty) {
      _resumeAfterWifi = true;
      _resumeAddress = address;
      stop = null;
      _go(InstallStage.naming);
      return;
    }
    if (provisioning == ImageProvisioning.consumer &&
        pending != PendingInstall.paired &&
        address == null &&
        _home == null) {
      _resumeAfterWifi = true;
      _resumeAddress = address;
      stop = null;
      _go(InstallStage.wifi);
      return;
    }
    final ip = phoneIp ?? await _ports.phoneAddress() ?? '';
    stop = null;
    _resetProgress();
    _go(InstallStage.claim);
    await _start(InstallStart.bank0, ip, address: address);
  }

  /// A request from an earlier run has no settled outcome: listen for what
  /// became of it, as a run that just stopped there would.
  Future<void> listenForOpenRequest() async {
    if (!await _permissions()) {
      _notify();
      return;
    }
    stop = InstallEnd_Stopped(
      case_: const StopCase.uncertain(),
      resume: Resume.nothing,
      detail: stop?.detail ?? '',
    );
    listeningForEarlier = true;
    _go(InstallStage.stopped);
    await listen();
  }

  void _resetProgress() {
    code = const [];
    deadline = null;
    _windowAsk = null;
    pressDispense = false;
    percent = 0;
    restarting = false;
    joining = CheckState.waiting;
    copying = CheckState.waiting;
    copied = 0;
    checked = CheckState.waiting;
    promoting = CheckState.waiting;
    written = 0;
    confirming = CheckState.waiting;
    sessionDropped = false;
    searchingBluetooth = false;
    _foundOverBluetooth();
  }

  /// A feeder that never joined: take a new Wi-Fi and go through Bluetooth
  /// setup again. Only stock's request cannot be sent twice, and this asks stock
  /// nothing.
  void newWifiForSetup() {
    settingUpAgain = true;
    _home = null;
    _resumeAfterWifi = true;
    _resumeAddress = null;
    stop = null;
    _go(InstallStage.wifi);
  }

  /// Wi-Fi the feeder could not be given, or none: ask again, and go on by the
  /// route that stopped.
  void changeWifi() {
    _home = null;
    _resumeAfterWifi = stop?.resume == Resume.fromBank0;
    _resumeAddress = null;
    stop = null;
    _go(InstallStage.wifi);
  }

  /// A feeder that never joined may still be restarting: look again.
  Future<void> keepWaiting() async {
    settingUpAgain = false;
    await again();
  }

  /// The stop's way on: from the start, or from bank 0, as the run said.
  Future<void> again({bool forget = false}) async {
    _forget = forget;
    final resumeFrom = stop?.resume;
    if (resumeFrom == Resume.fromBank0) {
      await resume();
      return;
    }
    if (!await _permissions()) {
      _notify();
      return;
    }
    phoneIp = await _ports.phoneAddress();
    stop = null;
    _resetProgress();
    listening = Listening.idle;
    listeningForEarlier = false;
    waitReason = null;
    restartAgain = false;
    // A fresh launch can get here from an open request, with neither asked yet.
    if (_book.label.isEmpty) {
      _go(InstallStage.naming);
    } else if (_home == null) {
      _go(InstallStage.wifi);
    } else if (_feederReset) {
      await startHandover();
    } else {
      _go(InstallStage.reset);
    }
  }

  Future<void> _start(InstallStart start, String ip, {String? address}) async {
    final paths = _paths;
    // One run at a time: a second would orphan the first, with no way left
    // to stop it and the broker's port still held.
    if (paths == null || _run != null) return;
    final forget = _forget;
    _forget = false;
    final run = _ports.newRun(
      InstallSpec(
        start: start,
        address: address,
        phoneIp: ip,
        home: _home,
        label: _book.label,
        feederName: defaultFeederName,
        knownFeeders: _book.feeders,
        storePath: paths.store,
        imageDir: paths.image,
        dataDir: paths.data,
        forgetEnrollment: forget,
        setupAgain: settingUpAgain,
      ),
    );
    _run = run;
    _notify();
    await _hold('Keep the feeder plugged in.');
    if (_disposed) return;
    InstallEnd lost(String detail) => InstallEnd.stopped(
      case_: const StopCase.other(),
      resume: Resume.nothing,
      detail: detail,
    );
    _updates = run.run().listen(
      (update) => unawaited(_update(run, update)),
      onError: (Object error) => _ended(lost('$error')),
      // A run's last update is its end, which cancels this first; a stream
      // that closes without one lost its thread.
      onDone: () {
        if (identical(_run, run)) _ended(lost('the install stopped answering'));
      },
    );
    _notify();
  }

  Future<void> _hold(String text) async {
    _holding = true;
    holdRefused = !await _ports.holdProcess(text);
    // The screen went away during the wait, and its release ran before this
    // hold took effect.
    if (_disposed) await _ports.releaseProcess();
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
      // The screen says to restart it; the broker is already listening.
      case InstallAsk_RestartFeeder():
        answer(const InstallAnswer.done());
      case InstallAsk_JoinFeederNetwork():
        _joinAsk = id;
        handover = Handover.join;
      case InstallAsk_LeaveFeederNetwork():
        await _leaveFeeder(answer);
      case InstallAsk_RecognizeFeeder(:final device, :final address):
        // Whatever reached the broker took the frame, and has no setup network
        // to take another.
        _frameTaken = true;
        _frameAddress = phoneIp;
        this.device = device;
        deviceAddress = address;
        _recognizeAsk = id;
        stage = InstallStage.confirm;
      case InstallAsk_OpenRecoveryWindow():
        _windowAsk = id;
        stage = InstallStage.claim;
      case InstallAsk_OpenLink():
        stage = InstallStage.claim;
        _searchBluetooth();
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
        _searchBluetooth();
        await _hold('The feeder is waiting for its owner.');
      case InstallEvent_Code(:final code, :final seconds):
        stage = InstallStage.claim;
        this.code = code;
        _foundOverBluetooth();
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
        _identityBytes = identity;
        _identity = identity
            .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
            .join();
        _address = address ?? _address;
        joining = rebooting ? CheckState.running : CheckState.waiting;
      case InstallEvent_Joined(:final address):
        stage = InstallStage.finishing;
        settingUpAgain = false;
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
      case InstallEvent_SessionDropped():
        sessionDropped = true;
      case InstallEvent_StockServerStarted() ||
          InstallEvent_AwaitingStock() ||
          InstallEvent_StockServerEnded() ||
          InstallEvent_NoSetupFeeder():
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
      // A pick-up of a feeder the owner already named.
      keepName: true,
    );
  }

  void _ended(InstallEnd end) {
    _run = null;
    _joinAsk = null;
    if (_onFeederNetwork) {
      _onFeederNetwork = false;
      unawaited(_ports.leaveFeederNetwork());
    }
    unawaited(_updates?.cancel());
    _updates = null;
    unawaited(_settle());
    _closeLink();
    _foundOverBluetooth();
    if (end case InstallEnd_Stopped(
      case_: StopCase_NoFeederCame() || StopCase_TwoFeeders(),
    )) {
      _frameTaken = false;
    }
    switch (end) {
      case InstallEnd_Done():
        stage = InstallStage.done;
        // A pick-up past the backup never read it again.
        final (identity, paths) = (_identityBytes, _paths);
        if (backupPath == null && identity != null && paths != null) {
          backupPath = _ports.keptBackup(paths, identity);
        }
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

  /// A feeder still linked does not advertise, so a pick-up would never find
  /// it. The run asks for this itself, unless it stopped first.
  void _closeLink() {
    final link = _link;
    _link = null;
    unawaited(
      link?.close().catchError((Object _) {
        // A link the feeder dropped has nothing left to close.
      }),
    );
  }

  /// The owner's word on the feeder that connected.
  void recognize({required bool yes}) {
    final id = _recognizeAsk;
    if (id == null) return;
    _recognizeAsk = null;
    // Somebody else's client: the feeder may still be waiting for its frame.
    if (!yes) _frameTaken = false;
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
    _notify();
    await _hold('Listening for the feeder. Leave it plugged in.');
    if (_disposed || listening != Listening.running) return;
    final listener = _ports.newListener(paths);
    _listener = listener;
    _listenUpdates = listener.run().listen(
      _heardFromListener,
      onError: (Object error) {
        listenerDetail = '$error';
        _listenEnded(Listening.failed);
      },
      // Its last update ends the listening; a stream that closes before one
      // lost its thread.
      onDone: () {
        if (identical(_listener, listener) && listening == Listening.running) {
          listenerDetail = 'the listener stopped answering';
          _listenEnded(Listening.failed);
        }
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
      case ListenUpdate_Replacement(:final address):
        unawaited(resume(address: address));
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
    if (_onFeederNetwork) {
      _onFeederNetwork = false;
      unawaited(_ports.leaveFeederNetwork());
    }
    unawaited(_updates?.cancel());
    _stopListening();
    _closeLink();
    _holdHintTimer?.cancel();
    unawaited(_release());
    super.dispose();
  }
}
