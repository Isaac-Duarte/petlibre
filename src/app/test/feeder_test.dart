import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/app.dart';
import 'package:wellfed/screens/feeder/shell.dart';
import 'package:wellfed/screens/home.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/install.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/session.dart';
import 'package:wellfed/src/rust/api/store.dart';
import 'package:wellfed/state/feeder.dart';
import 'package:wellfed/state/fleet.dart';
import 'package:wellfed/theme/theme.dart';

import 'fakes.dart';

const _identity = 'aa';
const _kitchen = KnownFeeder(
  identity: _identity,
  name: 'Kitchen',
  address: 'lan:4747',
);

/// One known feeder, its session opened.
Future<(FeederFleet, FakeSession)> _fleet(WidgetTester tester) async {
  final book = FakeBook()..feeders = [_kitchen];
  final ports = FakeFeederPorts();
  final fleet = FeederFleet(book, ports);
  addTearDown(fleet.dispose);
  await tester.pump();
  ports.session.opened();
  await tester.pump();
  return (fleet, ports.session);
}

FeederController _only(FeederFleet fleet) => fleet.all.single;

/// A phone-sized frame, so the whole tab fits as the board draws it.
void _phone(WidgetTester tester) {
  tester.view.physicalSize = const Size(412, 915);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
}

Future<void> _showShell(WidgetTester tester, FeederFleet fleet) async {
  _phone(tester);
  await tester.pumpWidget(themed(FeederShell(feeder: _only(fleet))));
  await settle(tester);
}

/// Tap and let the fake's answer come back.
Future<void> _tap(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await tester.pump();
  await tester.tap(target);
  await settle(tester);
}

void main() {
  testWidgets('Today says how the feeder stands, as the feeder said it', (
    tester,
  ) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);

    final fedAt = DateTime(2026, 10, 30, 7, 30).millisecondsSinceEpoch;
    session.status(
      fakeCard(
        nextMeal: NextMealView(
          atMs: DateTime(2026, 10, 30, 12, 30).millisecondsSinceEpoch,
          minute: 12 * 60 + 30,
          portions: 2,
        ),
        lastFed: fakeFed(atMs: fedAt),
        time: TimeInfo(
          generation: 1,
          writable: true,
          ntp: const NtpView.gateway(port: 123),
          syncs: 3,
          last: TimeSource.ntp,
          lastAtMs: DateTime(2026, 10, 30, 11, 58).millisecondsSinceEpoch,
        ),
      ),
    );
    await settle(tester);

    for (final tab in ['Today', 'Schedule', 'History', 'Settings']) {
      expect(find.text(tab), findsOneWidget);
    }
    expect(find.text('Ready to feed'), findsOneWidget);
    expect(find.text('Next meal 12:30 PM · 2 portions'), findsOneWidget);
    expect(find.text('Last fed 7:30 AM · 2 of 2'), findsOneWidget);
    expect(find.text('In the hopper at 7:30 AM'), findsOneWidget);
    expect(find.text('Mains · batteries 90%'), findsOneWidget);
    expect(find.text('Strong'), findsOneWidget);
    expect(find.text('Set by your router at 11:58 AM'), findsOneWidget);
    expect(find.text('Change in 9 days'), findsOneWidget);
    expect(find.text('Changed today'), findsNothing);
  });

  testWidgets('a low reading says low, and never that the hopper is empty', (
    tester,
  ) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);

    session.status(fakeCard(food: FoodLevel.low));
    await settle(tester);

    expect(find.textContaining('Low'), findsWidgets);
    expect(find.textContaining('empty'), findsNothing);
    expect(find.textContaining('Hopper'), findsNothing);
  });

  testWidgets('a low reading at rest borrows no time from the last feed', (
    tester,
  ) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);

    // The last feed read food; the level has gone low since, with no event.
    final fedAt = DateTime(2026, 10, 30, 7, 30).millisecondsSinceEpoch;
    session.status(fakeCard(food: FoodLevel.low, lastFed: fakeFed(atMs: fedAt)));
    await settle(tester);

    expect(find.text('Low'), findsOneWidget);
    expect(find.text('Low at 7:30 AM'), findsNothing);
    expect(find.textContaining('read low at'), findsNothing);
  });

  testWidgets('an uncertain last feed is a range, never one number', (
    tester,
  ) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);

    session.status(
      fakeCard(
        lastFed: fakeFed(
          confirmed: 0,
          atMost: 2,
          outcome: FeedOutcome.uncertain,
          reason: FeedReason.interrupted,
        ),
      ),
    );
    await settle(tester);

    expect(find.text('Last fed 0 to 2 of 2'), findsOneWidget);
  });

  group('the lock', () {
    Future<FakeSession> locked(WidgetTester tester) async {
      final (fleet, session) = await _fleet(tester);
      await _showShell(tester, fleet);
      session.status(fakeCard(locked: false));
      await settle(tester);
      return session;
    }

    testWidgets('goes out once and follows what the feeder says', (
      tester,
    ) async {
      final session = await locked(tester);

      await _tap(tester, find.byType(Switch));
      expect(session.requests, [const FeederRequest.setLock(locked: true)]);
      // Not moved by asking: only the status says it is locked.
      expect(tester.widget<Switch>(find.byType(Switch)).value, isFalse);

      session.status(fakeCard(locked: true));
      await settle(tester);
      expect(tester.widget<Switch>(find.byType(Switch)).value, isTrue);
    });

    testWidgets('says why it was refused', (tester) async {
      final session = await locked(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.forbidden()),
      );

      await _tap(tester, find.byType(Switch));
      expect(
        find.text("This phone isn't allowed to do that here."),
        findsOneWidget,
      );
    });

    testWidgets('says an unanswered change is unknown, and sends it once', (
      tester,
    ) async {
      final session = await locked(tester);
      session.replies.add(const FeederReply.unanswered());

      await _tap(tester, find.byType(Switch));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      await settle(tester);
      expect(session.requests, hasLength(1));
    });
  });

  group('the desiccant', () {
    Future<FakeSession> due(WidgetTester tester) async {
      final (fleet, session) = await _fleet(tester);
      await _showShell(tester, fleet);
      session.status(
        fakeCard(desiccant: const Desiccant.remaining(days: -3, due: true)),
      );
      await settle(tester);
      return session;
    }

    testWidgets('records a change when it is due', (tester) async {
      final session = await due(tester);

      expect(find.text('3 days overdue'), findsOneWidget);
      await _tap(tester, find.text('Changed today'));
      expect(session.requests, [const FeederRequest.resetDesiccant()]);
    });

    testWidgets('says why the feeder refused it, or that it may have run', (
      tester,
    ) async {
      final session = await due(tester);
      session.replies
        ..add(const FeederReply.refused(refusal: FeederRefusal.noClock()))
        ..add(const FeederReply.unanswered());

      await _tap(tester, find.text('Changed today'));
      expect(find.textContaining('Set its time first'), findsOneWidget);

      await _tap(tester, find.text('Changed today'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(session.requests, hasLength(2));
    });
  });

  group('a fault', () {
    Future<FakeSession> stopped(WidgetTester tester) async {
      final (fleet, session) = await _fleet(tester);
      await _showShell(tester, fleet);
      session.status(
        fakeCard(
          ready: false,
          blocked: Blocked.fault,
          fault: FaultView(
            feed: 7,
            outcome: FeedOutcome.failed,
            reason: FeedReason.jammed,
            result: fakeFed(
              confirmed: 1,
              outcome: FeedOutcome.failed,
              reason: FeedReason.jammed,
              atMs: DateTime(2026, 10, 30, 7, 30).millisecondsSinceEpoch,
            ),
          ),
        ),
      );
      await settle(tester);
      return session;
    }

    testWidgets('holds feeding, says what stopped it, and offers a restart', (
      tester,
    ) async {
      final session = await stopped(tester);

      expect(find.text("Can't feed"), findsOneWidget);
      expect(find.text('Meals are held until it restarts'), findsOneWidget);
      expect(find.text('The wheel jammed at 7:30 AM'), findsOneWidget);
      expect(
        find.textContaining('It stopped after 1 of 2 portions.'),
        findsOneWidget,
      );
      final feed = find.widgetWithText(
        FilledButton,
        'Feed now · unavailable until restart',
      );
      expect(tester.widget<FilledButton>(feed).onPressed, isNull);

      await _tap(tester, find.text('Restart it from here'));
      expect(session.requests, isEmpty, reason: 'it asks first');
      await _tap(tester, find.widgetWithText(FilledButton, 'Restart'));
      expect(session.requests, [const FeederRequest.reboot()]);
      expect(find.text('Kitchen is restarting.'), findsOneWidget);
    });

    testWidgets('a restart refused or unanswered says so', (tester) async {
      final session = await stopped(tester);
      session.replies
        ..add(const FeederReply.refused(refusal: FeederRefusal.busy()))
        ..add(const FeederReply.unanswered());

      await _tap(tester, find.text('Restart it from here'));
      await _tap(tester, find.widgetWithText(FilledButton, 'Restart'));
      expect(find.textContaining("It's busy"), findsOneWidget);

      await _tap(tester, find.text('Restart it from here'));
      await _tap(tester, find.widgetWithText(FilledButton, 'Restart'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(find.text('Kitchen is restarting.'), findsNothing);
      expect(session.requests, hasLength(2));
    });
  });

  testWidgets('the lights help names every light and button', (tester) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);
    session.status(fakeCard());
    await settle(tester);

    await _tap(tester, find.text('What do the lights on the feeder mean?'));

    expect(find.text('Lights and buttons'), findsOneWidget);
    for (final mark in ['A', 'B', 'C', 'D', '1', '2', '3', '4']) {
      expect(find.text(mark), findsNWidgets(2), reason: mark);
    }
  });

  testWidgets('the restart is in the menu too', (tester) async {
    final (fleet, session) = await _fleet(tester);
    await _showShell(tester, fleet);
    session.status(fakeCard());
    await settle(tester);

    await _tap(tester, find.byTooltip('More'));
    await _tap(tester, find.text('Restart'));
    await _tap(tester, find.widgetWithText(FilledButton, 'Restart'));
    expect(session.requests, [const FeederRequest.reboot()]);
  });

  group('Home', () {
    Future<(FeederFleet, FakeSession)> home(
      WidgetTester tester, {
      PendingInstall earlier = PendingInstall.nothing,
    }) async {
      _phone(tester);
      final (fleet, session) = await _fleet(tester);
      final router = wellfedRouter(
        book: FakeBook(),
        root: (context) =>
            HomeScreen(fleet: fleet, earlierInstall: () async => earlier),
        pages: const {},
        feeder: (context, identity, tab) =>
            FeederShell.of(fleet, identity, initialTab: tab),
      );
      addTearDown(router.dispose);
      await tester.pumpWidget(
        MaterialApp.router(
          theme: wellfedTheme(Brightness.light),
          routerConfig: router,
        ),
      );
      await settle(tester);
      return (fleet, session);
    }

    testWidgets('says when an earlier install did not finish', (tester) async {
      await home(tester);
      expect(find.text('Finish installing'), findsNothing);

      await home(tester, earlier: PendingInstall.values.last);
      expect(find.text('Finish installing'), findsOneWidget);
    });

    testWidgets("shows the next meal, the last feed and today's meals", (
      tester,
    ) async {
      final (_, session) = await home(tester);
      final now = DateTime.now();
      session.status(
        fakeCard(
          nextMeal: NextMealView(
            atMs: now.add(const Duration(minutes: 26)).millisecondsSinceEpoch,
            minute: 12 * 60 + 30,
            portions: 2,
          ),
          lastFed: fakeFed(
            atMs: DateTime(2026, 10, 30, 7, 30).millisecondsSinceEpoch,
          ),
          meals: [
            MealView(
              slot: 0,
              minute: 7 * 60 + 30,
              portions: 2,
              call: 0,
              standing: MealStanding.fed(result: fakeFed()),
            ),
            MealView(
              slot: 1,
              minute: 12 * 60 + 30,
              portions: 2,
              call: 0,
              standing: MealStanding.next(
                atMs: now
                    .add(const Duration(minutes: 26, seconds: 30))
                    .millisecondsSinceEpoch,
              ),
            ),
            const MealView(
              slot: 2,
              minute: 18 * 60 + 30,
              portions: 3,
              call: 1,
              standing: MealStanding.later(),
            ),
          ],
        ),
      );
      await settle(tester);

      expect(find.text('Next meal'), findsOneWidget);
      expect(find.text('Last fed'), findsOneWidget);
      expect(find.text('2 of 2'), findsOneWidget);
      expect(find.text("Today's meals"), findsOneWidget);
      expect(find.text('Fed'), findsOneWidget);
      expect(find.text('in 26 min'), findsOneWidget);
      expect(find.text('3 portions · meal call'), findsOneWidget);
      expect(find.text('Later'), findsOneWidget);
    });

    testWidgets('the card opens the feeder, and Schedule its tab', (
      tester,
    ) async {
      final (_, session) = await home(tester);
      session.status(
        fakeCard(
          meals: const [
            MealView(
              slot: 2,
              minute: 18 * 60 + 30,
              portions: 3,
              call: 0,
              standing: MealStanding.later(),
            ),
          ],
        ),
      );
      await settle(tester);

      await _tap(tester, find.widgetWithText(TextButton, 'Schedule'));
      final tabs = DefaultTabController.of(tester.element(find.byType(TabBar)));
      expect(tabs.index, 1);
    });

    testWidgets("a feeder with no clock offers this phone's", (tester) async {
      final (_, session) = await home(tester);
      session.status(fakeCard(clockSet: false));
      await settle(tester);

      expect(
        find.textContaining('Meals are on hold.', findRichText: true),
        findsOneWidget,
      );
      await _tap(tester, find.text('Set the time from this phone'));
      expect(session.requests, [const FeederRequest.syncClock()]);
    });
  });

  group('Feed now', () {
    Future<FakeSession> sheet(WidgetTester tester) async {
      final (fleet, session) = await _fleet(tester);
      await _showShell(tester, fleet);
      session.status(fakeCard());
      await settle(tester);
      await _tap(tester, find.widgetWithText(FilledButton, 'Feed now'));
      return session;
    }

    testWidgets('goes out once, and a refusal offers a new start', (
      tester,
    ) async {
      final session = await sheet(tester);
      expect(find.textContaining("Today's meals still run."), findsOneWidget);

      await _tap(tester, find.text('Feed 1 portion'));
      expect(session.feeds, [1]);

      session.updates.add(
        const SessionUpdate.feedRefused(refusal: FeedRefusal.busy),
      );
      await settle(tester);
      expect(find.text("Kitchen didn't feed"), findsOneWidget);
      expect(find.text('Start over'), findsOneWidget);
    });

    testWidgets('an unanswered feed is never offered again', (tester) async {
      final session = await sheet(tester);
      await _tap(tester, find.text('Feed 1 portion'));

      session.updates.add(const SessionUpdate.feedUnanswered());
      await settle(tester);
      expect(find.text('Did Kitchen feed?'), findsOneWidget);
      expect(find.text('Start over'), findsNothing);
      expect(session.feeds, [1]);
    });

    testWidgets('an uncertain feed shows a range, and the history', (
      tester,
    ) async {
      final session = await sheet(tester);
      await _tap(tester, find.text('Feed 1 portion'));
      session.updates.add(
        const SessionUpdate.feedStarted(feed: 7, portions: 2),
      );
      await settle(tester);
      session.updates.add(
        SessionUpdate.feedEnded(
          result: fakeFed(
            confirmed: 0,
            atMost: 2,
            outcome: FeedOutcome.uncertain,
            reason: FeedReason.power,
          ),
        ),
      );
      await settle(tester);

      expect(find.text('Fed between 0 and 2 portions'), findsOneWidget);
      expect(find.text('See history'), findsOneWidget);
      expect(find.text('Start over'), findsNothing);
      expect(session.feeds, [1]);
    });
  });

  test('a request with no session open never went out', () async {
    final book = FakeBook()..feeders = [_kitchen];
    final ports = FakeFeederPorts();
    // Not opened yet in the first microtask: no session, nothing sent.
    final controller = FeederController(book, ports, _kitchen);
    final reply = await controller.ask(const FeederRequest.reboot());
    expect(reply, const FeederReply.notSent());
    controller.dispose();
  });
}
