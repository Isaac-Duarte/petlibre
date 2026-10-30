import 'dart:async';

import 'package:flutter/foundation.dart';

import 'package:flutter_reactive_ble/flutter_reactive_ble.dart'
    show DiscoveredDevice;

import '../platform/pairing_ports.dart';
import '../platform/setup_link.dart';
import '../src/rust/api/error.dart';
import '../src/rust/api/model.dart';
import '../src/rust/api/setup.dart';
import '../src/rust/api/store.dart';
import 'app_state.dart';

export '../src/rust/api/store.dart' show KnownFeeder;

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
  PairingFlow(this._app, this._ports);

  static const handshakeWait = Duration(seconds: 10);

  /// Beyond the feeder's own window, for its reply to arrive.
  static const confirmSlack = Duration(seconds: 10);

  /// A feeder restarts and rejoins its Wi-Fi in about 30 s.
  static const rejoinWait = Duration(seconds: 90);

  final FeederBook _app;
  final PairingPorts _ports;

  PairingPhase phase = PairingPhase.finding;

  /// The code as panel positions, 1 (Lock) to 4 (Reset) from the left.
  List<int> code = const [];

  /// When the feeder stops taking presses.
  DateTime? deadline;
  PairingFailure? failure;
  SetupRefusal? refusal;
  String? message;
  Enrolled? enrolled;
  WifiSaved? wifiSaved;
  KnownFeeder? feeder;

  /// What this phone calls the feeder; only this phone uses it.
  String feederName = defaultFeederName;

  /// The network the feeder was given, for "Keep this phone on …".
  String? network;

  /// The feeder the owner connected to, so a new code goes to it without
  /// asking again.
  DiscoveredDevice? chosen;

  /// Feeders in setup mode heard so far, by radio id; each sighting
  /// replaces the last, so the signal is current.
  final found = <String, DiscoveredDevice>{};

  static const defaultFeederName = 'Feeder';

  Completer<DiscoveredDevice>? _chosen;
  BleLink? _link;
  StreamSubscription<List<int>>? _notifications;
  Timer? _timer;
  Completer<SetupStep>? _ended;
  StreamSubscription<SetupStep>? _recovering;
  Recovery? _recovery;
  Completer<void>? _recoveryDone;
  Future<void> _writing = Future.value();
  bool _disposed = false;

  /// BLE setup: `wifi` for the feeder's next join, or `null` to keep its
  /// own; `name` is what this phone calls the feeder. With `device`, the
  /// feeder an earlier try connected to, the search is skipped.
  Future<void> runSetup({
    required WifiCredentials? wifi,
    required String name,
    DiscoveredDevice? device,
  }) async {
    final SetupDriver driver;
    try {
      driver = _ports.newDriver(_app.label, wifi);
    } on BridgeError catch (error) {
      _stop(_failureOf(error), error.message);
      return;
    }
    code = driver.code();
    feederName = name;
    network = wifi?.ssid;

    final DiscoveredDevice target;
    if (device != null) {
      target = device;
    } else {
      _set(PairingPhase.finding);
      try {
        target = await _pick();
      } on TimeoutException {
        _stop(PairingFailure.notFound, null);
        return;
      } on _Failed catch (failed) {
        _stop(failed.why, failed.detail);
        return;
      }
    }
    chosen = target;
    // A screen that went away must not then hold the feeder's only link.
    if (_disposed) return;

    final SetupStep end;
    try {
      _set(PairingPhase.connecting);
      final link = await _ports.open(target);
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
  Future<void> runRecovery({
    required String address,
    required String name,
  }) async {
    feederName = name;
    final Recovery recovery;
    try {
      recovery = _ports.newRecovery(_app.label);
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
    // Rust reports a failed recovery as a step. Anything else that arrives
    // as an error (a panic, a lost stream) is still a screen to show, not an
    // exception for nobody to catch.
    // Held so dispose can cancel it: the stream waits out the code window.
    _recovering = recovery
        .run(address: address)
        .listen(
          (step) {
            switch (step) {
              case SetupStep_EnterCode(:final seconds):
                _showCode(seconds);
              case SetupStep_Failed(:final error):
                if (!done.isCompleted) done.completeError(error);
              default:
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
    } catch (error) {
      _stop(PairingFailure.other, '$error');
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

  /// Scan until the owner picks a feeder; a timeout when none shows up.
  Future<DiscoveredDevice> _pick() async {
    final chosen = Completer<DiscoveredDevice>();
    _chosen = chosen;
    final scanning = _ports.scan().listen(
      (device) {
        found[device.id] = device;
        if (!_disposed) notifyListeners();
      },
      onError: (Object error) {
        if (!chosen.isCompleted) {
          chosen.completeError(_Failed(PairingFailure.link, '$error'));
        }
      },
    );
    final nothing = Timer(SetupLink.findTimeout, () {
      if (found.isEmpty && !chosen.isCompleted) {
        chosen.completeError(TimeoutException(null));
      }
    });
    try {
      return await chosen.future;
    } finally {
      nothing.cancel();
      _chosen = null;
      await scanning.cancel();
    }
  }

  /// Connect to `device`, one of [found].
  void choose(DiscoveredDevice device) {
    final chosen = _chosen;
    if (chosen != null && !chosen.isCompleted) chosen.complete(device);
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
          case SetupStep_Failed(:final error):
            _fail(_failureOf(error), error.message);
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
      case SetupStep_Done(
        enrolled: final how,
        :final wifi,
        feeder: final identity,
      ):
        enrolled = how;
        wifiSaved = wifi;
        final hex = _ports.hexOf(identity);
        _set(PairingPhase.joining);

        // After BLE setup the feeder restarts onto its new Wi-Fi first.
        final address = knownAddress ?? await _ports.locate(hex, rejoinWait);
        final known = KnownFeeder(
          identity: hex,
          name: feederName,
          address: address,
        );
        // The feeder enrolled this phone whatever happens here; a failed
        // save is said so, not left on "joining".
        try {
          await _app.remember(known, paired: true);
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
      case SetupStep_Failed(:final error):
        _stop(_failureOf(error), error.message);
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
    // Taken first: the screen closing and setup ending may both get here.
    final link = _link;
    _link = null;
    unawaited(_notifications?.cancel());
    _notifications = null;
    await link?.close();
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
    final choosing = _chosen;
    if (choosing != null && !choosing.isCompleted) {
      choosing.completeError(
        _Failed(PairingFailure.other, 'the screen closed'),
      );
    }
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
