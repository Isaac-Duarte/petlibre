import 'dart:typed_data';

import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/platform/install_ports.dart';
import 'package:wellfed/screens/app_settings.dart';
import 'package:wellfed/screens/feeder/diagnostics.dart';
import 'package:wellfed/screens/feeder/live_log.dart';
import 'package:wellfed/screens/feeder/motor_trace.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/src/rust/api/session.dart';
import 'package:wellfed/src/rust/api/store.dart';

import 'fakes.dart';
import 'harness.dart';

const _counts = Counts(
  uptimeS: 3 * 86400 + 4 * 3600,
  feeds: 143,
  feedsFailed: 2,
  portions: 300,
  presses: 0,
  sessions: 0,
  reconnects: 0,
  dhcpRenewals: 0,
  outages: 0,
  batteryS: 0,
);

const _summary = TraceSummary(
  feed: 142,
  requested: 3,
  completed: 3,
  outcome: FeedOutcome.done,
  reason: FeedReason.done,
  driveMs: 27200,
  samples: 3,
  unit: CurrentUnit.milliAmps,
  currentPeak: 300,
  rising: 3,
  falling: 3,
  beamBreaks: 1,
  reversals: 0,
  stride: 1,
  truncated: false,
);

class _FakeBackups implements BackupPorts {
  final shared = <String>[];

  @override
  Future<String?> kept(String identity) async => '/backups/$identity';

  @override
  Future<void> share(String path) async => shared.add(path);
}

void main() {
  group('Diagnostics', () {
    testWidgets('shows what the feeder counts', (tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.status(fakeCard());
      session.replies
        ..add(
          const FeederReply.stats(
            stats: Stats.counters(
              Counters(
                boot: _counts,
                reset: ResetCause.powerOn,
                lifetime: _counts,
                resets: Resets(
                  powerOn: 40,
                  watchdog: 1,
                  software: 0,
                  brownout: 0,
                  other: 0,
                ),
                handshakeMaxMs: 0,
                handshakeMeanMs: 0,
              ),
            ),
          ),
        )
        ..add(
          const FeederReply.stats(
            stats: Stats.memory(
              Memory(
                heapFree: 200000,
                heapMin: 133120,
                tasks: [],
                tasksTotal: 0,
              ),
            ),
          ),
        )
        ..add(
          FeederReply.stats(
            stats: Stats.flash(
              Flash(
                boot: Erases(storage: Uint32List.fromList([1]), update: 0),
                lifetime: Erases(
                  storage: Uint32List.fromList([3, 40, 7]),
                  update: 0,
                ),
                eventsKept: 0,
                eventsCapacity: 0,
              ),
            ),
          ),
        );

      await showTab(tester, DiagnosticsScreen(feeder: feeder));

      expect(find.text('143'), findsOneWidget);
      expect(find.text('41'), findsOneWidget);
      expect(find.text('2'), findsOneWidget);
      expect(find.text('3 d 4 h'), findsOneWidget);
      expect(find.text('130 KB'), findsOneWidget);
      expect(find.text('40'), findsOneWidget);
    });

    testWidgets('what it refuses is said, not invented', (tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.status(fakeCard());
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.forbidden()),
      );

      await showTab(tester, DiagnosticsScreen(feeder: feeder));

      expect(find.textContaining("isn't allowed"), findsOneWidget);
      expect(find.text('3 d 4 h'), findsNothing);
    });
  });

  group('Motor trace', () {
    testWidgets('shows one feed, with each portion', (tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.replies.add(
        FeederReply.trace(
          trace: TraceView(
            summary: _summary,
            stored: true,
            entries: const [
              TraceEntry.current(atMs: 100, value: 200),
              TraceEntry.mark(atMs: 9000, edges: 1, high: true),
            ],
            portionMs: Uint32List.fromList([9000, 9100, 9100]),
          ),
        ),
      );

      await showTab(tester, MotorTraceScreen(feeder: feeder, feed: 142));

      expect(
        find.text('3 of 3 portions · 27.2 s of motor time'),
        findsOneWidget,
      );
      expect(find.text('9.0 s, 9.1 s, 9.1 s'), findsOneWidget);
      expect(find.text('All portions counted'), findsOneWidget);
    });

    testWidgets('a feed with no trace says so', (tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.notFound()),
      );

      await showTab(tester, MotorTraceScreen(feeder: feeder, feed: 3));

      expect(
        find.text('The feeder kept no trace of this feed.'),
        findsOneWidget,
      );
    });
  });

  group('Live log', () {
    testWidgets('streams what it hears and stops listening when closed', (
      tester,
    ) async {
      final (feeder, session) = await oneFeeder(tester);
      final shared = <String>[];
      await showTab(
        tester,
        LiveLogScreen(feeder: feeder, share: (text) async => shared.add(text)),
      );
      expect(session.requests.single, const FeederRequest.tailLog(after: null));

      session.updates.add(
        const SessionUpdate.log(seq: 1, dropped: 2, lines: ['PLNET1 rssi=-58']),
      );
      await settle(tester);
      expect(find.text('PLNET1 rssi=-58'), findsOneWidget);
      expect(find.text('… 2 lines dropped'), findsOneWidget);

      await tester.tap(find.byTooltip('Share the log'));
      expect(shared.single, contains('PLNET1 rssi=-58'));

      await tester.pumpWidget(const SizedBox());
      expect(feeder.onLog, isNull);
    });

    testWidgets('a refused tail says so', (tester) async {
      final (feeder, session) = await oneFeeder(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.forbidden()),
      );

      await showTab(tester, LiveLogScreen(feeder: feeder));

      expect(find.textContaining("isn't allowed"), findsOneWidget);
    });
  });

  group('App settings', () {
    Future<(FakeBook, _FakeBackups)> settings(WidgetTester tester) async {
      final book = FakeBook()
        ..label = 'My phone'
        ..feeders = [kitchen];
      final backups = _FakeBackups();
      await showTab(tester, AppSettingsScreen(book: book, backups: backups));
      return (book, backups);
    }

    testWidgets('appearance is kept', (tester) async {
      final (book, _) = await settings(tester);

      await tapIn(tester, find.text('Dark'));
      expect(book.appearance, Appearance.dark);
    });

    testWidgets('the name is checked against what a feeder keeps', (
      tester,
    ) async {
      final (book, _) = await settings(tester);

      await tapIn(tester, find.text('Name on feeders'));
      await tester.enterText(find.byType(TextField), 'a' * 17);
      await tapIn(tester, find.widgetWithText(FilledButton, 'Save'));
      expect(find.textContaining('1 to 16 bytes'), findsOneWidget);
      expect(book.label, 'My phone');
    });

    testWidgets('a backup can be handed on', (tester) async {
      final (_, backups) = await settings(tester);

      expect(find.text('1'), findsOneWidget);
      await tapIn(tester, find.byTooltip("Share Kitchen's backup"));
      expect(backups.shared, ['/backups/aa']);
    });

    testWidgets('About says what it is', (tester) async {
      await showTab(tester, AboutScreen(version: () => '0.1.0'));

      expect(find.text('App 0.1.0'), findsOneWidget);
      expect(find.text('Open-source licenses'), findsOneWidget);
      expect(find.textContaining('Not affiliated'), findsOneWidget);
    });
  });
}
