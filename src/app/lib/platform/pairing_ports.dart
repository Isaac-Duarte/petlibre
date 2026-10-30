import 'dart:async';

import 'package:flutter_reactive_ble/flutter_reactive_ble.dart'
    show DiscoveredDevice;

import '../src/rust/api/keys.dart';
import '../src/rust/api/setup.dart';
import '../src/rust/api/store.dart';
import 'discovery.dart';
import 'setup_link.dart';

/// What a pairing reaches outside Dart: the Rust session, the radio and NSD.
/// Behind this, so its screens are tested with no Rust and no radio.
abstract interface class PairingPorts {
  SetupDriver newDriver(String label, WifiCredentials? wifi);

  Recovery newRecovery(String label);

  /// Every advertisement of the setup service until cancelled.
  Stream<DiscoveredDevice> scan();

  Future<BleLink> open(DiscoveredDevice device);

  /// Where the feeder with `identity` (hex) answers, within `wait`.
  Future<String?> locate(String identity, Duration wait);

  /// A key as the store and NSD's `id` spell it.
  String hexOf(List<int> identity);
}

class AppPairingPorts implements PairingPorts {
  const AppPairingPorts(this._key);

  final ClientKey _key;

  @override
  SetupDriver newDriver(String label, WifiCredentials? wifi) =>
      SetupDriver(key: _key, label: label, wifi: wifi);

  @override
  Recovery newRecovery(String label) => Recovery(key: _key, label: label);

  @override
  Stream<DiscoveredDevice> scan() => SetupLink.scan();

  @override
  Future<BleLink> open(DiscoveredDevice device) => SetupLink.open(device);

  @override
  Future<String?> locate(String identity, Duration wait) =>
      Discovery.locate(identity, wait);

  @override
  String hexOf(List<int> identity) => identityHex(identity: identity);
}
