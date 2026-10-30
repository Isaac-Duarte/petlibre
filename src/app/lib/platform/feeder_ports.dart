import '../src/rust/api/install.dart';
import '../src/rust/api/keys.dart';
import '../src/rust/api/session.dart';
import '../src/rust/api/store.dart';
import '../src/rust/api/update.dart';
import 'discovery.dart';
import 'install_ports.dart';
import 'restore_ports.dart';

/// One running update: what it reports, and where it is told the feeder is.
abstract interface class UpdatePush {
  Stream<UpdateProgress> get progress;

  /// Answer the run's ask `id` with where the feeder answers.
  void located(int id, String? address);
}

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

  /// One update of the feeder to that image, which goes once.
  UpdatePush pushUpdate(String identity, String? address);

  /// Keep this process alive with a notification while the firmware goes
  /// across. `false` when Android refused.
  Future<bool> holdProcess(String text);

  Future<void> releaseProcess();

  /// What the restore screen reaches outside Dart.
  RestorePorts get restore;
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
  UpdatePush pushUpdate(String identity, String? address) =>
      _AppPush(_key, _install, identity, address);

  @override
  Future<bool> holdProcess(String text) => _install.holdProcess(text);

  @override
  Future<void> releaseProcess() => _install.releaseProcess();

  @override
  late final RestorePorts restore = AppRestorePorts(_key);
}

class _AppPush implements UpdatePush {
  _AppPush(this._key, this._install, this._identity, this._address);

  final ClientKey _key;
  final AppInstallPorts _install;
  final String _identity;
  final String? _address;
  FirmwareUpdate? _run;

  @override
  Stream<UpdateProgress> get progress async* {
    final paths = await _install.paths();
    final run = FirmwareUpdate(
      key: _key,
      feeder: identityBytes(hex: _identity),
      address: _address,
      imageDir: paths.image,
    );
    _run = run;
    yield* run.run();
  }

  @override
  void located(int id, String? address) => _run?.answer(
    id: id,
    answer: InstallAnswer.located(address: address),
  );
}
