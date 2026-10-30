import 'dart:async';

import 'package:flutter/foundation.dart';

import '../platform/restore_ports.dart';
import '../src/rust/api/install.dart';
import '../src/rust/api/restore.dart';
import 'install.dart' show CheckState;

/// Which screen a restore shows. The steps themselves are Rust's: this follows
/// what the run reports.
enum RestoreStage {
  loading,

  /// No copy of stock to put back: pick one from a file.
  noBackup,

  /// The facts, the warnings and the owner's word.
  before,

  /// The press, the sending, the restart.
  running,

  /// Bank 1 is activated: the feeder restarts into the vendor's firmware.
  done,

  /// The feeder stopped answering once the offer was out.
  uncertain,
  stopped,
}

/// One restore of a feeder's original firmware.
class RestoreFlow extends ChangeNotifier {
  RestoreFlow(
    this._ports, {
    required this.identity,
    required this.feederName,
    this.address,
  });

  final RestorePorts _ports;
  final List<int> identity;
  final String feederName;
  final String? address;

  RestoreStage stage = RestoreStage.loading;
  String? _dataDir;

  // The copy of stock.
  String? _backup;
  String? version;
  DateTime? backupDate;

  /// The owner picked this copy; it was not kept during the install.
  bool picked = false;

  /// Why the last copy looked at is not stock.
  String? notStock;

  bool understood = false;

  // The run.
  /// Waiting on the owner at the panel: Dispense enrolls the restore's key.
  bool pressDispense = false;
  CheckState pressing = CheckState.running;
  CheckState sending = CheckState.waiting;
  double written = 0;
  CheckState restarting = CheckState.waiting;
  bool sessionDropped = false;

  /// Android refused the foreground service, so only staying on screen keeps
  /// the run alive.
  bool holdRefused = false;

  RestoreStop? stop;

  /// Why the end is not known, at [RestoreStage.uncertain].
  UncertainWhy? uncertain;
  String? detail;

  RestoreRun? _run;
  StreamSubscription<RestoreUpdate>? _updates;
  bool _holding = false;
  bool _disposed = false;

  /// The offer may be out: leaving would end the run at nothing the owner
  /// could act on, so the screen offers no way back until it ends.
  bool get committed =>
      stage == RestoreStage.running && sending != CheckState.waiting;

  /// A new run may go again: nothing was offered.
  bool get mayRetry => switch (stop) {
    RestoreStop.noPress ||
    RestoreStop.enrollmentUnknown ||
    RestoreStop.keyRefused ||
    RestoreStop.keyGone ||
    RestoreStop.notAnswering ||
    RestoreStop.cancelled => true,
    _ => false,
  };

  Future<void> load() async {
    final dataDir = await _ports.dataDir();
    _dataDir = dataDir;
    final kept = _ports.keptBackup(dataDir, identity);
    if (kept == null) {
      stage = RestoreStage.noBackup;
    } else {
      await _look(kept, isPicked: false);
    }
    _changed();
  }

  /// Look at a file the owner picks, through the same checks as a kept copy.
  Future<void> pickFile() async {
    final path = await _ports.pickBackup();
    if (path == null) return;
    await _look(path, isPicked: true);
    _changed();
  }

  Future<void> _look(String path, {required bool isPicked}) async {
    switch (_ports.checkBackup(path)) {
      case BackupCheck_Ready(version: final ready):
        _backup = path;
        version = ready;
        backupDate = await _ports.backupDate(path);
        picked = isPicked;
        notStock = null;
        stage = RestoreStage.before;
      case BackupCheck_Missing():
        notStock = null;
        stage = RestoreStage.noBackup;
      case BackupCheck_NotStock(detail: final why):
        notStock = why;
        stage = RestoreStage.noBackup;
    }
  }

  void setUnderstood(bool value) {
    understood = value;
    _changed();
  }

  /// The owner's word is what starts it, each time.
  Future<void> start() async {
    final (dataDir, backup) = (_dataDir, _backup);
    // `_run` is set only after the hold below, so the stage guards a second tap.
    if (!understood ||
        stage != RestoreStage.before ||
        dataDir == null ||
        backup == null) {
      return;
    }
    pressDispense = false;
    pressing = CheckState.running;
    sending = CheckState.waiting;
    restarting = CheckState.waiting;
    written = 0;
    sessionDropped = false;
    stop = null;
    uncertain = null;
    detail = null;
    stage = RestoreStage.running;
    _changed();

    _holding = true;
    holdRefused = !await _ports.holdProcess(
      'Putting the original firmware back on $feederName',
    );
    if (_disposed) {
      // The screen went away during the wait, and its release ran before this
      // hold took effect.
      await _ports.releaseProcess();
      return;
    }

    final run = _ports.newRun(
      RestoreSpec(
        identity: Uint8List.fromList(identity),
        address: address,
        dataDir: dataDir,
        backup: backup,
      ),
    );
    _run = run;
    _updates = run.run().listen(_heard, onDone: _closed);
    _changed();
  }

  /// Back at the facts after a stop that left the way open. The owner gives
  /// their word again.
  void again() {
    understood = false;
    stage = RestoreStage.before;
    _changed();
  }

  void _heard(RestoreUpdate update) {
    switch (update) {
      case RestoreUpdate_Event(:final event):
        _event(event);
      case RestoreUpdate_Ask(:final id, :final ask):
        _ask(id, ask);
      case RestoreUpdate_Ended(:final end):
        _ended(end);
    }
    _changed();
  }

  void _event(InstallEvent event) {
    switch (event) {
      case InstallEvent_PressDispense():
        pressDispense = true;
      case InstallEvent_Promoting():
        pressDispense = false;
        pressing = CheckState.done;
        sending = CheckState.running;
      case InstallEvent_Written(:final written, :final length):
        this.written = length == 0 ? 0 : written / length;
        sessionDropped = false;
      case InstallEvent_SessionDropped():
        sessionDropped = true;
      case InstallEvent_Activated():
        sending = CheckState.done;
        restarting = CheckState.running;
      default:
        break;
    }
  }

  Future<void> _ask(int id, InstallAsk ask) async {
    final InstallAnswer answer;
    if (ask case InstallAsk_Locate(:final identity)) {
      answer = InstallAnswer.located(address: await _ports.locate(identity));
    } else {
      answer = const InstallAnswer.failed(
        reason: 'A restore asks only where the feeder is',
      );
    }
    _run?.answer(id: id, answer: answer);
  }

  void _ended(RestoreEnd end) {
    switch (end) {
      case RestoreEnd_Activated():
        stage = RestoreStage.done;
      case RestoreEnd_Uncertain(:final why, detail: final words):
        uncertain = why;
        detail = words;
        stage = RestoreStage.uncertain;
      case RestoreEnd_Stopped(:final case_, detail: final words):
        stop = case_;
        detail = words;
        stage = RestoreStage.stopped;
    }
    _finish();
  }

  /// The stream closed with no end: a run that says nothing more has not
  /// said how it ended, so after an offer it is uncertain.
  void _closed() {
    if (stage != RestoreStage.running) return;
    stage = sending == CheckState.waiting
        ? RestoreStage.stopped
        : RestoreStage.uncertain;
    if (stage == RestoreStage.stopped) stop = RestoreStop.other;
    if (stage == RestoreStage.uncertain) uncertain = UncertainWhy.failed;
    _finish();
    _changed();
  }

  void _finish() {
    _run = null;
    unawaited(_updates?.cancel());
    _updates = null;
    if (_holding) {
      _holding = false;
      unawaited(_ports.releaseProcess());
    }
    if (_disposed) _forget();
  }

  /// The owner has left: a live key has no expiry, so it does not outlast
  /// them. Only a [start] from the same screen reuses it.
  void _forget() {
    if (_dataDir case final dataDir?) _ports.forgetKey(dataDir, identity);
  }

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    if (_run case final run?) {
      // Its end does the rest: the key goes once the run can no longer use it.
      run.stop();
    } else {
      _finish();
    }
    super.dispose();
  }
}
