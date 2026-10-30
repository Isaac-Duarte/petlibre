import 'package:flutter/material.dart' show SizedBox;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/screens/feeder/updates.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/install.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/update.dart';
import 'package:wellfed/state/feeder.dart';

import 'fakes.dart';
import 'harness.dart';

const _newBuild =
    'cd00000000000000000000000000000000000000000000000000000000000000';

Future<(FakeFeederPorts, FakeSession)> _show(
  WidgetTester tester, {
  ImageCheck image = const ImageCheck.ready(
    provisioning: ImageProvisioning.consumer,
    build: _newBuild,
    version: 7,
  ),
  String build = 'ab',
}) async {
  final book = FakeBook()..feeders = [kitchen];
  final ports = FakeFeederPorts()..image = image;
  final feeder = FeederController(book, ports, kitchen);
  ports.controller = feeder;
  addTearDown(feeder.dispose);
  await tester.pump();
  ports.session.opened();
  ports.session.status(fakeCard(build: build));
  await tester.pump();
  ports.session.replies.add(
    const FeederReply.update(
      update: UpdateInfo(
        phase: UpdatePhase.idle,
        bank: 1,
        minVersion: 6,
        version: 0,
        imageLen: 0,
        written: 0,
      ),
    ),
  );
  await showTab(tester, UpdatesScreen(feeder: feeder));
  ports.session.requests.clear();
  return (ports, ports.session);
}

void main() {
  testWidgets('offers what the app carries, and nothing it does not', (
    tester,
  ) async {
    await _show(tester);
    expect(find.text('7'), findsOneWidget);
    expect(find.text('Install on Kitchen'), findsOneWidget);
  });

  testWidgets('an app with no image offers nothing', (tester) async {
    await _show(tester, image: const ImageCheck.absent());
    expect(find.textContaining('carries no update'), findsOneWidget);
    expect(find.text('Install on Kitchen'), findsNothing);
  });

  testWidgets('the running build is up to date', (tester) async {
    await _show(tester, build: _newBuild);
    expect(find.textContaining('runs the version'), findsOneWidget);
    expect(find.text('Install on Kitchen'), findsNothing);
  });

  testWidgets('one press sends once and shows its progress', (tester) async {
    final (ports, _) = await _show(tester);

    await tapIn(tester, find.text('Install on Kitchen'));
    expect(ports.pushes, ['lan:4747']);
    expect(find.text('Install on Kitchen'), findsNothing);

    ports.progress.add(const UpdateProgress.sending(written: 62, length: 100));
    await settle(tester);
    expect(find.text('Sending 62%'), findsOneWidget);

    ports.progress.add(const UpdateProgress.activated());
    await settle(tester);
    expect(find.text('Cancel update'), findsNothing);

    ports.progress.add(const UpdateProgress.ended(end: UpdateEnd.confirmed()));
    await settle(tester);
    expect(find.text('Updated'), findsOneWidget);
    expect(ports.pushes, hasLength(1));
  });

  testWidgets('cancel goes once; refused or unanswered, it says so', (
    tester,
  ) async {
    final (ports, session) = await _show(tester);
    await tapIn(tester, find.text('Install on Kitchen'));
    ports.progress.add(const UpdateProgress.sending(written: 1, length: 100));
    await settle(tester);

    session.replies.add(const FeederReply.unanswered());
    await tapIn(tester, find.text('Cancel update'));
    expect(session.requests.single, const FeederRequest.cancelUpdate());
    expect(find.textContaining('may or may not have'), findsOneWidget);
  });

  testWidgets('leaving mid-send and coming back shows the run, not an offer', (
    tester,
  ) async {
    final (ports, _) = await _show(tester);
    final feeder = ports.controller;
    await tapIn(tester, find.text('Install on Kitchen'));
    ports.progress.add(const UpdateProgress.sending(written: 62, length: 100));
    await settle(tester);

    await showTab(tester, const SizedBox());
    await showTab(tester, UpdatesScreen(feeder: feeder));

    expect(find.text('Install on Kitchen'), findsNothing);
    expect(find.text('Sending 62%'), findsOneWidget);
    expect(ports.pushes, hasLength(1));
  });

  testWidgets(
    'an uncertain end stays on coming back, until the feeder settles it',
    (tester) async {
      final (ports, session) = await _show(tester);
      final feeder = ports.controller;
      await tapIn(tester, find.text('Install on Kitchen'));
      ports.progress.add(
        const UpdateProgress.ended(end: UpdateEnd.uncertain()),
      );
      await settle(tester);

      await showTab(tester, const SizedBox());
      await showTab(tester, UpdatesScreen(feeder: feeder));
      expect(find.text('Install on Kitchen'), findsNothing);
      expect(find.textContaining('may or may not have'), findsOneWidget);

      session.replies.add(const FeederReply.unanswered());
      await tapIn(tester, find.text('Look again'));
      expect(find.textContaining("hasn't settled"), findsOneWidget);

      session.replies.add(
        const FeederReply.update(
          update: UpdateInfo(
            phase: UpdatePhase.idle,
            bank: 0,
            minVersion: 6,
            version: 0,
            imageLen: 0,
            written: 0,
          ),
        ),
      );
      await tapIn(tester, find.text('Look again'));
      expect(find.text('Install on Kitchen'), findsOneWidget);
      expect(ports.pushes, hasLength(1));
    },
  );

  testWidgets('holds the process for the run, and says if refused', (
    tester,
  ) async {
    final (ports, _) = await _show(tester);
    ports.holdGranted = false;

    await tapIn(tester, find.text('Install on Kitchen'));
    expect(ports.holds, 1);
    expect(find.textContaining('Keep this app open'), findsOneWidget);
    expect(ports.releases, 0);

    ports.progress.add(const UpdateProgress.ended(end: UpdateEnd.confirmed()));
    await settle(tester);
    expect(ports.releases, 1);
  });

  testWidgets('a run that asks where the feeder is hears what NSD says', (
    tester,
  ) async {
    final (ports, _) = await _show(tester);
    await tapIn(tester, find.text('Install on Kitchen'));

    ports.progress.add(
      const UpdateProgress.ask(id: 4, ask: InstallAsk.locate(identity: 'aa')),
    );
    await settle(tester);
    expect(ports.located, [(4, 'lan:4747')]);
  });

  group('how it ends', () {
    Future<void> end(WidgetTester tester, UpdateEnd end) async {
      final (ports, _) = await _show(tester);
      await tapIn(tester, find.text('Install on Kitchen'));
      ports.progress.add(UpdateProgress.ended(end: end));
      await settle(tester);
    }

    testWidgets('on batteries', (tester) async {
      await end(
        tester,
        const UpdateEnd.refused(
          refusal: FeederRefusal.update(reason: UpdateRefusal.battery),
        ),
      );
      expect(find.text('Plug Kitchen in first'), findsOneWidget);
      expect(find.text('Try again'), findsNothing);
    });

    testWidgets('an uncertain one is said, and offers nothing again', (
      tester,
    ) async {
      await end(tester, const UpdateEnd.uncertain());
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(find.text('Try again'), findsNothing);
    });

    testWidgets('an end it can say may be offered again after Done', (
      tester,
    ) async {
      await end(tester, const UpdateEnd.notOffered());
      expect(find.textContaining('Nothing was sent'), findsOneWidget);
      await tapIn(tester, find.text('Done'));
      expect(find.text('Install on Kitchen'), findsOneWidget);
    });

    testWidgets('a cancel and a lost run say which copy it runs', (
      tester,
    ) async {
      await end(tester, const UpdateEnd.stopped(stop: UpdateStop.cancelled));
      expect(find.textContaining('other copy of Wellfed'), findsOneWidget);
      expect(find.textContaining('still runs'), findsNothing);
      expect(find.text('Running now'), findsOneWidget);
    });

    testWidgets('a run that loses its stream is uncertain', (tester) async {
      final (ports, _) = await _show(tester);
      await tapIn(tester, find.text('Install on Kitchen'));
      await ports.progress.close();
      await settle(tester);
      expect(find.textContaining('may or may not have'), findsOneWidget);
    });
  });
}
