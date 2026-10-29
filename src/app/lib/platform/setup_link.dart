import 'dart:async';

import 'package:flutter_reactive_ble/flutter_reactive_ble.dart';

import '../src/rust/api/setup.dart';

/// The radio half of BLE setup: find a feeder in setup mode, connect, and
/// carry the setup session's bytes over its control characteristic. What
/// the bytes mean is the Rust session's business.
class SetupLink {
  SetupLink._(this._ble, this._connection, this._control, this.notifications);

  static const findTimeout = Duration(seconds: 30);
  static const connectTimeout = Duration(seconds: 15);

  final FlutterReactiveBle _ble;
  final StreamSubscription<ConnectionStateUpdate> _connection;
  final QualifiedCharacteristic _control;

  /// Each notification's bytes, in order; the session reassembles frames.
  final Stream<List<int>> notifications;

  /// The first feeder advertising the setup service now.
  static Future<DiscoveredDevice> find() {
    final ble = FlutterReactiveBle();
    return ble
        .scanForDevices(
          withServices: [Uuid.parse(setupService())],
          scanMode: ScanMode.lowLatency,
          // BLUETOOTH_SCAN is declared neverForLocation (Android 12+).
          requireLocationServicesEnabled: false,
        )
        .first
        .timeout(findTimeout);
  }

  /// Connect to `device`, then enable notifications before anything is
  /// written: the feeder answers the first message at once.
  static Future<SetupLink> open(DiscoveredDevice device) async {
    final ble = FlutterReactiveBle();
    final connected = Completer<void>();
    final connection = ble
        .connectToDevice(
          id: device.id,
          servicesWithCharacteristicsToDiscover: {
            Uuid.parse(setupService()): [Uuid.parse(setupControl())],
          },
          connectionTimeout: connectTimeout,
        )
        .listen(
          (update) {
            if (update.connectionState == DeviceConnectionState.connected &&
                !connected.isCompleted) {
              connected.complete();
            }
            if (update.connectionState == DeviceConnectionState.disconnected &&
                !connected.isCompleted) {
              connected.completeError(
                update.failure?.message ?? 'the feeder did not connect',
              );
            }
          },
          onError: (Object error) {
            if (!connected.isCompleted) connected.completeError(error);
          },
        );
    try {
      await connected.future.timeout(connectTimeout);
      // Writes never depend on it (the pieces fit the smallest MTU), but a
      // larger one carries notifications in fewer packets.
      await ble.requestMtu(deviceId: device.id, mtu: 247);
    } catch (_) {
      await connection.cancel();
      rethrow;
    }

    final control = QualifiedCharacteristic(
      serviceId: Uuid.parse(setupService()),
      characteristicId: Uuid.parse(setupControl()),
      deviceId: device.id,
    );
    final notifications = ble
        .subscribeToCharacteristic(control)
        .asBroadcastStream();
    return SetupLink._(ble, connection, control, notifications);
  }

  /// Write `pieces` in order, each acknowledged before the next.
  Future<void> write(List<List<int>> pieces) async {
    for (final piece in pieces) {
      await _ble
          .writeCharacteristicWithResponse(_control, value: piece)
          .timeout(connectTimeout);
    }
  }

  Future<void> close() => _connection.cancel();
}
