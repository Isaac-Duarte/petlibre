import '../src/rust/api/keys.dart';
import '../src/rust/api/session.dart';
import '../src/rust/api/store.dart';
import 'discovery.dart';

/// What a feeder's controller reaches outside Dart: the Rust session and
/// NSD. Behind this, so its screens are tested with no Rust and no network.
abstract interface class FeederPorts {
  /// A session with the feeder `identity` (hex) at `address`, not yet run.
  FeederSession open(
    String identity,
    String address, {
    required bool syncClock,
  });

  /// Where the feeder answers, within `wait`.
  Future<String?> locate(String identity, Duration wait);
}

class AppFeederPorts implements FeederPorts {
  const AppFeederPorts(this._key);

  final ClientKey _key;

  @override
  FeederSession open(
    String identity,
    String address, {
    required bool syncClock,
  }) => FeederSession(
    key: _key,
    feeder: identityBytes(hex: identity),
    address: address,
    syncClock: syncClock,
  );

  @override
  Future<String?> locate(String identity, Duration wait) =>
      Discovery.locate(identity, wait);
}
