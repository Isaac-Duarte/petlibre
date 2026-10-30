import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show RustOpaqueInterface;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/platform/restore_ports.dart';
import 'package:wellfed/screens/restore_screen.dart';
import 'package:wellfed/src/rust/api/install.dart';
import 'package:wellfed/src/rust/api/restore.dart';
import 'package:wellfed/state/restore.dart';

import 'fakes.dart';

/// A run the test drives.
class FakeRestoreRun implements RestoreRun, RustOpaqueInterface {
  // ignore: close_sinks, the test ends with it
  final updates = StreamController<RestoreUpdate>();
  final answers = <int, InstallAnswer>{};
  int stops = 0;

  @override
  Stream<RestoreUpdate> run() => updates.stream;

  @override
  void answer({required int id, required InstallAnswer answer}) =>
      answers[id] = answer;

  @override
  void stop() => stops++;

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;

  void event(InstallEvent event) =>
      updates.add(RestoreUpdate.event(event: event));

  void end(RestoreEnd end) => updates.add(RestoreUpdate.ended(end: end));

  void stopped(RestoreStop stop) =>
      end(RestoreEnd.stopped(case_: stop, detail: 'small print'));
}

class FakeRestorePorts implements RestorePorts {
  String? kept = '/kept/stock-1.1.15.bin';
  final checks = <String, BackupCheck>{
    '/kept/stock-1.1.15.bin': const BackupCheck.ready(version: '1.1.15'),
  };
  String? picks;
  bool holds = true;
  int released = 0;
  final held = <String>[];
  final specs = <RestoreSpec>[];
  final runs = <FakeRestoreRun>[];

  @override
  Future<String> dataDir() async => '/data';

  @override
  String? keptBackup(String dataDir, List<int> identity) => kept;

  @override
  BackupCheck checkBackup(String path) =>
      checks[path] ?? const BackupCheck.missing();

  @override
  Future<DateTime?> backupDate(String path) async => DateTime(2026, 10, 28);

  @override
  Future<String?> pickBackup() async => picks;

  @override
  RestoreRun newRun(RestoreSpec spec) {
    specs.add(spec);
    final run = FakeRestoreRun();
    runs.add(run);
    return run;
  }

  @override
  Future<String?> locate(String identity) async => '10.0.0.9:7700';

  @override
  Future<bool> holdProcess(String text) async {
    held.add(text);
    return holds;
  }

  @override
  Future<void> releaseProcess() async => released++;
}

final _flowIdentity = List<int>.filled(32, 0xab);

RestoreFlow _flow(FakeRestorePorts ports) => RestoreFlow(
  ports,
  identity: _flowIdentity,
  feederName: 'Kitchen',
  address: '10.0.0.5:7700',
);

/// The screen's stages, drawn from the flow itself: no Rust behind
/// [identityBytes], which [RestoreScreen] calls.
class _Harness extends StatelessWidget {
  const _Harness(this.flow, {this.brightness = Brightness.light});

  final RestoreFlow flow;
  final Brightness brightness;

  @override
  Widget build(BuildContext context) =>
      themed(RestoreView(flow: flow), brightness: brightness);
}

Future<RestoreFlow> _open(
  WidgetTester tester,
  FakeRestorePorts ports, {
  Brightness brightness = Brightness.light,
}) async {
  tester.view.physicalSize = const Size(1236, 2745);
  tester.view.devicePixelRatio = 3;
  addTearDown(tester.view.reset);
  final flow = _flow(ports);
  await flow.load();
  await tester.pumpWidget(_Harness(flow, brightness: brightness));
  await settle(tester);
  return flow;
}

Future<FakeRestoreRun> _started(
  WidgetTester tester,
  FakeRestorePorts ports,
) async {
  await tester.tap(find.text('I understand'));
  await settle(tester);
  await tester.tap(
    find.widgetWithText(FilledButton, 'Restore original firmware'),
  );
  await settle(tester);
  return ports.runs.last;
}

void main() {
  testWidgets('the facts and warnings hold the button until the owner agrees', (
    tester,
  ) async {
    final ports = FakeRestorePorts();
    await _open(tester, ports);

    expect(find.text('28 Oct, on this phone'), findsOneWidget);
    expect(find.text('Vendor 1.1.15'), findsOneWidget);
    expect(find.text('Matches the original'), findsOneWidget);
    expect(find.textContaining("don't carry over"), findsOneWidget);
    expect(find.textContaining("can't talk to Kitchen"), findsOneWidget);

    await tester.tap(
      find.widgetWithText(FilledButton, 'Restore original firmware'),
    );
    await settle(tester);
    expect(ports.runs, isEmpty, reason: 'nothing goes before the checkbox');

    final run = await _started(tester, ports);
    expect(ports.specs.single.backup, '/kept/stock-1.1.15.bin');
    expect(ports.specs.single.address, '10.0.0.5:7700');
    expect(ports.specs.single.dataDir, '/data');
    expect(ports.held, isNotEmpty);
    run.stopped(RestoreStop.cancelled);
    await settle(tester);
  });

  testWidgets('the steps follow the run: press, percent, restart, done', (
    tester,
  ) async {
    final ports = FakeRestorePorts();
    final flow = await _open(tester, ports);
    final run = await _started(tester, ports);

    run.event(const InstallEvent.pressDispense());
    await settle(tester);
    expect(
      find.textContaining(
        'Press Dispense on the feeder, once.',
        findRichText: true,
      ),
      findsOneWidget,
    );
    expect(flow.committed, isFalse, reason: 'leaving is still a clean stop');

    run.event(const InstallEvent.promoting());
    run.event(const InstallEvent.written(written: 31, length: 50));
    await settle(tester);
    expect(
      find.textContaining(
        'Press Dispense on the feeder, once.',
        findRichText: true,
      ),
      findsNothing,
    );
    expect(find.text('62%'), findsOneWidget);
    expect(
      flow.committed,
      isTrue,
      reason: 'no way back once an offer may be out',
    );

    run.event(const InstallEvent.activated());
    run.end(const RestoreEnd.activated());
    await settle(tester);
    expect(find.text('The original firmware is going back'), findsOneWidget);
    expect(
      find.textContaining('Hold Reset for about 5 seconds'),
      findsOneWidget,
    );
    expect(find.textContaining('once the DNS record is gone'), findsOneWidget);
    expect(ports.released, 1);
  });

  testWidgets('an uncertain end offers no retry', (tester) async {
    final ports = FakeRestorePorts();
    final flow = await _open(tester, ports);
    final run = await _started(tester, ports);

    run.event(const InstallEvent.promoting());
    run.end(const RestoreEnd.uncertain(detail: 'gone'));
    await settle(tester);

    expect(find.text("We can't tell if it went through"), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
    expect(ports.runs, hasLength(1));
    expect(flow.mayRetry, isFalse);
  });

  testWidgets(
    'a refusal offers no retry; a missed press asks for the word again',
    (tester) async {
      final ports = FakeRestorePorts();
      await _open(tester, ports);
      var run = await _started(tester, ports);
      run.stopped(RestoreStop.refused);
      await settle(tester);
      expect(find.text('Kitchen said no'), findsOneWidget);
      expect(find.text('Try again'), findsNothing);

      final again = FakeRestorePorts();
      await _open(tester, again);
      run = await _started(tester, again);
      run.stopped(RestoreStop.noPress);
      await settle(tester);
      expect(find.text('Dispense was not pressed in time'), findsOneWidget);

      await tester.tap(find.text('Try again'));
      await settle(tester);
      expect(find.text('I understand'), findsOneWidget);
      await tester.tap(
        find.widgetWithText(FilledButton, 'Restore original firmware'),
      );
      await settle(tester);
      expect(
        again.runs,
        hasLength(1),
        reason: 'not without the checkbox again',
      );
      final second = await _started(tester, again);
      expect(again.runs, hasLength(2));
      second.stopped(RestoreStop.cancelled);
      await settle(tester);
    },
  );

  testWidgets('a stream that closes with no end is uncertain once offering', (
    tester,
  ) async {
    final ports = FakeRestorePorts();
    final flow = await _open(tester, ports);
    final run = await _started(tester, ports);
    run.event(const InstallEvent.promoting());
    await settle(tester);

    await run.updates.close();
    await settle(tester);

    expect(flow.stage, RestoreStage.uncertain);
  });

  testWidgets('the run asks only where the feeder is', (tester) async {
    final ports = FakeRestorePorts();
    await _open(tester, ports);
    final run = await _started(tester, ports);

    run.updates.add(
      RestoreUpdate.ask(id: 3, ask: InstallAsk.locate(identity: 'ab' * 32)),
    );
    await settle(tester);
    expect(
      run.answers[3],
      const InstallAnswer.located(address: '10.0.0.9:7700'),
    );

    run.updates.add(
      const RestoreUpdate.ask(id: 4, ask: InstallAsk.closeLink()),
    );
    await settle(tester);
    expect(run.answers[4], isA<InstallAnswer_Failed>());
    run.stopped(RestoreStop.cancelled);
    await settle(tester);
  });

  testWidgets(
    'no kept backup says so, and a picked file takes the same checks',
    (tester) async {
      final ports = FakeRestorePorts()
        ..kept = null
        ..checks['/picked/good.bin'] = const BackupCheck.ready(
          version: '1.1.15',
        )
        ..checks['/picked/bad.bin'] = const BackupCheck.notStock(detail: 'no');
      final flow = await _open(tester, ports);
      expect(find.text('No backup on this phone'), findsOneWidget);

      // Nothing picked changes nothing.
      await tester.tap(find.text('Pick the file'));
      await settle(tester);
      expect(find.text('No backup on this phone'), findsOneWidget);

      ports.picks = '/picked/bad.bin';
      await tester.tap(find.text('Pick the file'));
      await settle(tester);
      expect(
        find.textContaining(
          "That isn't the original firmware.",
          findRichText: true,
        ),
        findsOneWidget,
      );
      expect(flow.stage, RestoreStage.noBackup);

      ports.picks = '/picked/good.bin';
      await tester.tap(find.text('Pick the file'));
      await settle(tester);
      expect(find.text('From a picked file'), findsOneWidget);

      final run = await _started(tester, ports);
      expect(ports.specs.single.backup, '/picked/good.bin');
      run.stopped(RestoreStop.cancelled);
      await settle(tester);
    },
  );

  testWidgets('leaving stops the run, and lets the process go', (tester) async {
    final ports = FakeRestorePorts();
    final flow = await _open(tester, ports);
    final run = await _started(tester, ports);

    await tester.pumpWidget(const SizedBox.shrink());
    flow.dispose();

    expect(run.stops, 1);
    expect(ports.released, 1);
  });

  for (final brightness in Brightness.values) {
    testWidgets('every stage draws in $brightness', (tester) async {
      final ports = FakeRestorePorts();
      await _open(tester, ports, brightness: brightness);
      expect(tester.takeException(), isNull);

      final run = await _started(tester, ports);
      run.event(const InstallEvent.pressDispense());
      await settle(tester);
      expect(tester.takeException(), isNull);

      run.event(const InstallEvent.promoting());
      run.end(const RestoreEnd.activated());
      await settle(tester);
      expect(tester.takeException(), isNull);
    });
  }
}
