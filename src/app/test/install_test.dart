import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:wellfed/screens/install_screen.dart';
import 'package:wellfed/src/rust/api/install.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/store.dart';
import 'package:wellfed/state/install.dart';
import 'package:wellfed/theme/theme.dart';
import 'package:wellfed/widgets/code_tiles.dart';
import 'package:wellfed/widgets/layout.dart';

import 'fakes.dart';

/// The screen under a router, as the app has it, so leaving works.
Widget _app(FakeBook book, FakeInstallPorts ports, {Brightness? brightness}) {
  final router = GoRouter(
    initialLocation: '/install',
    routes: [
      GoRoute(path: '/', builder: (_, _) => const Text('Home')),
      GoRoute(
        path: '/install',
        builder: (_, _) => InstallScreen(book: book, ports: ports),
      ),
    ],
  );
  return MaterialApp.router(
    theme: wellfedTheme(brightness ?? Brightness.light),
    routerConfig: router,
  );
}

/// The install's first screen. Most of these tests are about what both routes
/// share, and reach it by the router's.
Future<void> _open(
  WidgetTester tester,
  FakeInstallPorts ports, {
  FakeBook? book,
  InstallRoute route = InstallRoute.dns,
}) async {
  // Tall enough that a whole screen is built, not just what scrolls into view.
  tester.view.physicalSize = const Size(1080, 6000);
  tester.view.devicePixelRatio = 3;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_app(book ?? FakeBook(), ports));
  await settle(tester);
  if (route == InstallRoute.dns) await _tap(tester, 'Through your router');
}

Future<void> _tap(WidgetTester tester, String text) async {
  await tester.ensureVisible(find.text(text));
  await settle(tester);
  await tester.tap(find.text(text));
  await settle(tester);
}

/// From the first screen to a run waiting on the record.
Future<FakeRun> _toRun(
  WidgetTester tester,
  FakeInstallPorts ports, {
  FakeBook? book,
}) async {
  await _open(tester, ports, book: book);
  await _tap(tester, 'Continue');
  await tester.enterText(find.byType(TextField).at(0), 'Maple St');
  await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
  await tester.pump();
  await _tap(tester, 'Continue');
  await tester.enterText(find.byType(TextField), 'broker.example.net');
  await tester.pump();
  await _tap(tester, 'Check the record');
  return ports.run;
}

/// A run that reached the given screen by the events a real one sends.
Future<FakeRun> _atConfirm(WidgetTester tester, FakeInstallPorts ports) async {
  final run = await _toRun(tester, ports);
  run.ask(
    9,
    const InstallAsk.recognizeFeeder(
      device: 'AF_TEST',
      address: '192.168.1.40',
    ),
  );
  await settle(tester);
  return run;
}

Future<FakeRun> _atClaim(WidgetTester tester, FakeInstallPorts ports) async {
  final run = await _toRun(tester, ports);
  run.event(const InstallEvent.setupStarting());
  await settle(tester);
  return run;
}

Future<FakeRun> _atFinishing(
  WidgetTester tester,
  FakeInstallPorts ports,
) async {
  final run = await _toRun(tester, ports);
  run.event(
    InstallEvent.paired(
      identity: Uint8List.fromList(const [0xab, 0xcd]),
      rebooting: true,
    ),
  );
  await settle(tester);
  return run;
}

Future<FakeRun> _stopped(
  WidgetTester tester,
  FakeInstallPorts ports,
  StopCase stop, {
  Resume resume = Resume.nothing,
  String detail = 'its words',
}) async {
  final run = await _toRun(tester, ports);
  run.stopped(stop, resume: resume, detail: detail);
  await settle(tester);
  return run;
}

void main() {
  group('2.1 before you start', () {
    testWidgets(
      'the feeder\'s own Wi-Fi is first, and the router is on offer',
      (tester) async {
        final ports = FakeInstallPorts();
        await _open(tester, ports, route: InstallRoute.reset);

        expect(find.text('Install Wellfed'), findsOneWidget);
        expect(find.text('Through your router'), findsOneWidget);
        expect(find.text("Through the feeder's own Wi-Fi"), findsOneWidget);
        expect(find.text('Coming later'), findsNothing);
      },
    );

    for (final (image, words) in [
      (const ImageCheck.absent(), 'This build has no firmware to install.'),
      (const ImageCheck.notPinned(), 'This build pins no firmware.'),
      (
        const ImageCheck.mismatch(detail: 'review.json is not the pinned one'),
        "This build's firmware doesn't check out.",
      ),
    ]) {
      testWidgets('an APK with no usable image says so: $words', (
        tester,
      ) async {
        await _open(tester, FakeInstallPorts(image: image));
        expect(find.textContaining(words, findRichText: true), findsOneWidget);
        final button = tester.widget<FilledButton>(find.byType(FilledButton));
        expect(button.onPressed, isNull);
      });
    }

    testWidgets('a build of unclean sources cannot send an install', (
      tester,
    ) async {
      final ports = FakeInstallPorts(
        build: const BuildCheck.unreviewed(commit: 'abc1234'),
      );
      await _open(tester, ports);
      expect(
        find.textContaining(
          'This build cannot send an install.',
          findRichText: true,
        ),
        findsOneWidget,
      );
      final button = tester.widget<FilledButton>(find.byType(FilledButton));
      expect(button.onPressed, isNull);
    });

    testWidgets('Bluetooth refused stays here and says what to allow', (
      tester,
    ) async {
      final ports = FakeInstallPorts()..allowed = false;
      await _open(tester, ports);
      await _tap(tester, 'Continue');
      expect(find.text('Nearby devices is off'), findsOneWidget);
    });

    testWidgets('an unnamed phone is named first', (tester) async {
      final book = FakeBook(label: '');
      await _open(tester, FakeInstallPorts(), book: book);
      await _tap(tester, 'Continue');
      expect(find.text('Name this phone'), findsOneWidget);
      await tester.enterText(find.byType(TextField).first, 'Pixel');
      await tester.pump();
      await _tap(tester, 'Continue');
      expect(book.label, 'Pixel');
      expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);
    });

    testWidgets(
      'a pending install picks up from bank 0, asking stock nothing',
      (tester) async {
        final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired);
        await _open(tester, ports);
        await _tap(tester, 'Pick up where it stopped');
        expect(ports.specs.single.start, const InstallStart.bank0());
        // Paired: no Wi-Fi to give it.
        expect(ports.specs.single.home, isNull);
        expect(find.text('Wellfed is running. Now claim it.'), findsOneWidget);
      },
    );

    testWidgets(
      'an open request offers the listener, and never a new request',
      (tester) async {
        final ports = FakeInstallPorts(
          pendingInstall: PendingInstall.requestOpen,
        );
        await _open(tester, ports);
        expect(
          find.textContaining(
            'An earlier install has not finished.',
            findRichText: true,
          ),
          findsOneWidget,
        );
        expect(find.text('Pick up where it stopped'), findsNothing);

        await _tap(tester, 'Listen for the feeder');
        expect(ports.runs, isEmpty);
        expect(ports.listeners, hasLength(1));
        expect(find.text("We don't know yet if it installed"), findsOneWidget);
        await tester.pumpWidget(const SizedBox());
      },
    );

    testWidgets(
      'looking over Bluetooth after a restart asks for the Wi-Fi first',
      (tester) async {
        final ports = FakeInstallPorts(
          pendingInstall: PendingInstall.requestOpen,
        );
        await _open(tester, ports);
        await _tap(tester, 'Look for it over Bluetooth');
        expect(ports.runs, isEmpty);
        expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);

        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
        await tester.pump();
        await _tap(tester, 'Continue');
        expect(ports.specs.single.start, const InstallStart.bank0());
        expect(ports.specs.single.home?.ssid, 'Maple St');
        expect(find.text('Wellfed is running. Now claim it.'), findsOneWidget);
      },
    );

    testWidgets('Back from the Wi-Fi a pick-up asked for drops that pick-up', (
      tester,
    ) async {
      final ports = FakeInstallPorts(
        pendingInstall: PendingInstall.requestOpen,
      );
      await _open(tester, ports);
      await _tap(tester, 'Look for it over Bluetooth');
      expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);

      await tester.tap(find.byTooltip('Back'));
      await settle(tester);
      await _tap(tester, 'Continue');
      await tester.enterText(find.byType(TextField).at(0), 'Maple St');
      await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
      await tester.pump();
      await _tap(tester, 'Continue');

      // The owner's Continue is the first screen's: on to the record, with no
      // run from bank 0.
      expect(ports.runs, isEmpty);
      expect(find.text('Point the feeder at this phone'), findsOneWidget);
    });

    testWidgets('every way on from the first screen asks Android first', (
      tester,
    ) async {
      for (final button in [
        'Listen for the feeder',
        'Look for it over Bluetooth',
      ]) {
        final ports = FakeInstallPorts(
          pendingInstall: PendingInstall.requestOpen,
        )..allowed = false;
        await _open(tester, ports);
        await _tap(tester, button);
        expect(ports.asked, 1, reason: button);
        expect(ports.runs, isEmpty, reason: button);
        expect(ports.listeners, isEmpty, reason: button);
        expect(
          find.textContaining('Nearby devices is off.', findRichText: true),
          findsOneWidget,
          reason: button,
        );
        await tester.pumpWidget(const SizedBox());
        await settle(tester);
      }

      final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired)
        ..allowed = false;
      await _open(tester, ports);
      await _tap(tester, 'Pick up where it stopped');
      expect(ports.asked, 1);
      expect(ports.runs, isEmpty);
    });

    testWidgets('an unnamed phone is named before a pick-up, which goes on', (
      tester,
    ) async {
      final book = FakeBook(label: '');
      final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired);
      await _open(tester, ports, book: book);
      await _tap(tester, 'Pick up where it stopped');
      expect(find.text('Name this phone'), findsOneWidget);
      await tester.enterText(find.byType(TextField).first, 'Pixel');
      await tester.pump();
      await _tap(tester, 'Continue');
      expect(ports.specs.single.label, 'Pixel');
      expect(ports.specs.single.start, const InstallStart.bank0());
    });
  });

  group('2.3b the record', () {
    testWidgets(
      'it shows this phone, and a record that points here starts the run',
      (tester) async {
        final ports = FakeInstallPorts();
        final run = await _toRun(tester, ports);

        expect(find.text('192.168.1.23'), findsOneWidget);
        expect(ports.dnsAsked, ['broker.example.net']);
        final spec = ports.specs.single;
        expect(spec.start, const InstallStart.dns(name: 'broker.example.net'));
        expect(spec.phoneIp, '192.168.1.23');
        expect(spec.home?.ssid, 'Maple St');
        expect(spec.label, 'Pixel');
        expect(find.text('Waiting for the feeder to connect…'), findsOneWidget);
        expect(
          find.textContaining('Then restart the feeder:', findRichText: true),
          findsOneWidget,
        );

        // The run's prompts are answered by what the screen already shows.
        run.ask(1, const InstallAsk.setOverride(name: 'x', broker: 'y'));
        run.ask(2, const InstallAsk.restartFeeder());
        await settle(tester);
        expect(run.answers[1], const InstallAnswer.done());
        expect(run.answers[2], const InstallAnswer.done());
        // The process is held for the run.
        expect(ports.held, isNotEmpty);
      },
    );

    for (final (result, words) in [
      (const DnsResult.unresolved(), 'Nothing answers that name yet.'),
      (
        const DnsResult.mixed(others: ['1.1.1.1']),
        "This phone isn't the only answer.",
      ),
      (
        const DnsResult.elsewhere(others: ['1.1.1.1']),
        "The record doesn't point here yet.",
      ),
    ]) {
      testWidgets('a lookup is advice: $words', (tester) async {
        final ports = FakeInstallPorts(dns: result);
        await _open(tester, ports);
        await _tap(tester, 'Continue');
        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
        await tester.pump();
        await _tap(tester, 'Continue');
        await tester.enterText(find.byType(TextField), 'broker.example.net');
        await tester.pump();
        await _tap(tester, 'Check the record');

        expect(find.textContaining(words, findRichText: true), findsOneWidget);
        expect(ports.runs, isEmpty);
        // This phone may ask another resolver than the feeder does.
        await _tap(tester, 'Go on anyway');
        expect(ports.runs, hasLength(1));
      });
    }

    testWidgets('a name that cannot be one offers no way on', (tester) async {
      final ports = FakeInstallPorts(dns: const DnsResult.badName());
      await _open(tester, ports);
      await _tap(tester, 'Continue');
      await tester.enterText(find.byType(TextField).at(0), 'Maple St');
      await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
      await tester.pump();
      await _tap(tester, 'Continue');
      await tester.enterText(find.byType(TextField), '1.2.3.4');
      await tester.pump();
      await _tap(tester, 'Check the record');

      expect(
        find.textContaining("That can't be its name.", findRichText: true),
        findsOneWidget,
      );
      expect(find.text('Go on anyway'), findsNothing);
    });

    testWidgets('a phone off Wi-Fi says so and cannot check', (tester) async {
      final ports = FakeInstallPorts()..ip = null;
      await _open(tester, ports);
      await _tap(tester, 'Continue');
      await tester.enterText(find.byType(TextField).at(0), 'Maple St');
      await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
      await tester.pump();
      await _tap(tester, 'Continue');
      expect(
        find.textContaining("This phone isn't on Wi-Fi.", findRichText: true),
        findsOneWidget,
      );
      expect(find.text('Check the record'), findsNothing);
    });

    testWidgets(
      'why it still waits, and a lost report asks for one more restart',
      (tester) async {
        final ports = FakeInstallPorts();
        final run = await _toRun(tester, ports);

        run.event(const InstallEvent.waiting(why: WaitReason.notOnAdapter));
        await settle(tester);
        expect(find.textContaining('on batteries'), findsOneWidget);

        run.event(const InstallEvent.versionLost());
        await settle(tester);
        expect(
          find.textContaining(
            'Restart the feeder once more:',
            findRichText: true,
          ),
          findsOneWidget,
        );
      },
    );

    testWidgets('leaving a run asks first, and stops it through its cleanup', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toRun(tester, ports);
      await tester.tap(find.byTooltip('Close'));
      await settle(tester);
      expect(find.text('Stop the install?'), findsOneWidget);
      await _tap(tester, 'Stop');
      expect(run.stops, 1);
    });
  });

  group('2.4 is this your feeder', () {
    testWidgets('it shows what connected, and yes goes on to the install', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _atConfirm(tester, ports);

      expect(find.text('A feeder connected. Is it yours?'), findsOneWidget);
      expect(find.text('AF_TEST'), findsOneWidget);
      expect(find.text('192.168.1.40'), findsOneWidget);
      expect(find.text('Vendor firmware 1.1.15'), findsOneWidget);

      await _tap(tester, 'Yes, install Wellfed');
      expect(run.answers[9], const InstallAnswer.yes(yes: true));
      expect(find.text('Installing Wellfed'), findsOneWidget);
    });

    testWidgets('no answers no, and nothing was sent', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _atConfirm(tester, ports);
      await _tap(tester, "That's not my feeder");
      expect(run.answers[9], const InstallAnswer.yes(yes: false));
    });
  });

  group('2.5 installing', () {
    testWidgets('progress, the time left, and the restart', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _toRun(tester, ports);

      run.event(const InstallEvent.requesting(build: 'ab'));
      run.event(const InstallEvent.stockProgress(percent: 62));
      await settle(tester);
      expect(find.text('Installing Wellfed'), findsOneWidget);
      expect(find.text('Sending firmware'), findsOneWidget);
      expect(find.textContaining('62%'), findsOneWidget);
      expect(
        find.textContaining('Keep Wellfed open.', findRichText: true),
        findsOneWidget,
      );
      expect(ports.held.last, contains('serving the firmware'));

      run.event(const InstallEvent.stockRestarting());
      await settle(tester);
      expect(find.text('The feeder is restarting'), findsOneWidget);
    });
  });

  group('2.6 claim', () {
    testWidgets(
      'a consumer image is found over Bluetooth, with the code as setup shows it',
      (tester) async {
        final ports = FakeInstallPorts();
        final run = await _atClaim(tester, ports);
        expect(find.text('Wellfed is running. Now claim it.'), findsOneWidget);
        expect(
          find.text('Looking for the feeder over Bluetooth…'),
          findsOneWidget,
        );

        run.ask(3, const InstallAsk.openLink());
        await settle(tester);
        expect(run.answers[3], const InstallAnswer.done());

        run.ask(
          4,
          InstallAsk.writeLink(
            pieces: [
              Uint8List.fromList(const [1, 2]),
            ],
          ),
        );
        await settle(tester);
        expect(run.answers[4], const InstallAnswer.done());
        expect(ports.link.written.single, [
          [1, 2],
        ]);

        ports.onBytes!(const [5, 6]);
        expect(run.notifications.single, [5, 6]);

        run.event(
          InstallEvent.code(
            code: Uint8List.fromList(const [3, 1, 4, 4, 2, 1]),
            seconds: 60,
          ),
        );
        await tester.pump();
        expect(find.byType(CodeTiles), findsOneWidget);
        expect(
          find.bySemanticsLabel(
            'Press Mic, Lock, Reset, Reset, Dispense, Lock',
          ),
          findsOneWidget,
        );
        expect(find.textContaining(RegExp(r'^(1:00|0:5\d)$')), findsOneWidget);
        expect(find.text('Waiting for the feeder to confirm…'), findsOneWidget);

        run.ask(5, const InstallAsk.closeLink());
        await tester.pump();
        expect(run.answers[5], const InstallAnswer.done());
        expect(ports.link.closes, 1);
        await tester.pumpWidget(const SizedBox());
      },
    );

    testWidgets('a radio that finds nothing tells the run, which tries again', (
      tester,
    ) async {
      final ports = FakeInstallPorts()..linkFails = 'no feeder in time';
      final run = await _atClaim(tester, ports);
      run.ask(3, const InstallAsk.openLink());
      await settle(tester);
      expect(
        run.answers[3],
        const InstallAnswer.failed(reason: 'no feeder in time'),
      );
    });

    testWidgets('a dropped link reaches the run', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _atClaim(tester, ports);
      run.ask(3, const InstallAsk.openLink());
      await settle(tester);
      ports.onLost!();
      expect(run.lost, 1);
    });

    testWidgets('NSD answers the run', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _atClaim(tester, ports);
      run.ask(6, const InstallAsk.locate(identity: 'abcd'));
      await settle(tester);
      expect(
        run.answers[6],
        const InstallAnswer.located(address: '192.168.1.40:4747'),
      );
    });

    testWidgets(
      'a bench image asks for its recovery window, and the owner says when',
      (tester) async {
        final ports = FakeInstallPorts(
          image: const ImageCheck.ready(
            provisioning: ImageProvisioning.bench,
            build: 'ab',
            version: 7,
          ),
        );
        await _open(tester, ports);
        await _tap(tester, 'Continue');
        // A bench image joins its build network: no Wi-Fi to give it.
        await tester.enterText(find.byType(TextField), 'broker.example.net');
        await tester.pump();
        await _tap(tester, 'Check the record');
        final run = ports.run;
        expect(ports.specs.single.home, isNull);

        run.ask(7, const InstallAsk.openRecoveryWindow());
        await settle(tester);
        expect(
          find.textContaining('then also the', findRichText: true),
          findsOneWidget,
        );
        await _tap(tester, 'The lock light is blinking fast');
        expect(run.answers[7], const InstallAnswer.done());
      },
    );
  });

  group('long waits', () {
    testWidgets(
      'no find over Bluetooth asks for the hold, until a code comes',
      (tester) async {
        final ports = FakeInstallPorts();
        final run = await _atClaim(tester, ports);
        Finder hint() =>
            find.textContaining('Hold Lock + Mic for 5 s', findRichText: true);
        expect(hint(), findsNothing);

        await tester.pump(InstallFlow.setupHintAfter);
        expect(hint(), findsOneWidget);
        expect(
          find.textContaining('If the lock light is on', findRichText: true),
          findsOneWidget,
        );

        run.event(
          InstallEvent.code(
            code: Uint8List.fromList(const [1, 2]),
            seconds: 60,
          ),
        );
        await tester.pump();
        expect(hint(), findsNothing);
      },
    );

    testWidgets('joining says what it waits for, and only once it is long', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _atFinishing(tester, ports);
      Finder words() => find.textContaining(
        'restart and join your Wi-Fi',
        findRichText: true,
      );
      expect(words(), findsNothing);

      await tester.pump(WaitWords.after);
      await tester.pump();
      expect(words(), findsOneWidget);

      run.event(const InstallEvent.joined(address: '192.168.1.40:4747'));
      await tester.pump();
      expect(words(), findsNothing);
    });

    testWidgets('copying goes quiet only when its progress stops', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _atFinishing(tester, ports);
      run.event(const InstallEvent.joined(address: '192.168.1.40:4747'));
      run.event(const InstallEvent.readingBackup(total: 1000));
      await tester.pump();
      Finder words() => find.textContaining(
        'Waiting for the feeder to answer',
        findRichText: true,
      );

      await tester.pump(WaitWords.after - const Duration(seconds: 1));
      run.event(InstallEvent.backupRead(bytes: BigInt.from(500), total: 1000));
      await tester.pump();
      await tester.pump(WaitWords.after - const Duration(seconds: 1));
      expect(words(), findsNothing);

      await tester.pump(const Duration(seconds: 2));
      await tester.pump();
      expect(words(), findsOneWidget);
    });
  });

  group('2.7 back up and finish', () {
    testWidgets('each step in its turn', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _atFinishing(tester, ports);

      expect(find.text('Backing up the original firmware'), findsOneWidget);
      expect(find.text('This phone is its owner'), findsOneWidget);
      expect(find.text('Finding it on your network'), findsOneWidget);

      run.event(const InstallEvent.joined(address: '192.168.1.40:4747'));
      run.event(const InstallEvent.readingBackup(total: 1000));
      run.event(InstallEvent.backupRead(bytes: BigInt.from(750), total: 1000));
      await tester.pump();
      expect(find.text('75%'), findsOneWidget);

      run.event(const InstallEvent.backupKept(path: '/data/stock.bin'));
      run.event(const InstallEvent.promoting());
      run.event(const InstallEvent.written(written: 500, length: 1000));
      await tester.pump();
      expect(find.text('Making Wellfed permanent'), findsOneWidget);
      expect(find.text('50%'), findsOneWidget);

      run.event(const InstallEvent.activated());
      await tester.pump();
      expect(find.text('Restart and confirm'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    });
  });

  group('2.8 done', () {
    Future<(FakeRun, FakeBook)> finished(
      WidgetTester tester,
      FakeInstallPorts ports, {
      PlansOutcome plans = PlansOutcome.set_,
    }) async {
      final book = FakeBook();
      final run = await _toRun(tester, ports, book: book);
      run.event(
        InstallEvent.paired(
          identity: Uint8List.fromList(const [0xab, 0xcd]),
          rebooting: true,
        ),
      );
      run.event(const InstallEvent.joined(address: '192.168.1.40:4747'));
      run.event(const InstallEvent.backupKept(path: '/data/stock.bin'));
      run.event(const InstallEvent.stockPlans(converted: 3, refused: 0));
      run.event(const InstallEvent.installed(build: 'ab'));
      run.event(InstallEvent.plans(result: plans));
      run.end(const InstallEnd.done());
      await settle(tester);
      return (run, book);
    }

    testWidgets(
      'it says what came along and to remove the record, and names the feeder',
      (tester) async {
        final ports = FakeInstallPorts();
        final (_, book) = await finished(tester, ports);

        expect(find.text('Your feeder runs Wellfed.'), findsOneWidget);
        expect(find.text('Its 3 meals came along.'), findsOneWidget);
        expect(
          find.textContaining('Remove the DNS record now.', findRichText: true),
          findsOneWidget,
        );
        expect(ports.releases, 1);

        // Home hears of it as the run ends, due its first clock sync.
        expect(book.remembered.first.$2, isTrue);

        await tester.enterText(find.byType(TextField), 'Kitchen');
        await _tap(tester, 'Go to the feeder');
        expect(find.text('Home'), findsOneWidget);
        final saved = book.feeders.single;
        expect(saved.identity, 'abcd');
        expect(saved.name, 'Kitchen');
        expect(saved.address, '192.168.1.40:4747');
      },
    );

    testWidgets('the backup goes to the share sheet', (tester) async {
      final ports = FakeInstallPorts();
      await finished(tester, ports);
      await _tap(tester, 'Save the backup somewhere else');
      expect(ports.shared, ['/data/stock.bin']);
    });

    for (final (plans, words) in [
      (PlansOutcome.noneHeld, 'It had no meals set.'),
      (PlansOutcome.noneConverted, "Its meals couldn't be moved."),
      (PlansOutcome.skipped, 'Its meals stayed behind.'),
      (PlansOutcome.notRead, 'Its meals stayed behind.'),
    ]) {
      testWidgets('meals: $words', (tester) async {
        await finished(tester, FakeInstallPorts(), plans: plans);
        expect(find.textContaining(words), findsOneWidget);
      });
    }
  });

  group('2.9 stopped', () {
    testWidgets(
      'an uncertain install is listened for, counts to 20 minutes and sends nothing',
      (tester) async {
        final ports = FakeInstallPorts();
        final run = await _stopped(tester, ports, const StopCase.uncertain());

        expect(find.text("We don't know yet if it installed"), findsOneWidget);
        final listener = ports.listeners.single;
        expect(
          ports.releases,
          0,
          reason: 'the broker stays up while it listens',
        );

        listener.updates.add(
          const ListenUpdate.tick(seconds: 180, stock: false),
        );
        await tester.pump();
        expect(
          find.text('Listening for the feeder · 3 of 20 min'),
          findsOneWidget,
        );

        // The three other ways stay on the screen.
        expect(find.text('Different vendor firmware'), findsOneWidget);
        expect(find.text('Two feeders answered'), findsOneWidget);
        expect(find.text("The backup didn't match"), findsOneWidget);

        // Only one run: the request is never sent a second time.
        expect(ports.runs, hasLength(1));
        expect(run.answers, isEmpty);
        await tester.pumpWidget(const SizedBox());
      },
    );

    testWidgets(
      'hearing the replacement advertise setup picks up from bank 0',
      (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(tester, ports, const StopCase.uncertain());
        ports.heard.add(null);
        await settle(tester);

        expect(ports.listeners.single.stops, 1);
        expect(ports.specs.last.start, const InstallStart.bank0());
        expect(find.text('Wellfed is running. Now claim it.'), findsOneWidget);
      },
    );

    testWidgets(
      'the replacement answering on the network picks up from bank 0',
      (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(tester, ports, const StopCase.uncertain());
        ports.listeners.single.updates.add(
          const ListenUpdate.replacement(address: '192.168.1.40:4747'),
        );
        await settle(tester);
        expect(ports.specs.last.start, const InstallStart.bank0());
        expect(ports.specs.last.address, '192.168.1.40:4747');
        // A run of its own, with the listener's port let go.
        expect(ports.runs, hasLength(2));
        expect(ports.listeners.single.stops, 1);
      },
    );

    testWidgets(
      'stock coming back says it did not install, and a closed request may start over',
      (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(tester, ports, const StopCase.uncertain());
        ports.listeners.single.updates.add(
          const ListenUpdate.stockStayed(resolved: true),
        );
        await settle(tester);

        expect(find.text("It didn't install"), findsOneWidget);
        await _tap(tester, 'Start again');
        expect(find.text('Point the feeder at this phone'), findsOneWidget);
      },
    );

    testWidgets(
      'stock coming back with the request still open offers no new request',
      (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(tester, ports, const StopCase.uncertain());
        ports.listeners.single.updates.add(
          const ListenUpdate.stockStayed(resolved: false),
        );
        await settle(tester);

        expect(find.text("It didn't install"), findsOneWidget);
        expect(find.text('Start again'), findsNothing);
        await _tap(tester, 'Listen again');
        expect(ports.listeners, hasLength(2));
      },
    );

    testWidgets(
      'silence for the whole wait offers listening again, and never a resend',
      (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(tester, ports, const StopCase.uncertain());
        ports.listeners.single.updates.add(const ListenUpdate.silent());
        await settle(tester);

        expect(find.text('Still no word from the feeder'), findsOneWidget);
        expect(find.text('Start again'), findsNothing);
        expect(find.text('Listen again'), findsOneWidget);
        expect(find.text('Look for it over Bluetooth'), findsOneWidget);
        expect(ports.releases, 1);
      },
    );

    testWidgets('a listener that cannot run says why', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(tester, ports, const StopCase.uncertain());
      ports.listeners.single.updates.add(
        const ListenUpdate.failed(detail: 'no enrollment'),
      );
      await settle(tester);
      expect(find.text("We can't listen for the feeder"), findsOneWidget);
      expect(find.textContaining('no enrollment'), findsOneWidget);
    });

    testWidgets('different firmware', (tester) async {
      await _stopped(
        tester,
        FakeInstallPorts(),
        const StopCase.otherFirmware(version: '1.1.14'),
      );
      expect(find.text('Different vendor firmware'), findsOneWidget);
      expect(find.textContaining('This feeder runs 1.1.14.'), findsOneWidget);
      // Listed once, not again among the others.
      expect(find.text('Two feeders answered'), findsOneWidget);
    });

    testWidgets('two feeders forget the trust and start over', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.twoFeeders(),
        resume: Resume.fromStart,
      );
      expect(find.text('Two feeders answered'), findsOneWidget);
      await _tap(tester, 'Start again');
      await _tap(tester, 'Check the record');
      expect(ports.specs.last.forgetEnrollment, isTrue);
    });

    testWidgets('a backup that does not match picks up from bank 0', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.backupMismatch(),
        resume: Resume.fromBank0,
      );
      expect(find.text("The backup didn't match"), findsOneWidget);
      await _tap(tester, 'Pick up where it stopped');
      expect(ports.specs.last.start, const InstallStart.bank0());
    });

    testWidgets("not the owner's feeder forgets the trust", (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.notYourFeeder(),
        resume: Resume.fromStart,
      );
      expect(find.text("That wasn't your feeder"), findsOneWidget);
      await _tap(tester, 'Start again');
      await _tap(tester, 'Check the record');
      expect(ports.specs.last.forgetEnrollment, isTrue);
    });

    testWidgets('an open request can be listened for', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.earlierRequestOpen(),
        resume: Resume.fromStart,
      );
      expect(find.text('An earlier install is still open'), findsOneWidget);
      await _tap(tester, 'Listen for it');
      expect(ports.listeners, hasLength(1));
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets('an unset clock is the owner\'s to set', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.clockUnset(),
        resume: Resume.fromStart,
      );
      expect(find.text("The feeder's clock isn't set"), findsOneWidget);
      await _tap(tester, 'Set its clock and start again');
      await _tap(tester, 'Check the record');
      expect(ports.specs.last.calibrateClock, isTrue);
    });

    testWidgets('unreadable meals are the owner\'s to leave', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(
        tester,
        ports,
        const StopCase.plansUnreadable(),
        resume: Resume.fromBank0,
      );
      expect(find.text("Its meals couldn't be read"), findsOneWidget);
      await _tap(tester, 'Go on without its meals');
      expect(ports.specs.last.withoutStockPlans, isTrue);
      expect(ports.specs.last.start, const InstallStart.bank0());
    });

    for (final (stop, title, resume, button) in [
      (
        const StopCase.noFeederCame(),
        'No feeder connected',
        Resume.fromStart,
        'Start again',
      ),
      (
        const StopCase.stockRefused(code: 7),
        'The feeder refused the install',
        Resume.fromStart,
        'Start again',
      ),
      (
        const StopCase.noSetupFeeder(),
        'No feeder is waiting over Bluetooth',
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.setupFailed(refusal: SetupRefusal.wrongCode),
        "This phone couldn't claim it",
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.setupFailed(),
        "This phone couldn't claim it",
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.notJoined(),
        "It didn't join your Wi-Fi",
        Resume.fromBank0,
        'Enter the Wi-Fi again',
      ),
      (
        const StopCase.otherFeeder(),
        'That was a different feeder',
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.notAnswering(),
        'The feeder stopped answering',
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.promotionFailed(),
        "Wellfed didn't become permanent",
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.fellBack(),
        'It went back after restarting',
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.uncertainWrite(write: UncertainWrite.promotion),
        "We don't know if that last step ran",
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.uncertainWrite(write: UncertainWrite.plans),
        "We don't know if that last step ran",
        Resume.fromBank0,
        'Pick up where it stopped',
      ),
      (
        const StopCase.serverFailed(),
        "This phone couldn't serve the feeder",
        Resume.fromStart,
        'Start again',
      ),
      (
        const StopCase.cancelled(),
        'You stopped the install',
        Resume.fromStart,
        'Start again',
      ),
      (
        const StopCase.other(),
        'The install stopped',
        Resume.nothing,
        'Go to Home',
      ),
      (
        const StopCase.plansUnknown(),
        "We don't know if its meals came along",
        Resume.nothing,
        'Go to Home',
      ),
      (
        const StopCase.plansNotApplied(),
        'Wellfed is installed, but its meals are not',
        Resume.nothing,
        'Go to Home',
      ),
    ]) {
      testWidgets('$title offers "$button"', (tester) async {
        await _stopped(tester, FakeInstallPorts(), stop, resume: resume);
        expect(find.text(title), findsOneWidget);
        expect(find.text(button), findsOneWidget);
        // The run's own words stay in the small print.
        expect(find.text('its words'), findsOneWidget);
      });
    }

    group('a request with no Wi-Fi to give', () {
      /// A fresh launch: an open request listened for, and stock staying.
      Future<void> stockStayed(
        WidgetTester tester,
        FakeInstallPorts ports,
      ) async {
        await _open(tester, ports);
        await _tap(tester, 'Listen for the feeder');
        ports.listeners.single.updates.add(
          const ListenUpdate.stockStayed(resolved: true),
        );
        await settle(tester);
        expect(find.text('Start again'), findsOneWidget);
      }

      testWidgets('Start again after a fresh launch asks for the Wi-Fi', (
        tester,
      ) async {
        final ports = FakeInstallPorts(
          pendingInstall: PendingInstall.requestOpen,
        );
        await stockStayed(tester, ports);

        await _tap(tester, 'Start again');
        expect(ports.runs, isEmpty);
        expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);

        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
        await tester.pump();
        await _tap(tester, 'Continue');
        expect(find.text('Point the feeder at this phone'), findsOneWidget);

        await tester.enterText(find.byType(TextField), 'broker.example.net');
        await tester.pump();
        await _tap(tester, 'Check the record');
        expect(ports.specs.single.home?.ssid, 'Maple St');
        expect(ports.specs.single.start, isA<InstallStart_Dns>());
      });

      testWidgets('a bench image has none to ask for', (tester) async {
        final ports = FakeInstallPorts(
          pendingInstall: PendingInstall.requestOpen,
          image: const ImageCheck.ready(
            provisioning: ImageProvisioning.bench,
            build: 'ab',
            version: 7,
          ),
        );
        await stockStayed(tester, ports);
        await _tap(tester, 'Start again');
        expect(find.text('Point the feeder at this phone'), findsOneWidget);
      });
    });

    group('a feeder that never joined', () {
      Future<FakeRun> notJoined(WidgetTester tester, FakeInstallPorts ports) =>
          _stopped(
            tester,
            ports,
            const StopCase.notJoined(),
            resume: Resume.fromBank0,
          );

      testWidgets('a new Wi-Fi goes through setup again, and says how', (
        tester,
      ) async {
        final ports = FakeInstallPorts();
        await notJoined(tester, ports);
        expect(find.text("It didn't join your Wi-Fi"), findsOneWidget);

        await _tap(tester, 'Enter the Wi-Fi again');
        expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);
        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'rightpassword');
        await tester.pump();
        await _tap(tester, 'Continue');

        expect(ports.specs, hasLength(2));
        final again = ports.specs.last;
        expect(again.start, const InstallStart.bank0());
        expect(again.home?.passphrase, 'rightpassword');
        expect(again.setupAgain, isTrue);

        expect(find.text('Wellfed is running. Now claim it.'), findsOneWidget);
        expect(find.textContaining('Mic', findRichText: true), findsOneWidget);
      });

      testWidgets('a pick-up that finds no feeder keeps going through setup', (
        tester,
      ) async {
        final ports = FakeInstallPorts();
        await notJoined(tester, ports);
        await _tap(tester, 'Enter the Wi-Fi again');
        await tester.enterText(find.byType(TextField).at(0), 'Maple St');
        await tester.enterText(find.byType(TextField).at(1), 'rightpassword');
        await tester.pump();
        await _tap(tester, 'Continue');

        ports.run.stopped(
          const StopCase.noSetupFeeder(),
          resume: Resume.fromBank0,
        );
        await settle(tester);
        await _tap(tester, 'Pick up where it stopped');
        expect(ports.specs.last.setupAgain, isTrue);
      });

      testWidgets('Back from the Wi-Fi leaves setup alone', (tester) async {
        final ports = FakeInstallPorts();
        await notJoined(tester, ports);
        await _tap(tester, 'Enter the Wi-Fi again');
        await tester.tap(find.byTooltip('Back'));
        await settle(tester);
        expect(find.text('Install Wellfed'), findsOneWidget);
        expect(ports.specs, hasLength(1));
      });

      testWidgets('waiting longer keeps the Wi-Fi it has', (tester) async {
        final ports = FakeInstallPorts();
        await notJoined(tester, ports);
        await _tap(tester, 'Keep waiting for it');
        expect(ports.specs.last.start, const InstallStart.bank0());
        expect(ports.specs.last.setupAgain, isFalse);
      });
    });

    group('what a run was given', () {
      testWidgets('a phone off Wi-Fi says so, not that the feeder stopped', (
        tester,
      ) async {
        final ports = FakeInstallPorts();
        await _stopped(
          tester,
          ports,
          const StopCase.phoneOffWifi(),
          resume: Resume.fromStart,
        );
        expect(find.text("This phone isn't on Wi-Fi"), findsOneWidget);
        expect(find.text('The feeder stopped answering'), findsNothing);
        await _tap(tester, 'Start again');
        expect(find.text('Point the feeder at this phone'), findsOneWidget);
      });

      for (final (stop, title) in [
        (const StopCase.wifiRefused(), "That Wi-Fi can't be given to a feeder"),
        (const StopCase.noWifiGiven(), 'This install needs your Wi-Fi'),
      ]) {
        testWidgets(' asks for it again, and goes on from there', (
          tester,
        ) async {
          final ports = FakeInstallPorts();
          await _stopped(tester, ports, stop, resume: Resume.fromStart);
          expect(find.text(title), findsOneWidget);
          expect(find.text('The Bluetooth link dropped'), findsNothing);

          await _tap(tester, 'Enter the Wi-Fi again');
          expect(find.text('Connect it to your Wi-Fi'), findsOneWidget);
          await tester.enterText(find.byType(TextField).at(0), 'Maple St');
          await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
          await tester.pump();
          await _tap(tester, 'Continue');
          expect(find.text('Point the feeder at this phone'), findsOneWidget);
        });
      }
    });

    for (final (why, title) in [
      (RequestRefusal.stale, "The feeder's reports got too old"),
      (RequestRefusal.offline, 'The feeder went offline'),
      (
        RequestRefusal.imageServer,
        "This phone wasn't ready to serve the firmware",
      ),
      (RequestRefusal.other, "The install wasn't sent"),
    ]) {
      testWidgets('a refused request says why: $title', (tester) async {
        final ports = FakeInstallPorts();
        await _stopped(
          tester,
          ports,
          StopCase.requestRefused(why: why),
          resume: Resume.fromStart,
        );
        expect(find.text(title), findsOneWidget);
        expect(find.text('An earlier install is still open'), findsNothing);
        await _tap(tester, 'Start again');
        expect(find.text('Point the feeder at this phone'), findsOneWidget);
      });
    }

    testWidgets('a stop closes the link, so the feeder advertises again', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toRun(tester, ports);
      run.ask(3, const InstallAsk.openLink());
      await settle(tester);
      expect(ports.link.closes, 0);

      run.stopped(const StopCase.setupFailed(), resume: Resume.fromBank0);
      await settle(tester);
      expect(ports.link.closes, 1);
    });

    testWidgets('a pick-up starts its code and checklist over', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _toRun(tester, ports);
      run.event(const InstallEvent.setupStarting());
      run.event(
        InstallEvent.code(
          code: Uint8List.fromList(const [3, 1, 4, 4, 2, 1]),
          seconds: 60,
        ),
      );
      await tester.pump();
      run.stopped(const StopCase.setupFailed(), resume: Resume.fromBank0);
      await settle(tester);

      await _tap(tester, 'Pick up where it stopped');
      expect(find.byType(CodeTiles), findsNothing);
      expect(
        find.text('Looking for the feeder on your Wi-Fi…'),
        findsOneWidget,
      );
      await tester.pump(WaitWords.after);
      await tester.pump();
      expect(
        find.textContaining('Still looking for the feeder', findRichText: true),
        findsOneWidget,
      );
    });

    testWidgets('a stream that closes with no end is a stop, not a hang', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toRun(tester, ports);
      unawaited(run.updates.close());
      await settle(tester);
      expect(find.text('The install stopped'), findsOneWidget);
      expect(find.text('the install stopped answering'), findsOneWidget);
    });

    testWidgets('a listener that closes with no end says so', (tester) async {
      final ports = FakeInstallPorts();
      await _stopped(tester, ports, const StopCase.uncertain());
      unawaited(ports.listeners.single.updates.close());
      await settle(tester);
      expect(find.text("We can't listen for the feeder"), findsOneWidget);
    });

    testWidgets('a stop with a paired feeder saves it for Home', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final book = FakeBook();
      final run = await _toRun(tester, ports, book: book);
      run.event(
        InstallEvent.paired(
          identity: Uint8List.fromList(const [0xab, 0xcd]),
          rebooting: false,
        ),
      );
      await settle(tester);
      expect(book.feeders, isEmpty, reason: 'Home stays out of the run');

      run.stopped(const StopCase.promotionFailed(), resume: Resume.fromBank0);
      await settle(tester);
      expect(book.feeders.single.identity, 'abcd');
    });
  });

  testWidgets('every stage draws in dark too', (tester) async {
    final ports = FakeInstallPorts();
    tester.view.physicalSize = const Size(1080, 6000);
    tester.view.devicePixelRatio = 3;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      _app(FakeBook(), ports, brightness: Brightness.dark),
    );
    await settle(tester);
    expect(find.text('Install Wellfed'), findsOneWidget);

    await _tap(tester, 'Through your router');
    await _tap(tester, 'Continue');
    await tester.enterText(find.byType(TextField).at(0), 'Maple St');
    await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
    await tester.pump();
    await _tap(tester, 'Continue');
    await tester.enterText(find.byType(TextField), 'broker.example.net');
    await tester.pump();
    await _tap(tester, 'Check the record');
    final run = ports.run;

    run.ask(
      1,
      const InstallAsk.recognizeFeeder(device: 'AF', address: '192.168.1.40'),
    );
    await settle(tester);
    expect(tester.takeException(), isNull);
    await _tap(tester, 'Yes, install Wellfed');
    expect(tester.takeException(), isNull);
    run.event(const InstallEvent.setupStarting());
    await settle(tester);
    expect(tester.takeException(), isNull);
    run.event(InstallEvent.paired(identity: Uint8List(2), rebooting: true));
    await settle(tester);
    expect(tester.takeException(), isNull);
    run.end(const InstallEnd.done());
    await settle(tester);
    expect(find.text('Your feeder runs Wellfed.'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a pick-up past the backup still offers to save it', (
    tester,
  ) async {
    final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired)
      ..kept = '/data/kept.bin';
    await _open(tester, ports);
    await _tap(tester, 'Pick up where it stopped');
    final run = ports.run;
    run.event(
      InstallEvent.paired(
        identity: Uint8List.fromList(const [0xab, 0xcd]),
        address: '192.168.1.40:4747',
        rebooting: false,
      ),
    );
    run.end(const InstallEnd.done());
    await settle(tester);
    await _tap(tester, 'Save the backup somewhere else');
    expect(ports.shared, ['/data/kept.bin']);
  });

  testWidgets('a pick-up keeps the name the owner gave the feeder', (
    tester,
  ) async {
    final book = FakeBook();
    await book.remember(const KnownFeeder(identity: 'abcd', name: 'Kitchen'));
    final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired);
    await _open(tester, ports, book: book);
    await _tap(tester, 'Pick up where it stopped');
    final run = ports.run;
    run.event(
      InstallEvent.paired(
        identity: Uint8List.fromList(const [0xab, 0xcd]),
        rebooting: false,
      ),
    );
    run.stopped(const StopCase.promotionFailed(), resume: Resume.fromBank0);
    await settle(tester);
    expect(book.feeders.single.name, 'Kitchen');
  });

  testWidgets('Android refusing the service says to stay on screen', (
    tester,
  ) async {
    final ports = FakeInstallPorts()..holds = false;
    final run = await _toRun(tester, ports);
    run.event(const InstallEvent.requesting(build: 'ab'));
    await settle(tester);
    expect(
      find.textContaining(
        "Android won't keep Wellfed running in the background.",
        findRichText: true,
      ),
      findsOneWidget,
    );
  });

  test(
    'only one run at a time: a second start leaves the first alone',
    () async {
      final ports = FakeInstallPorts();
      final flow = InstallFlow(FakeBook(), ports);
      await flow.load();
      flow.brokerName = 'broker.example.net';
      await flow.startRun();
      await flow.startRun();
      expect(ports.runs, hasLength(1));
      flow.dispose();
    },
  );

  test('the flow reads the APK before it offers anything', () async {
    final ports = FakeInstallPorts(pendingInstall: PendingInstall.paired);
    final flow = InstallFlow(FakeBook(), ports);
    await flow.load();
    expect(flow.stage, InstallStage.before);
    expect(flow.mayStart, isTrue);
    expect(flow.pending, PendingInstall.paired);
    expect(flow.phoneIp, '192.168.1.23');
    flow.dispose();
  });

  test('a disposed flow stops its run', () async {
    final ports = FakeInstallPorts();
    final flow = InstallFlow(FakeBook(), ports);
    await flow.load();
    flow.brokerName = 'broker.example.net';
    await flow.startRun();
    final run = ports.run;
    flow.dispose();
    expect(run.stops, 1);
    expect(ports.releases, 1);
    expect(flow.stage, InstallStage.before);
  });

  group('2.2 and 2.3 the reset route', () {
    testWidgets('Reset waits for the owner to understand what it erases', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      await _open(tester, ports, route: InstallRoute.reset);
      await _tap(tester, 'Continue');
      await tester.enterText(find.byType(TextField).at(0), 'Maple St');
      await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
      await tester.pump();
      await _tap(tester, 'Continue');

      expect(find.text('Reset the feeder'), findsOneWidget);
      await tester.tap(find.text('I held Reset'));
      await settle(tester);
      expect(ports.runs, isEmpty);
    });

    testWidgets('the phone joins, sends once, comes back and waits', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      expect(ports.specs.single.start, const InstallStart.reset());
      expect(ports.specs.single.home?.ssid, 'Maple St');

      run.ask(1, const InstallAsk.eraseStockPlans());
      await settle(tester);
      expect(run.answers[1], const InstallAnswer.yes(yes: true));

      // The broker listens first, then the owner joins.
      run.ask(2, const InstallAsk.joinFeederNetwork());
      await settle(tester);
      expect(run.answers[2], isNull);
      await _tap(tester, "Join the feeder's network");
      expect(ports.joined, 1);
      expect(run.answers[2], const InstallAnswer.done());

      run.ask(3, const InstallAsk.leaveFeederNetwork());
      await settle(tester);
      expect(ports.left, 1);
      expect(run.answers[3], const InstallAnswer.done());
      expect(
        find.text('Waiting for the feeder to join your Wi-Fi…'),
        findsOneWidget,
      );
    });

    testWidgets('an empty picker lets the owner try again', (tester) async {
      final ports = FakeInstallPorts()..joinFails = 'no network';
      final run = await _toHandover(tester, ports);
      run.ask(2, const InstallAsk.joinFeederNetwork());
      await settle(tester);

      await _tap(tester, "Join the feeder's network");
      expect(
        find.textContaining(
          "This phone didn't join the feeder's network.",
          findRichText: true,
        ),
        findsOneWidget,
      );
      expect(run.answers[2], isNull);

      ports.joinFails = null;
      await _tap(tester, "Join the feeder's network");
      expect(run.answers[2], const InstallAnswer.done());
    });

    testWidgets('coming back at another address fails the ask', (tester) async {
      final ports = FakeInstallPorts()..ipAfterLeaving = '192.168.1.99';
      final run = await _toHandover(tester, ports);
      run.ask(3, const InstallAsk.leaveFeederNetwork());
      await settle(tester);
      expect(run.answers[3], isA<InstallAnswer_Failed>());
    });

    testWidgets('a run that ends on the feeder\'s network leaves it', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      run.ask(2, const InstallAsk.joinFeederNetwork());
      await settle(tester);
      await _tap(tester, "Join the feeder's network");

      run.stopped(const StopCase.cancelled(), resume: Resume.fromStart);
      await settle(tester);
      expect(ports.left, 1);
    });

    testWidgets('a frame stock did not keep goes again with no second reset', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      run.stopped(const StopCase.frameNotTaken(), resume: Resume.fromStart);
      await settle(tester);
      expect(find.text("The feeder didn't reach this phone"), findsOneWidget);

      await _tap(tester, 'Try again');
      expect(ports.runs, hasLength(2));
      expect(ports.specs.last.start, const InstallStart.reset());
      expect(find.text('Reset the feeder'), findsNothing);
    });

    testWidgets('a new password goes straight to the handover', (tester) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      run.stopped(const StopCase.frameNotTaken(), resume: Resume.fromStart);
      await settle(tester);

      await _tap(tester, 'Enter the Wi-Fi again');
      await tester.enterText(find.byType(TextField).at(0), 'Maple St');
      await tester.enterText(find.byType(TextField).at(1), 'betterhorse');
      await tester.pump();
      await _tap(tester, 'Continue');
      expect(ports.runs, hasLength(2));
      expect(ports.specs.last.home?.passphrase, 'betterhorse');
    });

    testWidgets('a stock that kept the frame is only waited for', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      run.ask(
        4,
        const InstallAsk.recognizeFeeder(device: 'AF', address: '192.168.1.40'),
      );
      await settle(tester);
      run.stopped(const StopCase.serverFailed(), resume: Resume.fromStart);
      await settle(tester);

      await _tap(tester, 'Start again');
      expect(ports.specs.last.start, const InstallStart.frameTaken());
    });

    testWidgets('a client the owner said no to took no frame of ours', (
      tester,
    ) async {
      final ports = FakeInstallPorts();
      final run = await _toHandover(tester, ports);
      run.ask(
        4,
        const InstallAsk.recognizeFeeder(device: 'AF', address: '192.168.1.40'),
      );
      await settle(tester);
      await _tap(tester, "That's not my feeder");
      run.stopped(const StopCase.notYourFeeder(), resume: Resume.fromStart);
      await settle(tester);

      await _tap(tester, 'Start again');
      expect(ports.specs.last.start, const InstallStart.reset());
    });
  });
}

/// From the first screen to the reset route's broker running.
Future<FakeRun> _toHandover(WidgetTester tester, FakeInstallPorts ports) async {
  await _open(tester, ports, route: InstallRoute.reset);
  await _tap(tester, 'Continue');
  await tester.enterText(find.byType(TextField).at(0), 'Maple St');
  await tester.enterText(find.byType(TextField).at(1), 'correcthorse');
  await tester.pump();
  await _tap(tester, 'Continue');
  await _tap(tester, 'I understand its meals will be erased');
  await _tap(tester, 'I held Reset');
  return ports.run;
}
