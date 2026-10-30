import 'dart:async';

import 'package:flutter/foundation.dart';

import '../platform/feeder_ports.dart' show UpdatePush;
import '../src/rust/api/feeder.dart';
import '../src/rust/api/install.dart' show InstallAsk, InstallAsk_Locate;
import '../src/rust/api/model.dart';
import '../src/rust/api/update.dart';
import 'feeder.dart';

enum UpdateStage { idle, sending, restarting, ended }

/// One feeder's update, for as long as its controller lives. A run goes once:
/// [start] does nothing while one runs or while its end is unknown, wherever
/// the owner has been meanwhile.
class UpdateFlow extends ChangeNotifier {
  UpdateFlow(this._feeder);

  final FeederController _feeder;

  UpdateStage stage = UpdateStage.idle;
  int written = 0;
  int length = 0;
  bool reconnecting = false;
  bool cancelling = false;
  UpdateEnd? end;

  /// Android refused the foreground service, so only staying on screen keeps
  /// the run alive.
  bool holdRefused = false;

  /// Why a look at an unknown end settled nothing.
  bool stillUnknown = false;

  UpdatePush? _push;
  StreamSubscription<UpdateProgress>? _run;
  bool _holding = false;
  bool _disposed = false;

  bool get running =>
      stage == UpdateStage.sending || stage == UpdateStage.restarting;

  /// Only the feeder can tell how it went, so this stays until it has.
  bool get endUnknown =>
      end is UpdateEnd_Uncertain || end is UpdateEnd_Unconfirmed;

  Future<void> start() async {
    if (stage != UpdateStage.idle) return;
    stage = UpdateStage.sending;
    written = 0;
    length = 0;
    reconnecting = false;
    stillUnknown = false;
    end = null;
    _changed();

    final ports = _feeder.ports;
    final name = _feeder.known.name;
    _holding = true;
    holdRefused = !await ports.holdProcess('Updating $name');
    if (_disposed) {
      await ports.releaseProcess();
      return;
    }

    final push = ports.pushUpdate(
      _feeder.known.identity,
      _feeder.known.address,
    );
    _push = push;
    _run = push.progress.listen(
      _heard,
      // Rust ends every run with a value; a stream that closes without one
      // lost its thread, and what the feeder did is not known here.
      onDone: () {
        if (running) _ended(const UpdateEnd.uncertain());
      },
    );
    _changed();
  }

  void _heard(UpdateProgress progress) {
    switch (progress) {
      case UpdateProgress_Sending(written: final sent, length: final total):
        written = sent;
        length = total;
        reconnecting = false;
      case UpdateProgress_Reconnecting():
        reconnecting = true;
      case UpdateProgress_Ask(:final id, :final ask):
        unawaited(_locate(id, ask));
      case UpdateProgress_Activated():
        stage = UpdateStage.restarting;
        reconnecting = false;
      case UpdateProgress_Ended(end: final last):
        _ended(last);
        return;
    }
    _changed();
  }

  Future<void> _locate(int id, InstallAsk ask) async {
    if (ask is! InstallAsk_Locate) {
      _push?.located(id, null);
      return;
    }
    final found = await _feeder.ports.locate(ask.identity, _locateWait);
    _push?.located(id, found);
  }

  static const _locateWait = Duration(seconds: 12);

  void _ended(UpdateEnd last) {
    stage = UpdateStage.ended;
    end = last;
    cancelling = false;
    _release();
    _changed();
  }

  void _release() {
    _push = null;
    unawaited(_run?.cancel());
    _run = null;
    if (_holding) {
      _holding = false;
      unawaited(_feeder.ports.releaseProcess());
    }
  }

  /// Ask to stop, once. The run reports how it ended.
  Future<FeederReply> cancel() async {
    cancelling = true;
    _changed();
    final reply = await _feeder.ask(const FeederRequest.cancelUpdate());
    cancelling = false;
    _changed();
    return reply;
  }

  /// The owner has read an end that says how it went: the firmware screen may
  /// offer the update again, as their own new decision.
  void acknowledge() {
    if (stage != UpdateStage.ended || endUnknown) return;
    stage = UpdateStage.idle;
    end = null;
    _changed();
  }

  /// Read what the feeder says of an unknown end. Only its word that nothing is
  /// under way settles it; a read goes as often as the owner looks.
  Future<void> lookAgain() async {
    if (!endUnknown) return;
    final reply = await _feeder.ask(const FeederRequest.updateState());
    stillUnknown =
        !(reply is FeederReply_Update &&
            reply.update.phase == UpdatePhase.idle);
    if (!stillUnknown) {
      stage = UpdateStage.idle;
      end = null;
    }
    _changed();
  }

  void _changed() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _release();
    super.dispose();
  }
}
