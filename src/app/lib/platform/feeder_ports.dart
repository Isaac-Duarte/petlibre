import '../src/rust/api/install.dart';
import '../src/rust/api/keys.dart';
import '../src/rust/api/session.dart';
import '../src/rust/api/store.dart';
import '../src/rust/api/update.dart';
import 'discovery.dart';
import 'install_ports.dart';

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

  /// The image this APK carries, unpacked and checked against its pin.
  Future<ImageCheck> updateImage();

  /// Offer that image to the feeder once, and report until the update ends.
  Stream<UpdateProgress> pushUpdate(String identity, String address);
}

class AppFeederPorts implements FeederPorts {
  AppFeederPorts(this._key);

  final ClientKey _key;

  /// The unpacking and paths the install uses: one image, one place.
  late final _install = AppInstallPorts(_key);

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

  @override
  Future<ImageCheck> updateImage() async =>
      _install.prepareImage(await _install.paths());

  @override
  Stream<UpdateProgress> pushUpdate(String identity, String address) async* {
    final paths = await _install.paths();
    yield* FirmwareUpdate(
      key: _key,
      feeder: identityBytes(hex: identity),
      address: address,
      imageDir: paths.image,
    ).run();
  }
}
