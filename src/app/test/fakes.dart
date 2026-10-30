import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_reactive_ble/flutter_reactive_ble.dart'
    show DiscoveredDevice, Uuid;
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustOpaqueInterface;
import 'package:flutter_test/flutter_test.dart' show WidgetTester;
import 'package:wellfed/platform/install_ports.dart';
import 'package:wellfed/platform/pairing_ports.dart';
import 'package:wellfed/platform/setup_link.dart';
import 'package:wellfed/src/rust/api/install.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/setup.dart';
import 'package:wellfed/src/rust/api/store.dart';
import 'package:wellfed/state/app_state.dart';
import 'package:wellfed/theme/theme.dart';

/// Pump past transitions. Not `pumpAndSettle`: a screen waiting on the feeder
/// shows a spinner, which never settles.
Future<void> settle(WidgetTester tester) async {
  for (var frame = 0; frame < 8; frame++) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}

Widget themed(Widget child, {Brightness brightness = Brightness.light}) =>
    MaterialApp(theme: wellfedTheme(brightness), home: child);

mixin _FakeOpaque implements RustOpaqueInterface {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

/// The feeders the app knows, in memory.
class FakeBook extends ChangeNotifier implements FeederBook {
  FakeBook({this.label = 'Pixel'});

  @override
  String label;

  @override
  List<KnownFeeder> feeders = [];

  final remembered = <(KnownFeeder, bool)>[];

  /// What the next save throws, as a full or read-only disk would.
  Object? saveFails;

  @override
  Future<void> setLabel(String label) async {
    if (saveFails case final error?) throw error;
    this.label = label;
    notifyListeners();
  }

  @override
  Future<void> remember(
    KnownFeeder feeder, {
    bool paired = false,
    bool keepName = false,
  }) async {
    if (saveFails case final error?) throw error;
    remembered.add((feeder, paired));
    final known = feeders.where((f) => f.identity == feeder.identity);
    feeders = [
      for (final other in feeders)
        if (other.identity != feeder.identity) other,
      KnownFeeder(
        identity: feeder.identity,
        name: keepName && known.isNotEmpty ? known.first.name : feeder.name,
        address: feeder.address,
      ),
    ];
    notifyListeners();
  }

  @override
  Future<void> answeredAt(String identity, String address) async {}
}

/// A run the test drives: its updates are the test's, its answers recorded.
class FakeRun with _FakeOpaque implements InstallRun {
  // ignore: close_sinks, the test ends with it
  final updates = StreamController<InstallUpdate>();
  final answers = <int, InstallAnswer>{};
  final notifications = <List<int>>[];
  int lost = 0;
  int stops = 0;

  @override
  Stream<InstallUpdate> run() => updates.stream;

  @override
  void answer({required int id, required InstallAnswer answer}) =>
      answers[id] = answer;

  @override
  void linkLost() => lost++;

  @override
  void notified({required List<int> bytes}) => notifications.add(bytes);

  @override
  void stop() => stops++;

  void event(InstallEvent event) =>
      updates.add(InstallUpdate.event(event: event));

  void ask(int id, InstallAsk ask) =>
      updates.add(InstallUpdate.ask(id: id, ask: ask));

  void end(InstallEnd end) => updates.add(InstallUpdate.ended(end: end));

  void stopped(
    StopCase stop, {
    Resume resume = Resume.nothing,
    String? detail,
  }) => end(
    InstallEnd.stopped(case_: stop, resume: resume, detail: detail ?? ''),
  );
}

class FakeListener with _FakeOpaque implements InstallListener {
  // ignore: close_sinks, the test ends with it
  final updates = StreamController<ListenUpdate>();
  int stops = 0;

  @override
  Stream<ListenUpdate> run() => updates.stream;

  @override
  void stop() => stops++;
}

class FakeLink implements InstallLink {
  final written = <List<List<int>>>[];
  int closes = 0;

  @override
  Future<void> write(List<List<int>> pieces) async => written.add(pieces);

  @override
  Future<void> close() async => closes++;
}

/// The install's outside world, with no Rust and no phone.
class FakeInstallPorts implements InstallPorts {
  FakeInstallPorts({
    this.image = const ImageCheck.ready(
      provisioning: ImageProvisioning.consumer,
      build: 'ab',
      version: 7,
    ),
    this.build = const BuildCheck.reviewable(),
    this.dns = const DnsResult.points(),
    this.ip = '192.168.1.23',
    this.pendingInstall = PendingInstall.nothing,
  });

  ImageCheck image;
  BuildCheck build;
  DnsResult dns;
  String? ip;
  PendingInstall pendingInstall;
  bool holds = true;
  String? kept;
  bool allowed = true;
  int asked = 0;

  final specs = <InstallSpec>[];
  final runs = <FakeRun>[];
  final listeners = <FakeListener>[];
  final dnsAsked = <String>[];
  final held = <String>[];
  int releases = 0;
  final shared = <String>[];
  // ignore: close_sinks, the test ends with it
  final heard = StreamController<void>.broadcast();
  final link = FakeLink();
  void Function(List<int>)? onBytes;
  void Function()? onLost;
  Object? linkFails;
  String? located = '192.168.1.40:4747';

  /// The latest run.
  FakeRun get run => runs.last;

  @override
  Future<InstallPaths> paths() async =>
      const InstallPaths(store: 'store', image: 'image', data: 'data');

  @override
  Future<ImageCheck> prepareImage(InstallPaths paths) async => image;

  @override
  BuildCheck checkBuild() => build;

  @override
  PendingInstall pending(InstallPaths paths) => pendingInstall;

  @override
  String? keptBackup(InstallPaths paths, List<int> identity) => kept;

  @override
  Future<String?> phoneAddress() async => ip;

  @override
  Future<DnsResult> checkDns(String name, String phoneIp) async {
    dnsAsked.add(name);
    return dns;
  }

  @override
  Future<bool> askPermissions({required bool bluetooth}) async {
    asked++;
    return allowed;
  }

  @override
  InstallRun newRun(InstallSpec spec) {
    specs.add(spec);
    final run = FakeRun();
    runs.add(run);
    return run;
  }

  @override
  InstallListener newListener(InstallPaths paths) {
    final listener = FakeListener();
    listeners.add(listener);
    return listener;
  }

  @override
  Duration get listenWait => const Duration(minutes: 20);

  @override
  Future<InstallLink> openLink({
    required void Function(List<int> bytes) onBytes,
    required void Function() onLost,
  }) async {
    if (linkFails case final error?) throw error;
    this.onBytes = onBytes;
    this.onLost = onLost;
    return link;
  }

  @override
  Stream<void> setupHeard() => heard.stream;

  @override
  Future<String?> locate(String identity) async => located;

  @override
  Future<bool> holdProcess(String text) async {
    held.add(text);
    return holds;
  }

  @override
  Future<void> releaseProcess() async => releases++;

  @override
  Future<void> shareBackup(String path) async => shared.add(path);
}

/// A BLE setup session the test scripts.
class FakeDriver with _FakeOpaque implements SetupDriver {
  FakeDriver(this.positions);

  final List<int> positions;

  /// What the next notification comes to.
  List<SetupStep> script = const [];

  @override
  Uint8List code() => Uint8List.fromList(positions);

  @override
  List<Uint8List> start() => [
    Uint8List.fromList([1]),
  ];

  @override
  List<SetupStep> notified({required List<int> bytes}) => script;
}

/// A recovery the test scripts, as its stream would.
class FakeRecovery with _FakeOpaque implements Recovery {
  // ignore: close_sinks, the test ends with it
  final steps = StreamController<SetupStep>();
  int closes = 0;

  @override
  void close() => closes++;

  @override
  Uint8List code() => Uint8List.fromList(const [1, 2, 3, 4, 1, 2]);

  @override
  Stream<SetupStep> run({required String address}) => steps.stream;
}

class FakeBleLink implements BleLink {
  // ignore: close_sinks, the test ends with it
  // ignore: close_sinks, the test ends with it
  final bytes = StreamController<List<int>>(onCancel: () async {});
  final written = <List<List<int>>>[];
  int closes = 0;

  @override
  Stream<List<int>> get notifications => bytes.stream;

  @override
  Future<void> write(List<List<int>> pieces) async => written.add(pieces);

  @override
  Future<void> close() async => closes++;
}

DiscoveredDevice feederNearby(String id, {int rssi = -50}) => DiscoveredDevice(
  id: id,
  name: '',
  serviceData: const {},
  manufacturerData: Uint8List(0),
  rssi: rssi,
  serviceUuids: <Uuid>[],
);

/// Pairing's outside world, with no Rust and no radio.
class FakePairingPorts implements PairingPorts {
  final drivers = <FakeDriver>[];
  final recoveries = <FakeRecovery>[];
  // ignore: close_sinks, the test ends with it
  // ignore: close_sinks, the test ends with it
  final scans = StreamController<DiscoveredDevice>(onCancel: () async {});
  final opened = <DiscoveredDevice>[];
  FakeBleLink link = FakeBleLink();
  String? located = '192.168.1.40:4747';
  int nextCode = 1;

  @override
  SetupDriver newDriver(String label, WifiCredentials? wifi) {
    final code = [nextCode, 2, 3, 4, 1, 2];
    nextCode++;
    final driver = FakeDriver(code);
    drivers.add(driver);
    return driver;
  }

  @override
  Recovery newRecovery(String label) {
    final recovery = FakeRecovery();
    recoveries.add(recovery);
    return recovery;
  }

  @override
  Stream<DiscoveredDevice> scan() => scans.stream;

  @override
  Future<BleLink> open(DiscoveredDevice device) async {
    opened.add(device);
    link = FakeBleLink();
    return link;
  }

  @override
  Future<String?> locate(String identity, Duration wait) async => located;

  @override
  String hexOf(List<int> identity) =>
      identity.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
}

final doneStep = SetupStep.done(
  enrolled: Enrolled.new_,
  wifi: WifiSaved.saved,
  feeder: Uint8List.fromList(const [0xab, 0xcd]),
);
