import 'dart:async';

import 'package:flutter/foundation.dart';

import 'package:flutter_reactive_ble/flutter_reactive_ble.dart'
    show DiscoveredDevice;

import '../platform/discovery.dart';
import '../platform/setup_link.dart';
import '../src/rust/api/error.dart';
import '../src/rust/api/model.dart';
import '../src/rust/api/setup.dart';
import '../src/rust/api/store.dart';
import 'app_state.dart';

enum PairingPhase {
  /// Looking for a feeder in setup mode.
  finding,
  connecting,

  /// Showing the code; the feeder takes presses until the deadline.
  code,

  /// The feeder proved the code and is restarting onto its Wi-Fi; the
  /// phone looks for it on the LAN.
  joining,
  done,
  failed,
}

/// Why pairing stopped.
enum PairingFailure {
  /// No feeder in setup mode answered the scan.
  notFound,

  /// The radio link failed or dropped.
  link,

  /// The feeder said no; see [PairingFlow.refusal].
  refused,

  /// The feeder did not prove the code: mistyped, or another device.
  mismatch,

  /// No answer in time.
  timeout,

  /// The recovery window was not open (the feeder closed the connection).
  windowClosed,

  /// Paired, but the feeder never showed up on this network.
  notOnLan,

  /// The phone's name or the Wi-Fi fails the feeder's rules.
  input,

  /// Anything else; [PairingFlow.message] has the detail.
  other,
}

/// One pairing, BLE setup or LAN recovery, from the scan to the feeder
/// answering on the LAN. The protocol runs in Rust; this moves its bytes
/// and tracks what the screen shows.
class PairingFlow extends ChangeNotifier {
  PairingFlow(this._app);

  static const handshakeWait = Duration(seconds: 10);

  /// Beyond the feeder's own window, for its reply to arrive.
  static const confirmSlack = Duration(seconds: 10);

  /// A feeder restarts and rejoins its Wi-Fi in about 30 s.
  static const rejoinWait = Duration(seconds: 90);

  final AppState _app;

  PairingPhase phase = PairingPhase.finding;

  /// The code as panel positions, 1 (Lock) to 4 (Reset) from the left.
  List<int> code = const [];

  /// When the feeder stops taking presses.
  DateTime? deadline;
  PairingFailure? failure;
  SetupRefusal? refusal;
  String? message;
  Enrolled? enrolled;
  KnownFeeder? feeder;

  SetupLink? _link;
  StreamSubscription<List<int>>? _notifications;
  Timer? _timer;
  Completer<SetupStep>? _ended;
  StreamSubscription<SetupStep>? _recovering;
  Recovery? _recovery;
  Completer<void>? _recoveryDone;
  Future<void> _writing = Future.value();
  bool _disposed = false;

  /// BLE setup: `wifi` for the feeder's next join, or `null` to keep its own.
  Future<void> runSetup({required WifiCredentials? wifi}) async {
    final SetupDriver driver;
    try {
      driver = SetupDriver(key: _app.key, label: _app.label, wifi: wifi);
    } on BridgeError catch (error) {
      _stop(_failureOf(error), error.message);
      return;
    }
    code = driver.code();

    _set(PairingPhase.finding);
    final DiscoveredDevice device;
    try {
      device = await SetupLink.find();
    } on TimeoutException {
      _stop(PairingFailure.notFound, null);
      return;
    } catch (error) {
      _stop(PairingFailure.link, '$error');
      return;
    }
    // The scan cannot be cut short, but a screen that went away must not
    // then hold the feeder's only link.
    if (_disposed) return;

    final SetupStep end;
    try {
      _set(PairingPhase.connecting);
      final link = await SetupLink.open(device);
      _link = link;
      if (_disposed) return;

      final ended = Completer<SetupStep>();
      // A failure after setup stopped waiting is nobody's error.
      ended.future.ignore();
      _ended = ended;
      _notifications = link.notifications.listen(
        (bytes) => _received(driver, bytes),
        onError: (Object error) => _fail(PairingFailure.link, '$error'),
        onDone: () => _fail(PairingFailure.link, 'the feeder dropped the link'),
      );
      _arm(handshakeWait);
      await link.write(driver.start());
      end = await ended.future;
    } on TimeoutException {
      _stop(PairingFailure.timeout, null);
      return;
    } on _Failed catch (failed) {
      _stop(failed.why, failed.detail);
      return;
    } on BridgeError catch (error) {
      _stop(_failureOf(error), error.message);
      return;
    } catch (error) {
      _stop(PairingFailure.link, '$error');
      return;
    } finally {
      await _closeLink();
    }
    await _finish(end, knownAddress: null);
  }

  /// LAN recovery with the feeder at `address`, whose window is open.
  Future<void> runRecovery({required String address}) async {
    final Recovery recovery;
    try {
      recovery = Recovery(key: _app.key, label: _app.label);
    } on BridgeError catch (error) {
      _stop(_failureOf(error), error.message);
      return;
    }
    code = recovery.code();
    _recovery = recovery;
    _set(PairingPhase.connecting);

    SetupStep? end;
    final done = Completer<void>();
    _recoveryDone = done;
    // Held so dispose can cancel it: the stream waits out the code window.
    _recovering = recovery
        .run(address: address)
        .listen(
          (step) {
            if (step is SetupStep_EnterCode) {
              _showCode(step.seconds);
            } else {
              end = step;
            }
          },
          onError: (Object error) {
            if (!done.isCompleted) done.completeError(error);
          },
          onDone: () {
            if (!done.isCompleted) done.complete();
          },
        );
    try {
      await done.future;
    } on BridgeError catch (error) {
      _stop(_failureOf(error), error.message);
      return;
    } finally {
      _timer?.cancel();
      _recovering = null;
    }
    if (_disposed) return;
    final ended = end;
    if (ended == null) {
      _stop(PairingFailure.other, 'the feeder ended without an answer');
      return;
    }
    await _finish(ended, knownAddress: address);
  }

  /// One notification: hand it to the session and act on what it says.
  Future<void> _received(SetupDriver driver, List<int> bytes) async {
    final ended = _ended;
    if (ended == null || ended.isCompleted) return;
    try {
      for (final step in driver.notified(bytes: bytes)) {
        switch (step) {
          case SetupStep_Write(:final pieces):
            _arm(handshakeWait);
            // Notifications can arrive while an earlier answer is still
            // being written; each answer goes out whole, in order.
            final link = _link;
            if (link != null) {
              _writing = _writing.then((_) => link.write(pieces));
              await _writing;
            }
          case SetupStep_EnterCode(:final seconds):
            _showCode(seconds);
          case SetupStep_Done() || SetupStep_Refused():
            _timer?.cancel();
            ended.complete(step);
        }
      }
    } on BridgeError catch (error) {
      _fail(_failureOf(error), error.message);
    } catch (error) {
      _fail(PairingFailure.link, '$error');
    }
  }

  void _showCode(int seconds) {
    deadline = DateTime.now().add(Duration(seconds: seconds));
    _arm(Duration(seconds: seconds) + confirmSlack);
    _set(PairingPhase.code);
  }

  /// After the exchange: remember the feeder, then find it on the LAN.
  Future<void> _finish(SetupStep end, {required String? knownAddress}) async {
    switch (end) {
      case SetupStep_Refused(refusal: final why):
        refusal = why;
        _stop(PairingFailure.refused, null);
        return;
      case SetupStep_Done(enrolled: final how, feeder: final identity):
        enrolled = how;
        final hex = identityHex(identity: identity);
        _set(PairingPhase.joining);

        // After BLE setup the feeder restarts onto its new Wi-Fi first.
        final address = knownAddress ?? await Discovery.locate(hex, rejoinWait);
        final known = KnownFeeder(
          identity: hex,
          name: 'Feeder',
          address: address,
        );
        // The feeder enrolled this phone whatever happens here; a failed
        // save is said so, not left on "joining".
        try {
          await _app.remember(known);
        } on BridgeError catch (error) {
          _stop(
            PairingFailure.other,
            'paired, but not saved: ${error.message}',
          );
          return;
        }
        feeder = known;
        if (address == null) {
          _stop(PairingFailure.notOnLan, null);
          return;
        }
        _set(PairingPhase.done);
      case SetupStep_Write() || SetupStep_EnterCode():
        _stop(PairingFailure.other, 'the session ended mid-exchange');
    }
  }

  /// Fail after `wait` with no message from the feeder.
  void _arm(Duration wait) {
    _timer?.cancel();
    _timer = Timer(wait, () => _fail(PairingFailure.timeout, null));
  }

  void _fail(PairingFailure why, String? detail) {
    final ended = _ended;
    if (ended != null && !ended.isCompleted) {
      ended.completeError(_Failed(why, detail));
    }
  }

  Future<void> _closeLink() async {
    _timer?.cancel();
    await _notifications?.cancel();
    _notifications = null;
    await _link?.close();
    _link = null;
  }

  PairingFailure _failureOf(BridgeError error) => switch (error.kind) {
    ErrorKind.codeMismatch => PairingFailure.mismatch,
    ErrorKind.timeout => PairingFailure.timeout,
    ErrorKind.closed => PairingFailure.windowClosed,
    ErrorKind.label || ErrorKind.credentials => PairingFailure.input,
    _ => PairingFailure.other,
  };

  void _stop(PairingFailure why, String? detail) {
    failure = why;
    message = detail;
    _set(PairingPhase.failed);
  }

  void _set(PairingPhase next) {
    phase = next;
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _fail(PairingFailure.other, 'the screen closed');
    // Hang up in Rust too, so the feeder's window is free for a retry.
    _recovery?.close();
    unawaited(_recovering?.cancel());
    final recovering = _recoveryDone;
    if (recovering != null && !recovering.isCompleted) recovering.complete();
    unawaited(_closeLink());
    super.dispose();
  }
}

class _Failed implements Exception {
  const _Failed(this.why, this.detail);

  final PairingFailure why;
  final String? detail;
}
