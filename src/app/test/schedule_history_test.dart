import 'package:flutter/foundation.dart' show Uint8List;
import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/screens/feeder/feed_detail.dart';
import 'package:wellfed/screens/feeder/history.dart';
import 'package:wellfed/screens/feeder/meal_call.dart';
import 'package:wellfed/screens/feeder/schedule.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/session.dart';
import 'package:wellfed/state/feeder.dart';

import 'fakes.dart';
import 'harness.dart';

PlanView _plan(
  int minute, {
  int portions = 2,
  List<Weekday> days = Weekday.values,
  bool enabled = true,
  int call = 0,
}) => PlanView(
  minute: minute,
  portions: portions,
  days: days,
  enabled: enabled,
  call: call,
);

PlansView _plans(List<PlanView?> filled, {int generation = 3}) => PlansView(
  generation: generation,
  writable: true,
  plans: [...filled, ...List.filled(10 - filled.length, null)],
);

void main() {
  group('Schedule', () {
    Future<FakeSession> show(
      WidgetTester tester,
      PlansView plans, {
      bool clockSet = true,
    }) async {
      final (feeder, session) = await oneFeeder(tester);
      session.status(fakeCard(plans: plans, clockSet: clockSet));
      await showTab(tester, ScheduleTab(feeder: feeder));
      return session;
    }

    testWidgets('lists the meals by time, with their days and calls', (
      tester,
    ) async {
      await show(
        tester,
        _plans([
          _plan(18 * 60 + 30, portions: 3, call: 2),
          _plan(7 * 60 + 30),
          _plan(
            12 * 60 + 30,
            days: const [
              Weekday.mon,
              Weekday.tue,
              Weekday.wed,
              Weekday.thu,
              Weekday.fri,
            ],
          ),
          _plan(22 * 60, portions: 1, days: const [], enabled: false),
        ]),
      );

      expect(find.text('4 of 10 meals'), findsOneWidget);
      expect(find.text('2 portions · every day'), findsOneWidget);
      expect(find.text('2 portions · Mon to Fri'), findsOneWidget);
      expect(
        find.text('3 portions · every day · meal call twice'),
        findsOneWidget,
      );
      expect(find.text('1 portion · once'), findsOneWidget);
      final first = tester.getTopLeft(find.text('7:30 AM')).dy;
      final last = tester.getTopLeft(find.text('10:00 PM')).dy;
      expect(first, lessThan(last));
    });

    testWidgets('a feeder at its limit offers no new meal', (tester) async {
      await show(tester, _plans([for (var i = 0; i < 10; i++) _plan(i * 60)]));

      expect(find.textContaining('10 of 10 meals'), findsOneWidget);
      final add = find.widgetWithText(
        FilledButton,
        'Add meal · delete one first',
      );
      expect(tester.widget<FilledButton>(add).onPressed, isNull);
    });

    testWidgets('meals on hold offer the time', (tester) async {
      await show(tester, _plans([_plan(60)]), clockSet: false);

      expect(find.text('Set the time from this phone'), findsOneWidget);
    });

    testWidgets('a meal is switched once, against what the feeder said', (
      tester,
    ) async {
      final plans = _plans([_plan(60)]);
      final session = await show(tester, plans);

      await tapIn(tester, find.byType(Switch));
      expect(session.requests, [
        FeederRequest.editPlans(
          plans: plans,
          edit: const PlanEdit.enabled(slot: 0, enabled: false),
        ),
      ]);
      expect(tester.widget<Switch>(find.byType(Switch)).value, isTrue);
    });

    testWidgets('switching says why it was refused or may have run', (
      tester,
    ) async {
      final session = await show(tester, _plans([_plan(60)]));
      session.replies
        ..add(const FeederReply.refused(refusal: FeederRefusal.busy()))
        ..add(const FeederReply.unanswered());

      await tapIn(tester, find.byType(Switch));
      expect(find.textContaining("It's busy"), findsOneWidget);
      await tapIn(tester, find.byType(Switch));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(session.requests, hasLength(2));
    });

    testWidgets('a new meal goes in as an add', (tester) async {
      final plans = _plans([_plan(60)]);
      final session = await show(tester, plans);

      await tapIn(tester, find.text('Add meal'));
      await tapIn(tester, find.byTooltip('One more'));
      await tapIn(tester, find.text('Weekends'));
      await tapIn(tester, find.widgetWithText(ChoiceChip, '2'));
      await tapIn(tester, find.text('Save'));

      final added = session.requests.single as FeederRequest_EditPlans;
      expect(added.plans, plans);
      final plan = (added.edit as PlanEdit_Add).plan;
      expect(plan.minute, 8 * 60);
      expect(plan.portions, 2);
      expect(plan.days, [Weekday.sat, Weekday.sun]);
      expect(plan.call, 2);
      expect(plan.enabled, isTrue);
      expect(find.text('New meal'), findsNothing, reason: 'it closes');
    });

    testWidgets('an edit changes its slot, or deletes it', (tester) async {
      final plans = _plans([_plan(60, portions: 3)]);
      final session = await show(tester, plans);

      await tapIn(tester, find.text('1:00 AM'));
      await tapIn(tester, find.byTooltip('One fewer'));
      await tapIn(tester, find.text('Save'));
      final change = session.requests.single as FeederRequest_EditPlans;
      final changed = change.edit as PlanEdit_Change;
      expect(changed.slot, 0);
      expect(changed.minute, 60);
      expect(changed.portions, 2);
      expect(changed.days, Weekday.values);
      expect(changed.enabled, isTrue);

      session.requests.clear();
      await tapIn(tester, find.text('1:00 AM'));
      await tapIn(tester, find.text('Delete meal'));
      expect(
        (session.requests.single as FeederRequest_EditPlans).edit,
        const PlanEdit.delete(slot: 0),
      );
    });

    testWidgets('a refused or unanswered save keeps the editor open', (
      tester,
    ) async {
      final session = await show(tester, _plans([_plan(60)]));
      session.replies
        ..add(const FeederReply.refused(refusal: FeederRefusal.forbidden()))
        ..add(const FeederReply.unanswered());

      await tapIn(tester, find.text('1:00 AM'));
      await tapIn(tester, find.text('Save'));
      expect(find.textContaining("isn't allowed"), findsOneWidget);
      expect(find.text('Delete meal'), findsOneWidget);

      await tapIn(tester, find.text('Save'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(find.text('Delete meal'), findsOneWidget);
      expect(session.requests, hasLength(2));
    });

    testWidgets("someone else's change is shown, and only overwritten on a "
        'choice, against what the feeder now holds', (tester) async {
      final session = await show(tester, _plans([_plan(60)]));
      final theirs = _plans([_plan(120)], generation: 4);

      await tapIn(tester, find.text('1:00 AM'));
      session.replies
        ..add(
          const FeederReply.refused(refusal: FeederRefusal.stale(current: 4)),
        )
        ..add(FeederReply.plans(plans: theirs));
      await tapIn(tester, find.text('Save'));

      expect(
        find.text('Someone else changed the meals while you were editing'),
        findsOneWidget,
      );
      expect(find.text('2:00 AM · 2 portions'), findsOneWidget);
      expect(find.text('1:00 AM · 2 portions'), findsOneWidget);
      expect(session.requests, hasLength(2), reason: 'the save, then a read');

      await tapIn(tester, find.text('Save mine over it'));
      final again = session.requests.last as FeederRequest_EditPlans;
      expect(again.plans, theirs);
      expect(session.requests, hasLength(3));
    });

    testWidgets('keeping theirs sends nothing more', (tester) async {
      final session = await show(tester, _plans([_plan(60)]));

      await tapIn(tester, find.text('1:00 AM'));
      session.replies
        ..add(
          const FeederReply.refused(refusal: FeederRefusal.stale(current: 4)),
        )
        ..add(FeederReply.plans(plans: _plans([_plan(120)])));
      await tapIn(tester, find.text('Save'));
      await tapIn(tester, find.text('Keep theirs'));

      expect(session.requests, hasLength(2));
      expect(find.text('Delete meal'), findsNothing);
    });
  });

  group('the meal call', () {
    Future<FakeSession> show(
      WidgetTester tester,
      CallClip clip, {
      PickRecording? pick,
    }) async {
      final (feeder, session) = await oneFeeder(tester);
      session.replies.add(
        FeederReply.call(
          call: CallInfo(
            writable: true,
            clip: clip,
            ms: clip is CallClip_Stored ? 3500 : null,
          ),
        ),
      );
      await showTab(
        tester,
        MealCallScreen(feeder: feeder, pick: pick ?? () async => null),
      );
      return session;
    }

    testWidgets('uploads what was picked, once', (tester) async {
      final session = await show(
        tester,
        const CallClip.none(),
        pick: () async => Uint8List.fromList([1, 2, 3]),
      );
      expect(find.text('No recording stored'), findsOneWidget);
      session.replies.add(
        const FeederReply.call(
          call: CallInfo(
            writable: true,
            clip: CallClip.stored(bytes: 9, crc: 1),
            ms: 3500,
          ),
        ),
      );

      await tapIn(tester, find.text('Choose a recording'));
      expect(session.requests.last, isA<FeederRequest_UploadCall>());
      expect(find.text('Stored: 3.5 s a play'), findsOneWidget);
    });

    testWidgets('a recording refused or unanswered says so, then reads', (
      tester,
    ) async {
      final session = await show(
        tester,
        const CallClip.none(),
        pick: () async => Uint8List.fromList([1]),
      );
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.forbidden()),
      );
      await tapIn(tester, find.text('Choose a recording'));
      expect(find.textContaining("isn't allowed"), findsOneWidget);

      session.requests.clear();
      session.replies.add(const FeederReply.unanswered());
      await tapIn(tester, find.text('Choose a recording'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(
        session.requests.whereType<FeederRequest_UploadCall>(),
        hasLength(1),
      );
      expect(session.requests.last, const FeederRequest.getCall());
    });

    testWidgets('a stored clip can be deleted', (tester) async {
      final session = await show(
        tester,
        const CallClip.stored(bytes: 9, crc: 1),
      );

      await tapIn(tester, find.text('Delete the recording'));
      expect(session.requests, contains(const FeederRequest.deleteCall()));
    });
  });

  group('History', () {
    HistoryEntry entry(int seq, HistoryBody body, {DateTime? at}) =>
        HistoryEntry(
          seq: BigInt.from(seq),
          atMs: at?.millisecondsSinceEpoch,
          body: body,
        );

    Future<(FeederController, FakeSession)> show(
      WidgetTester tester,
      List<HistoryEntry> entries,
    ) async {
      final (feeder, session) = await oneFeeder(tester);
      session.replies.add(
        FeederReply.clients(
          clients: [
            ClientView(
              key: Uint8List(0),
              fingerprint: 'beef',
              label: "Ana's phone",
              role: ClientRole.member,
              you: false,
            ),
          ],
        ),
      );
      session.history(entries);
      await showTab(tester, HistoryTab(feeder: feeder));
      return (feeder, session);
    }

    final now = DateTime.now();

    testWidgets('shows what happened, newest first, under its day', (
      tester,
    ) async {
      await show(tester, [
        entry(
          1,
          const HistoryBody.missed(
            slot: 1,
            dueMs: 0,
            reason: MissedReason.blocked(reason: Blocked.battery),
          ),
          at: now.subtract(const Duration(days: 1)),
        ),
        entry(2, HistoryBody.gap(from: BigInt.from(3), to: BigInt.from(5))),
        entry(3, const HistoryBody.restarted(), at: now),
        entry(
          4,
          HistoryBody.fed(
            result: fakeFed(atMs: now.millisecondsSinceEpoch)
                .copyByClient('beef'),
          ),
          at: now,
        ),
      ]);

      expect(find.text('Today'), findsOneWidget);
      expect(find.text('Yesterday'), findsOneWidget);
      expect(find.text('Fed 2 of 2'), findsOneWidget);
      expect(find.text("Feed now · Ana's phone"), findsOneWidget);
      expect(find.text('Started up'), findsOneWidget);
      expect(find.textContaining('the batteries were too low'), findsOneWidget);
      expect(find.text("Some events here weren't kept"), findsOneWidget);
      final fed = tester.getTopLeft(find.text('Fed 2 of 2')).dy;
      final started = tester.getTopLeft(find.text('Started up')).dy;
      expect(fed, lessThan(started));
    });

    testWidgets('an uncertain feed is a range, also in its detail', (
      tester,
    ) async {
      final uncertain = fakeFed(
        confirmed: 0,
        atMost: 3,
        requested: 3,
        outcome: FeedOutcome.uncertain,
        reason: FeedReason.power,
        atMs: now.millisecondsSinceEpoch,
      );
      await show(tester, [
        entry(1, HistoryBody.fed(result: uncertain), at: now),
      ]);

      expect(find.text('Fed between 0 and 3'), findsOneWidget);
      await tapIn(tester, find.text('Fed between 0 and 3'));
      expect(find.byType(FeedDetail), findsOneWidget);
      expect(find.text('Fed between 0 and 3 portions'), findsOneWidget);
      expect(
        find.text('3 asked for · none confirmed · each could have dropped'),
        findsOneWidget,
      );
      expect(find.text('Motor trace for this feed'), findsOneWidget);
    });

    testWidgets('filters by kind', (tester) async {
      await show(tester, [
        entry(1, HistoryBody.fed(result: fakeFed()), at: now),
        entry(2, const HistoryBody.plansChanged(by: FedBy.feeder()), at: now),
        entry(3, const HistoryBody.lost(count: 2), at: now),
      ]);

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Feeds'));
      expect(find.text('Fed 2 of 2'), findsOneWidget);
      expect(find.text('Meals changed'), findsNothing);

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Changes'));
      expect(find.text('Meals changed'), findsOneWidget);
      expect(find.text('Fed 2 of 2'), findsNothing);

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Problems'));
      expect(find.text("Some events weren't kept"), findsOneWidget);
      expect(find.text('Fed 2 of 2'), findsNothing);
    });
  });
}
