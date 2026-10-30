import 'dart:io';
import 'dart:typed_data';

import 'package:file_selector/file_selector.dart';

import '../src/rust/api/install.dart' as install;
import '../src/rust/api/keys.dart';
import '../src/rust/api/restore.dart';
import '../src/rust/api/restore.dart' as rust;
import 'install_ports.dart';

/// What a restore reaches outside Dart: the Rust run, NSD, the foreground
/// service, the kept backup and the file picker. Behind this, so every screen
/// is tested with no Rust and no phone.
abstract interface class RestorePorts {
  /// The installer's own folder.
  Future<String> dataDir();

  /// The checked copy of stock kept for the feeder with `identity`.
  String? keptBackup(String dataDir, List<int> identity);

  /// Whether the file at `path` is a copy of stock this app can put back.
  BackupCheck checkBackup(String path);

  /// When the file at `path` was written.
  Future<DateTime?> backupDate(String path);

  /// A file the owner picks, as a path this app can read; `null` when they
  /// picked none.
  Future<String?> pickBackup();

  RestoreRun newRun(RestoreSpec spec);

  Future<String?> locate(String identity);

  /// Each sighting of a feeder advertising Bluetooth setup.
  Stream<void> setupHeard();

  /// Forget the restore's key for the feeder with `identity`.
  void forgetKey(String dataDir, List<int> identity);

  /// Keep this process alive with a notification while the firmware goes
  /// across. `false` when Android refused.
  Future<bool> holdProcess(String text);

  Future<void> releaseProcess();
}

class AppRestorePorts implements RestorePorts {
  AppRestorePorts(this._key) : _install = AppInstallPorts(_key);

  final ClientKey _key;

  /// The install's folders, NSD and foreground service are a restore's too.
  final AppInstallPorts _install;

  @override
  Future<String> dataDir() async => (await _install.paths()).data;

  @override
  String? keptBackup(String dataDir, List<int> identity) =>
      install.keptBackup(dataDir: dataDir, identity: identity);

  @override
  BackupCheck checkBackup(String path) => rust.checkBackup(path: path);

  @override
  Future<DateTime?> backupDate(String path) async {
    final file = File(path);
    return file.existsSync() ? file.lastModified() : null;
  }

  @override
  Future<String?> pickBackup() async => (await openFile())?.path;

  @override
  RestoreRun newRun(RestoreSpec spec) => RestoreRun(key: _key, spec: spec);

  @override
  Future<String?> locate(String identity) => _install.locate(identity);

  @override
  Stream<void> setupHeard() => _install.setupHeard();

  @override
  void forgetKey(String dataDir, List<int> identity) => rust.forgetRestoreKey(
    dataDir: dataDir,
    identity: Uint8List.fromList(identity),
  );

  @override
  Future<bool> holdProcess(String text) => _install.holdProcess(text);

  @override
  Future<void> releaseProcess() => _install.releaseProcess();
}
