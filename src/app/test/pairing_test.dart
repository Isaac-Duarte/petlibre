import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show AnyhowException;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/app.dart';
import 'package:wellfed/screens/name_step.dart';
import 'package:wellfed/screens/pairing_view.dart';
import 'package:wellfed/screens/setup_screen.dart';
import 'package:wellfed/src/rust/api/error.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/setup.dart';
import 'package:wellfed/state/pairing.dart';

import 'fakes.dart';

Widget _view(PairingFlow flow, {bool ble = true, VoidCallback? onNewCode}) =>
    themed(
      PairingView(
        flow: flow,
        ble: ble,
        onRetry: () {},
        onNewCode: onNewCode ?? () {},
      ),
    );

void _tall(WidgetTester tester) {
  tester.view.physicalSize = const Size(1080, 3000);
  tester.view.devicePixelRatio = 3;
  addTearDown(tester.view.reset);
}

/// The flow at a failure, with the screen on it.
Future<void> _failed(
  WidgetTester tester, {
  PairingFailure? failure,
  SetupRefusal? refusal,
  bool ble = true,
}) async {
  _tall(tester);
  final flow = PairingFlow(FakeBook(), FakePairingPorts())
    ..phase = PairingPhase.failed
    ..failure = failure
    ..refusal = refusal;
  await tester.pumpWidget(_view(flow, ble: ble));
}

void main() {
  group('3.3 the code', () {
    testWidgets('it shows the tiles, the time left and how to start over', (
      tester,
    ) async {
      _tall(tester);
      var newCodes = 0;
      final flow = PairingFlow(FakeBook(), FakePairingPorts())
        ..phase = PairingPhase.code
        ..code = const [3, 1, 4, 4, 2, 1]
        ..deadline = DateTime.now().add(const Duration(seconds: 47));
      await tester.pumpWidget(_view(flow, onNewCode: () => newCodes++));

      expect(
        find.bySemanticsLabel('Press Mic, Lock, Reset, Reset, Dispense, Lock'),
        findsOneWidget,
      );
      expect(find.text('Waiting for the feeder to confirm…'), findsOneWidget);
      expect(find.textContaining('0:4'), findsOneWidget);

      await tester.tap(find.text('Show a new code'));
      expect(newCodes, 1);
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets(
      'Show a new code goes back to the same feeder with another code',
      (tester) async {
        _tall(tester);
        final book = FakeBook();
        final ports = FakePairingPorts();
        await tester.pumpWidget(
          themed(SetupScreen(app: book, ports: ports, askPermissions: false)),
        );

        await tester.tap(find.text('The Wi-Fi light is blinking'));
        await settle(tester);
        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
        await tester.pump();
        await tester.tap(find.text('Continue'));
        await settle(tester);
        await tester.tap(find.text('Find the feeder'));
        await settle(tester);

        // The search, one feeder close by, and connecting to it.
        ports.scans.add(feederNearby('feeder-1'));
        await settle(tester);
        expect(find.text('In setup mode · close by'), findsOneWidget);
        await tester.tap(find.text('Connect'));
        await settle(tester);
        expect(ports.opened.single.id, 'feeder-1');

        ports.drivers.single.script = const [SetupStep.enterCode(seconds: 60)];
        ports.link.bytes.add(const [1]);
        await tester.pump();
        await tester.pump();
        expect(find.text('Show a new code'), findsOneWidget);
        final first = ports.link;

        await tester.tap(find.text('Show a new code'));
        await tester.pump();
        await tester.pump();

        // The old link hung up, and a new session started at the same feeder.
        expect(first.closes, 1);
        expect(ports.drivers, hasLength(2));
        expect(ports.opened.map((device) => device.id), [
          'feeder-1',
          'feeder-1',
        ]);
        expect(ports.drivers[1].positions, isNot(ports.drivers[0].positions));
        await tester.pumpWidget(const SizedBox());
      },
    );
  });

  group('3.5 names', () {
    testWidgets(
      'both names are asked, and the phone\'s fits the feeder\'s 16 bytes',
      (tester) async {
        String? label;
        String? feeder;
        await tester.pumpWidget(
          themed(
            NameStep(
              initial: 'Pixel',
              feeder: 'Feeder',
              action: 'Find the feeder',
              onDone: (l, f) {
                label = l;
                feeder = f;
              },
            ),
          ),
        );

        expect(find.text('Name things'), findsOneWidget);
        expect(find.text('5/16'), findsOneWidget);

        await tester.enterText(
          find.byType(TextField).at(1),
          'A name far too long to fit',
        );
        await tester.pump();
        expect(
          tester.widget<FilledButton>(find.byType(FilledButton)).onPressed,
          isNull,
        );

        await tester.enterText(find.byType(TextField).at(0), 'Kitchen');
        await tester.enterText(find.byType(TextField).at(1), "Sam's Pixel");
        await tester.pump();
        await tester.tap(find.text('Find the feeder'));
        await tester.pump();
        expect(label, "Sam's Pixel");
        expect(feeder, 'Kitchen');
      },
    );

    testWidgets('a name it cannot save is said so, on this step', (
      tester,
    ) async {
      await tester.pumpWidget(
        themed(
          NameStep(
            initial: 'Pixel',
            action: 'Continue',
            onDone: (_, _) => throw const BridgeError(
              kind: ErrorKind.store,
              message: 'the disk is full',
            ),
          ),
        ),
      );
      await tester.tap(find.text('Continue'));
      await settle(tester);
      expect(
        find.textContaining('the disk is full', findRichText: true),
        findsOneWidget,
      );
    });

    testWidgets('an unnamed phone is offered its own model, cut to fit', (
      tester,
    ) async {
      await tester.pumpWidget(
        themed(
          NameStep(
            initial: '',
            action: 'Continue',
            suggest: () async => 'A phone model far too long',
            onDone: (_, _) {},
          ),
        ),
      );
      await settle(tester);
      expect(find.text('A phone model fa'), findsOneWidget);
    });
  });

  group('3.6 joining and 3.7 ready', () {
    testWidgets(
      'after the code, the feeder is saved and the success screen stays',
      (tester) async {
        _tall(tester);
        final book = FakeBook();
        final ports = FakePairingPorts();
        final flow = PairingFlow(book, ports);

        // Under the router, as the app has it: a saved feeder redraws the root
        // beneath the flow, and the flow's own screen stays on top.
        final router = wellfedRouter(
          book: book,
          root: (context) => Text(book.feeders.isEmpty ? 'Welcome' : 'Home'),
          pages: {'/setup': (context) => _view(flow)},
        );
        await tester.pumpWidget(MaterialApp.router(routerConfig: router));
        unawaited(router.push('/setup'));
        await settle(tester);

        unawaited(
          flow.runSetup(
            wifi: const WifiCredentials(ssid: 'Maple St', passphrase: 'x'),
            name: 'Kitchen',
            device: feederNearby('feeder-1'),
          ),
        );
        await tester.pump();
        await tester.pump();
        ports.drivers.single.script = [doneStep];
        ports.link.bytes.add(const [1]);
        await settle(tester);

        expect(book.feeders.single.name, 'Kitchen');
        expect(book.remembered.single.$2, isTrue, reason: 'its clock is due');
        expect(find.text('Kitchen is ready.'), findsOneWidget);
        expect(find.textContaining('on Maple St'), findsOneWidget);

        await tester.tap(find.text('Go to Kitchen'));
        await settle(tester);
        expect(find.text('Home'), findsOneWidget);
      },
    );

    testWidgets('a feeder already owned says nothing changed', (tester) async {
      _tall(tester);
      final flow = PairingFlow(FakeBook(), FakePairingPorts())
        ..phase = PairingPhase.done
        ..enrolled = Enrolled.already
        ..feederName = 'Kitchen';
      await tester.pumpWidget(_view(flow));
      expect(find.textContaining('already one of its owners'), findsOneWidget);
    });

    testWidgets('the joining steps name the network', (tester) async {
      _tall(tester);
      final flow = PairingFlow(FakeBook(), FakePairingPorts())
        ..phase = PairingPhase.joining
        ..network = 'Maple St'
        ..feederName = 'Kitchen'
        ..wifiSaved = WifiSaved.saved;
      await tester.pumpWidget(_view(flow));
      await tester.pump();
      expect(find.text('Kitchen is joining your Wi-Fi'), findsOneWidget);
      expect(find.text('Wi-Fi saved on the feeder'), findsOneWidget);
      expect(
        find.textContaining('Keep this phone on Maple St'),
        findsOneWidget,
      );
      await tester.pumpWidget(const SizedBox());
    });
  });

  group('3.8 setup did not finish', () {
    for (final (refusal, title) in [
      (SetupRefusal.wrongCode, "That code didn't match"),
      (SetupRefusal.unconfirmed, 'Time ran out'),
      (SetupRefusal.full, 'The feeder is full'),
      (SetupRefusal.busy, 'Busy with another phone'),
      (SetupRefusal.notWritable, "The feeder can't save right now"),
      (SetupRefusal.invalid, 'The feeder rejected the name or Wi-Fi'),
    ]) {
      testWidgets('the feeder refused: $title', (tester) async {
        await _failed(
          tester,
          failure: PairingFailure.refused,
          refusal: refusal,
        );
        expect(find.text(title), findsOneWidget);
        expect(find.text('Try again with a new code'), findsOneWidget);
        // Listed once, not again among the other reasons.
        expect(
          find.text('Other reasons setup stops'.toUpperCase()),
          findsOneWidget,
        );
      });
    }

    testWidgets('a wrong code is still in setup mode', (tester) async {
      await _failed(
        tester,
        failure: PairingFailure.refused,
        refusal: SetupRefusal.wrongCode,
      );
      expect(find.textContaining("It's still in setup mode"), findsOneWidget);
      expect(find.text('Setup mode ended'), findsOneWidget);
    });

    for (final (failure, title) in [
      (PairingFailure.notFound, 'No feeder in setup mode nearby'),
      (PairingFailure.link, 'The Bluetooth link dropped'),
      (PairingFailure.mismatch, "That code didn't match"),
      (PairingFailure.timeout, 'The feeder stopped answering'),
      (PairingFailure.windowClosed, 'The feeder is not taking an owner now'),
      (PairingFailure.input, 'Check the names and Wi-Fi'),
      (PairingFailure.other, "Setup didn't finish"),
    ]) {
      testWidgets('it stopped: $title', (tester) async {
        await _failed(tester, failure: failure);
        expect(find.text(title), findsOneWidget);
        expect(find.text('Try again with a new code'), findsOneWidget);
      });
    }

    testWidgets(
      'paired but not on this network goes Home, which keeps looking',
      (tester) async {
        await _failed(tester, failure: PairingFailure.notOnLan);
        expect(find.text('Paired, but not on this network'), findsOneWidget);
        expect(find.text('Go to Home'), findsOneWidget);
        expect(find.text('Try again with a new code'), findsNothing);
      },
    );

    testWidgets('recovery lists no setup reasons', (tester) async {
      await _failed(tester, failure: PairingFailure.windowClosed, ble: false);
      expect(find.text('Setup mode ended'), findsNothing);
    });
  });

  group('recovery', () {
    Future<PairingFlow> started(FakePairingPorts ports) async {
      final flow = PairingFlow(FakeBook(), ports);
      unawaited(
        flow.runRecovery(address: '192.168.1.40:4747', name: 'Kitchen'),
      );
      await Future<void>.delayed(Duration.zero);
      return flow;
    }

    test(
      'a shut window comes back as its own failure, not an exception',
      () async {
        final ports = FakePairingPorts();
        final flow = await started(ports);

        // Rust sends a failed recovery as a step: frb decodes a stream's error
        // as AnyhowException whatever Rust encoded.
        ports.recoveries.single.steps.add(
          const SetupStep.failed(
            error: BridgeError(kind: ErrorKind.closed, message: 'closed'),
          ),
        );
        await Future<void>.delayed(Duration.zero);
        await Future<void>.delayed(Duration.zero);

        expect(flow.phase, PairingPhase.failed);
        expect(flow.failure, PairingFailure.windowClosed);
        flow.dispose();
      },
    );

    test(
      'an error on the stream is still a screen, not an exception',
      () async {
        final ports = FakePairingPorts();
        final flow = await started(ports);

        ports.recoveries.single.steps.addError(AnyhowException('garbled'));
        await Future<void>.delayed(Duration.zero);
        await Future<void>.delayed(Duration.zero);

        expect(flow.phase, PairingPhase.failed);
        expect(flow.failure, PairingFailure.other);
        expect(flow.message, contains('garbled'));
        flow.dispose();
      },
    );

    test('closing the screen hangs up in Rust too', () async {
      final ports = FakePairingPorts();
      final flow = await started(ports);
      flow.dispose();
      expect(ports.recoveries.single.closes, 1);
    });
  });

  testWidgets('a saved feeder does not take the flow off the screen', (
    tester,
  ) async {
    final book = FakeBook();
    var builds = 0;
    final router = wellfedRouter(
      book: book,
      root: (context) => Text(book.feeders.isEmpty ? 'Welcome' : 'Home'),
      pages: {
        '/flow': (context) {
          builds++;
          return const Scaffold(body: Text('Flow'));
        },
      },
    );
    await tester.pumpWidget(MaterialApp.router(routerConfig: router));
    expect(find.text('Welcome'), findsOneWidget);
    unawaited(router.push('/flow'));
    await settle(tester);
    expect(find.text('Flow'), findsOneWidget);
    final built = builds;

    await book.remember(const KnownFeeder(identity: 'ab', name: 'Kitchen'));
    await settle(tester);
    expect(find.text('Flow'), findsOneWidget);
    expect(builds, built, reason: 'the page above the root was left alone');

    router.pop();
    await settle(tester);
    expect(find.text('Home'), findsOneWidget);
  });
}
