import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/screens/feeder/clock_portion.dart';
import 'package:wellfed/screens/feeder/settings_tab.dart';
import 'package:wellfed/screens/feeder/wifi.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/session.dart';
import 'package:wellfed/state/feeder.dart';

import 'fakes.dart';
import 'harness.dart';

const _settings = SettingsView(
  generation: 5,
  lights: true,
  sound: true,
  volume: 50,
  lightsWindow: Window.between(start: 7 * 60, end: 22 * 60),
  soundWindow: Window.always(),
  desiccantDays: 14,
  mealReminder: MealReminder.before(minutes: 15),
  writable: true,
);

Future<(FeederController, FakeSession)> _settingsTab(
  WidgetTester tester, {
  ClientRole role = ClientRole.owner,
}) async {
  final (feeder, session) = await oneFeeder(tester);
  session.updates.add(SessionUpdate.opened(build: 'test', role: role));
  session.status(fakeCard(settings: _settings));
  await showTab(tester, SettingsTab(feeder: feeder));
  session.requests.clear();
  return (feeder, session);
}

FeederRequest_PutSettings _put(FakeSession session) =>
    session.requests.single as FeederRequest_PutSettings;

void main() {
  group('Settings', () {
    testWidgets('a session opening says which build answered', (tester) async {
      final ports = FakeFeederPorts();
      final feeder = FeederController(
        FakeBook()..feeders = [kitchen],
        ports,
        kitchen,
      );
      addTearDown(feeder.dispose);
      await tester.pump();
      ports.session.opened();
      await tester.pump();
      expect(ports.answers, [(kitchen.identity, 'test')]);
    });

    testWidgets('shows what the feeder holds', (tester) async {
      await _settingsTab(tester);

      expect(find.text('7:00 AM to 10:00 PM'), findsOneWidget);
      expect(find.text('Any time'), findsOneWidget);
      expect(find.text('50'), findsOneWidget);
      expect(find.text('15 minutes ahead'), findsOneWidget);
      expect(find.text('Desiccant every 14 days'), findsOneWidget);
      expect(find.text('Next change in 9 days'), findsOneWidget);
      for (final row in ['Wi-Fi', 'Time zone', 'People', 'Firmware']) {
        expect(find.text(row), findsOneWidget);
      }
      expect(find.text('Diagnostics'), findsOneWidget);
    });

    testWidgets('a member sees what a member may change', (tester) async {
      await _settingsTab(tester, role: ClientRole.member);

      expect(find.text('OWNER ONLY'), findsOneWidget);
      expect(find.text('Restart'), findsNothing);
      expect(find.text('Diagnostics'), findsNothing);
      expect(find.text('Firmware updates'), findsOneWidget);
    });

    testWidgets('a toggle goes against the generation it read', (tester) async {
      final (_, session) = await _settingsTab(tester);

      await tapIn(tester, find.byType(Switch).first);
      final put = _put(session);
      expect(put.base, 5);
      expect(put.changes.lights, isFalse);
      expect(put.changes.sound, isNull);
    });

    testWidgets('a change refused as stale, or unanswered, says so', (
      tester,
    ) async {
      final (_, session) = await _settingsTab(tester);
      session.replies
        ..add(
          const FeederReply.refused(refusal: FeederRefusal.stale(current: 6)),
        )
        ..add(const FeederReply.unanswered());

      await tapIn(tester, find.byType(Switch).first);
      expect(find.textContaining('This changed just now'), findsOneWidget);
      await tapIn(tester, find.byType(Switch).first);
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(session.requests, hasLength(2));
    });

    testWidgets('a window is chosen in its dialog', (tester) async {
      final (_, session) = await _settingsTab(tester);

      await tapIn(tester, find.text('Lights'));
      expect(find.text('When can the lights be on?'), findsOneWidget);
      await tapIn(tester, find.text('Any time').last);
      await tapIn(tester, find.widgetWithText(FilledButton, 'Save'));

      expect(_put(session).changes.lightsWindow, const Window.always());
    });

    testWidgets('a muted test sound says why', (tester) async {
      final (_, session) = await _settingsTab(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.muted()),
      );

      await tapIn(tester, find.byTooltip('Play a test sound'));
      expect(
        session.requests.single,
        const FeederRequest.play(tune: Tune.test),
      );
      expect(find.textContaining('Sound is off'), findsOneWidget);
    });

    testWidgets('removing asks first, and goes back', (tester) async {
      final (feeder, session) = await _settingsTab(tester);

      await tapIn(tester, find.text('Remove from this phone'));
      expect(find.textContaining('stays on its People list'), findsOneWidget);
      await tapIn(tester, find.text('Cancel'));
      expect(session.requests, isEmpty);
      expect(feeder.known.name, 'Kitchen');
    });
  });

  group('Wi-Fi', () {
    Future<FakeSession> wifi(WidgetTester tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.status(fakeCard());
      await showTab(tester, WifiScreen(feeder: feeder));
      await tester.enterText(find.byType(TextField).first, 'Maple St 5G');
      await tester.enterText(find.byType(TextField).last, 'correcthorse');
      await settle(tester);
      return session;
    }

    testWidgets('saves once, then restarts to join', (tester) async {
      final session = await wifi(tester);

      await tapIn(tester, find.text('Switch network'));
      expect(session.requests, hasLength(2));
      final put = session.requests.first as FeederRequest_PutWifi;
      expect(put.ssid, 'Maple St 5G');
      expect(put.passphrase, 'correcthorse');
      expect(session.requests.last, const FeederRequest.reboot());
    });

    testWidgets('what Rust or the feeder refuses restarts nothing', (
      tester,
    ) async {
      final session = await wifi(tester);
      session.replies
        ..add(const FeederReply.notAsked(reason: NotAsked.credentials))
        ..add(const FeederReply.refused(refusal: FeederRefusal.forbidden()));

      await tapIn(tester, find.text('Switch network'));
      expect(find.textContaining('A network name is'), findsOneWidget);
      expect(session.requests, hasLength(1));

      await tapIn(tester, find.text('Switch network'));
      expect(find.textContaining("isn't allowed"), findsOneWidget);
      expect(session.requests, hasLength(2));
      expect(session.requests, isNot(contains(const FeederRequest.reboot())));
    });

    testWidgets('an unanswered save restarts nothing and is not sent again', (
      tester,
    ) async {
      final session = await wifi(tester);
      session.replies.add(const FeederReply.unanswered());

      await tapIn(tester, find.text('Switch network'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(session.requests, hasLength(1));
    });

    testWidgets('forgetting asks first, goes once, and says if unanswered', (
      tester,
    ) async {
      final session = await wifi(tester);
      session.replies.add(const FeederReply.unanswered());

      await tapIn(tester, find.text('Forget this network'));
      expect(session.requests, isEmpty);
      await tapIn(tester, find.widgetWithText(FilledButton, 'Forget'));

      expect(session.requests.single, isA<FeederRequest_ForgetWifi>());
      expect(find.textContaining('may or may not have'), findsOneWidget);
    });

    testWidgets('a restart that fails says the network is saved', (
      tester,
    ) async {
      final session = await wifi(tester);
      session.replies
        ..add(const FeederReply.done(generation: 2))
        ..add(const FeederReply.refused(refusal: FeederRefusal.busy()));

      await tapIn(tester, find.text('Switch network'));
      expect(find.textContaining('The network is saved'), findsOneWidget);
    });
  });

  group('Clock and portion size', () {
    const time = TimeInfo(
      generation: 8,
      writable: true,
      ntp: NtpView.gateway(port: 123),
      clockMs: 1000,
      errorMs: 400,
      syncs: 1,
      last: TimeSource.ntp,
    );
    const calibration = CalibrationInfo(
      generation: 2,
      writable: true,
      portionMg: 11000,
      bowl: Bowl.single,
    );

    Future<FakeSession> show(WidgetTester tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.status(fakeCard(time: time));
      session.replies.add(
        const FeederReply.calibration(calibration: calibration),
      );
      await showTab(tester, ClockPortionScreen(feeder: feeder));
      session.requests.clear();
      return session;
    }

    testWidgets('shows the clock and the portion, with no fact it lacks', (
      tester,
    ) async {
      await show(tester);

      expect(find.text('under 1 s'), findsOneWidget);
      expect(find.text('Set by your router'), findsOneWidget);
      expect(find.text('11 g'), findsOneWidget);
    });

    testWidgets('where the time comes from goes against its generation', (
      tester,
    ) async {
      final session = await show(tester);

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Phones only'));
      final put = session.requests.single as FeederRequest_PutTime;
      expect(put.base, 8);
      expect(put.ntp, 'off');
      expect(put.zone, isNull);
    });

    testWidgets('a bad address and an unanswered change say so', (
      tester,
    ) async {
      final session = await show(tester);
      session.replies
        ..add(const FeederReply.notAsked(reason: NotAsked.badNtp))
        ..add(const FeederReply.unanswered());

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Address'));
      await tester.enterText(find.byType(TextField), 'pool.ntp.org');
      await tapIn(tester, find.widgetWithText(FilledButton, 'Save'));
      expect(find.textContaining('IPv4 address, like'), findsOneWidget);

      await tapIn(tester, find.widgetWithText(ChoiceChip, 'Phones only'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
    });

    testWidgets('a portion steps, or comes from what 10 weighed', (
      tester,
    ) async {
      final session = await show(tester);

      await tapIn(tester, find.byTooltip('More'));
      var put = session.requests.first as FeederRequest_PutCalibration;
      expect(put.base, 2);
      expect((put.portion as PortionChange_Milligrams).mg, 12000);
      expect(put.bowl, Bowl.single);

      session.requests.clear();
      await tapIn(tester, find.text('I weighed 10 portions'));
      await tester.enterText(find.byType(TextField), '98.4');
      await tapIn(tester, find.widgetWithText(FilledButton, 'Save'));
      put = session.requests.first as FeederRequest_PutCalibration;
      final weighed = put.portion as PortionChange_Weighed;
      expect(weighed.grams, '98.4');
      expect(weighed.portions, 10);
    });

    testWidgets('a portion refused says so', (tester) async {
      final session = await show(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.invalid()),
      );

      await tapIn(tester, find.byTooltip('More'));
      expect(find.textContaining('refused that value'), findsOneWidget);
    });
  });
}
