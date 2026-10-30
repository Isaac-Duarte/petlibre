import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart' show FlutterError;
import 'package:flutter/services.dart' show ByteData, rootBundle;
import 'package:flutter_foreground_task/flutter_foreground_task.dart';
import 'package:network_info_plus/network_info_plus.dart';
import 'package:path_provider/path_provider.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:share_plus/share_plus.dart';

import '../src/rust/api/install.dart';
import '../src/rust/api/install.dart' as rust;
import '../src/rust/api/keys.dart';
import 'discovery.dart';
import 'setup_link.dart';

/// Where the installer's files live on this phone.
class InstallPaths {
  const InstallPaths({
    required this.store,
    required this.image,
    required this.data,
  });

  /// The app's own file.
  final String store;

  /// Where the APK's image is unpacked.
  final String image;

  /// The stock server's config and records, and the backups.
  final String data;
}

/// A connected feeder's control characteristic, as the install's run writes
/// it. Its notifications go to the callbacks [InstallPorts.openLink] took.
abstract interface class InstallLink {
  Future<void> write(List<List<int>> pieces);

  Future<void> close();
}

/// What an install reaches outside Dart: the Rust run, the radio, NSD,
/// Android's permissions and foreground service, the APK's image and the
/// share sheet. Behind this, so every screen is tested with no Rust and no
/// phone.
abstract interface class InstallPorts {
  Future<InstallPaths> paths();

  /// Unpack the APK's image, and check it against this build's pin.
  Future<ImageCheck> prepareImage(InstallPaths paths);

  BuildCheck checkBuild();

  /// What an earlier run left on the feeder.
  PendingInstall pending(InstallPaths paths);

  /// The checked copy of stock kept for the feeder with `identity`.
  String? keptBackup(InstallPaths paths, List<int> identity);

  /// This phone's address on the Wi-Fi, or `null` when it is on none.
  Future<String?> phoneAddress();

  Future<DnsResult> checkDns(String name, String phoneIp);

  /// Ask Android for what the install needs; whether Bluetooth was allowed.
  /// `bluetooth` is `false` for a bench image, which pairs over the LAN.
  Future<bool> askPermissions({required bool bluetooth});

  InstallRun newRun(InstallSpec spec);

  InstallListener newListener(InstallPaths paths);

  /// How long a listener listens.
  Duration get listenWait;

  /// Find the first feeder advertising Bluetooth setup and connect to it,
  /// notifications on. Throws when none is heard in time.
  Future<InstallLink> openLink({
    required void Function(List<int> bytes) onBytes,
    required void Function() onLost,
  });

  /// Whether a feeder is advertising Bluetooth setup: the replacement is
  /// running, and waiting for its owner. Each sighting is an event.
  Stream<void> setupHeard();

  Future<String?> locate(String identity);

  /// Keep this process alive with a notification, for as long as a run or a
  /// listener needs stock's server on this phone. `false` when Android
  /// refused, so the app's being on screen is all that keeps it alive.
  Future<bool> holdProcess(String text);

  Future<void> releaseProcess();

  /// Hand the kept stock backup at `path` to the owner, to save elsewhere.
  Future<void> shareBackup(String path);
}

class AppInstallPorts implements InstallPorts {
  const AppInstallPorts(this._key);

  final ClientKey _key;

  /// The image's files, as `make app-installer-input` fills them in.
  static const _imageAssets = [
    'review.json',
    'build-manifest.json',
    'image.bin',
    'update.manifest',
  ];

  static const _locateWait = Duration(seconds: 12);

  @override
  Future<InstallPaths> paths() async {
    final support = await getApplicationSupportDirectory();
    return InstallPaths(
      store: '${support.path}/wellfed.json',
      image: '${support.path}/install/image',
      data: '${support.path}/install/stock',
    );
  }

  @override
  Future<ImageCheck> prepareImage(InstallPaths paths) async {
    final folder = Directory(paths.image);
    // Whatever an earlier build unpacked is not this build's.
    if (folder.existsSync()) folder.deleteSync(recursive: true);
    folder.createSync(recursive: true);
    for (final name in _imageAssets) {
      final ByteData data;
      try {
        data = await rootBundle.load('assets/installer/$name');
      } on FlutterError {
        return const ImageCheck.absent();
      }
      final bytes = data.buffer.asUint8List(
        data.offsetInBytes,
        data.lengthInBytes,
      );
      await File('${paths.image}/$name').writeAsBytes(bytes);
    }
    return checkImage(dir: paths.image);
  }

  @override
  BuildCheck checkBuild() => rust.checkBuild();

  @override
  PendingInstall pending(InstallPaths paths) =>
      pendingInstall(dataDir: paths.data);

  @override
  String? keptBackup(InstallPaths paths, List<int> identity) =>
      rust.keptBackup(dataDir: paths.data, identity: identity);

  @override
  Future<String?> phoneAddress() => NetworkInfo().getWifiIP();

  @override
  Future<DnsResult> checkDns(String name, String phoneIp) =>
      rust.checkDns(name: name, phoneIp: phoneIp);

  @override
  Future<bool> askPermissions({required bool bluetooth}) async {
    final granted = await [
      if (bluetooth) ...[Permission.bluetoothScan, Permission.bluetoothConnect],
      // Only to read the Wi-Fi's name for the prefill, and to show the
      // install's notification; the install goes on without either.
      Permission.locationWhenInUse,
      Permission.notification,
    ].request();
    return !bluetooth ||
        (granted[Permission.bluetoothScan]?.isGranted == true &&
            granted[Permission.bluetoothConnect]?.isGranted == true);
  }

  @override
  InstallRun newRun(InstallSpec spec) => InstallRun(key: _key, spec: spec);

  @override
  InstallListener newListener(InstallPaths paths) =>
      InstallListener(dataDir: paths.data);

  @override
  Duration get listenWait => Duration(seconds: InstallListener.waitSeconds());

  @override
  Future<InstallLink> openLink({
    required void Function(List<int> bytes) onBytes,
    required void Function() onLost,
  }) async {
    final device = await SetupLink.scan().first.timeout(SetupLink.findTimeout);
    final link = await SetupLink.open(device);
    return _Link(
      link,
      link.notifications.listen(
        onBytes,
        onError: (Object _) => onLost(),
        onDone: onLost,
      ),
    );
  }

  @override
  Stream<void> setupHeard() => SetupLink.scan().map((_) {});

  @override
  Future<String?> locate(String identity) =>
      Discovery.locate(identity, _locateWait);

  @override
  Future<bool> holdProcess(String text) async {
    if (await FlutterForegroundTask.isRunningService) {
      final updated = await FlutterForegroundTask.updateService(
        notificationTitle: _notificationTitle,
        notificationText: text,
      );
      return updated is ServiceRequestSuccess;
    }
    FlutterForegroundTask.init(
      androidNotificationOptions: AndroidNotificationOptions(
        channelId: 'install',
        channelName: 'Installing Wellfed',
        channelDescription:
            'Shown while this phone serves a feeder its firmware.',
        onlyAlertOnce: true,
      ),
      iosNotificationOptions: const IOSNotificationOptions(),
      foregroundTaskOptions: ForegroundTaskOptions(
        eventAction: ForegroundTaskEventAction.nothing(),
        allowWakeLock: true,
        allowWifiLock: true,
        allowAutoRestart: false,
        // A run lives in this process: swiping the app away ends it, and
        // nothing would be left for the notification to speak for.
        stopWithTask: true,
      ),
    );
    final started = await FlutterForegroundTask.startService(
      notificationTitle: _notificationTitle,
      notificationText: text,
    );
    return started is ServiceRequestSuccess;
  }

  static const _notificationTitle = 'Wellfed is installing on a feeder';

  @override
  Future<void> releaseProcess() async {
    await FlutterForegroundTask.stopService();
  }

  @override
  Future<void> shareBackup(String path) async {
    await SharePlus.instance.share(
      ShareParams(
        files: [XFile(path, mimeType: 'application/octet-stream')],
        subject: "The feeder's original firmware",
      ),
    );
  }
}

class _Link implements InstallLink {
  _Link(this._link, this._notifications);

  final BleLink _link;
  final StreamSubscription<List<int>> _notifications;

  @override
  Future<void> write(List<List<int>> pieces) => _link.write(pieces);

  @override
  Future<void> close() async {
    await _notifications.cancel();
    await _link.close();
  }
}
