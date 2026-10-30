import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../src/rust/api/session.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/when.dart';
import '../home.dart' show blockedWords;
import 'feed_detail.dart';
import 'today.dart' show faultTitle;

enum _Filter { all, feeds, problems, changes }

bool _shown(_Filter filter, HistoryBody body) => switch (filter) {
  _Filter.all => body is! HistoryBody_Other,
  _Filter.feeds => body is HistoryBody_Fed || body is HistoryBody_Missed,
  _Filter.problems => switch (body) {
    HistoryBody_Fed(:final result) =>
      result.outcome != FeedOutcome.done ||
          result.confirmed != result.requested,
    HistoryBody_Food(:final level) => level == FoodLevel.empty,
    HistoryBody_FeedingBlocked(:final reason) => reason != null,
    HistoryBody_Missed() ||
    HistoryBody_DispenseRefused() ||
    HistoryBody_Lost() ||
    HistoryBody_Gap() => true,
    _ => false,
  },
  _Filter.changes => switch (body) {
    HistoryBody_Lock() ||
    HistoryBody_SettingsChanged() ||
    HistoryBody_WifiChanged() ||
    HistoryBody_PlansChanged() ||
    HistoryBody_Joined() ||
    HistoryBody_Revoked() ||
    HistoryBody_ClockSet() => true,
    _ => false,
  },
};

/// Who a client is, from the list the feeder gives; a key it no longer lists
/// keeps a generic name, since the event carries only its fingerprint.
String whoText(FeederController feeder, FedBy by) => switch (by) {
  FedBy_Panel() => 'The Dispense button',
  FedBy_Client(:final fingerprint) => feeder.clientName(fingerprint),
  FedBy_Feeder() => 'The feeder',
  FedBy_Plan() => 'A meal',
  FedBy_Unknown() => 'Unknown',
};

/// What the feeder's history keeps (9.1): the ring, newest first. It is the
/// feeder's own record, read by the session; 9.3's longer history needs a
/// server, and is not here.
class HistoryTab extends StatefulWidget {
  const HistoryTab({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<HistoryTab> createState() => _HistoryTabState();
}

class _HistoryTabState extends State<HistoryTab> {
  _Filter _filter = _Filter.all;

  @override
  void initState() {
    super.initState();
    unawaited(widget.feeder.loadClients());
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.feeder,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final c = context.colors;
    final feeder = widget.feeder;
    final entries = [
      for (final entry in feeder.history.reversed)
        if (_shown(_filter, entry.body)) entry,
    ];
    final rows = <Widget>[];
    String? day;
    for (final (i, entry) in entries.indexed) {
      final at = entry.atMs;
      if (at != null) {
        final heading = dayText(DateTime.fromMillisecondsSinceEpoch(at));
        if (heading != day) {
          day = heading;
          rows.add(_DayHeading(heading, first: i == 0));
        }
      }
      rows.add(_EntryRow(feeder: feeder, entry: entry));
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
          child: Wrap(
            spacing: Space.s2,
            children: [
              for (final (name, filter) in const [
                ('All', _Filter.all),
                ('Feeds', _Filter.feeds),
                ('Problems', _Filter.problems),
                ('Changes', _Filter.changes),
              ])
                ChoiceChip(
                  label: Text(name),
                  selected: _filter == filter,
                  onSelected: (_) => setState(() => _filter = filter),
                ),
            ],
          ),
        ),
        Expanded(
          child: rows.isEmpty
              ? Center(
                  child: Text(
                    feeder.historyRead
                        ? 'Nothing here yet.'
                        : feeder.connection == Connection.online
                        ? 'Reading the history…'
                        : "Can't read the history right now.",
                    style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
                  ),
                )
              : ListView(
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
                  children: rows,
                ),
        ),
      ],
    );
  }
}

class _DayHeading extends StatelessWidget {
  const _DayHeading(this.text, {required this.first});

  final String text;
  final bool first;

  @override
  Widget build(BuildContext context) => Padding(
    padding: EdgeInsets.only(top: first ? 4 : Space.s4, bottom: Space.s1),
    child: Text(
      text,
      style: TextStyle(
        fontSize: Sizes.sm,
        fontWeight: Fonts.semibold,
        color: context.colors.textMuted,
      ),
    ),
  );
}

class _EntryRow extends StatelessWidget {
  const _EntryRow({required this.feeder, required this.entry});

  final FeederController feeder;
  final HistoryEntry entry;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final (glyph, color, title, note) = _words(context, c);
    final at = entry.atMs;
    final body = entry.body;

    return InkWell(
      onTap: body is HistoryBody_Fed
          ? () => unawaited(
              Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) =>
                      FeedDetail(feeder: feeder, result: body.result),
                ),
              ),
            )
          : null,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: Space.s2),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: 64,
              child: Text(
                at == null ? '' : clockTextMs(context, at),
                style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
              ),
            ),
            StrokeIcon(glyph, size: 18, color: color),
            const SizedBox(width: Space.s2),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: TextStyle(
                      fontSize: Sizes.base,
                      fontWeight: Fonts.semibold,
                      color: c.text,
                    ),
                  ),
                  if (note.isNotEmpty)
                    Text(
                      note,
                      style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  (Glyph, Color, String, String) _words(BuildContext context, ThemeTokens c) {
    String who(FedBy by) => whoText(feeder, by);
    return switch (entry.body) {
      HistoryBody_Fed(:final result) when result.uncertain => (
        Glyph.warning,
        c.warning,
        'Fed between ${result.confirmed} and ${result.atMost}',
        '${_asker(context, result.by)} · not repeated',
      ),
      HistoryBody_Fed(:final result)
          when result.outcome == FeedOutcome.done &&
              result.confirmed == result.requested =>
        (
          Glyph.check,
          c.ok,
          'Fed ${deliveredText(result)}',
          _asker(context, result.by),
        ),
      HistoryBody_Fed(:final result) => (
        Glyph.warning,
        c.warning,
        'Fed ${deliveredText(result)}',
        '${_asker(context, result.by)} · '
            '${faultTitle(context, result.reason, null)}',
      ),
      HistoryBody_Missed(:final slot, :final dueMs, :final reason) => (
        Glyph.warning,
        c.warning,
        'Meal skipped',
        '${_meal(context, slot, dueMs)} · ${_missed(reason)}',
      ),
      HistoryBody_Restarted() => (Glyph.power, c.textMuted, 'Started up', ''),
      HistoryBody_Lock(:final locked, :final by) => (
        Glyph.lock,
        c.textMuted,
        locked ? 'Buttons locked' : 'Buttons unlocked',
        who(by),
      ),
      HistoryBody_SettingsChanged(:final by) => (
        Glyph.check,
        c.textMuted,
        'Settings changed',
        who(by),
      ),
      HistoryBody_WifiChanged(:final by) => (
        Glyph.wifi,
        c.textMuted,
        'Wi-Fi changed',
        who(by),
      ),
      HistoryBody_PlansChanged(:final by) => (
        Glyph.clock,
        c.textMuted,
        'Meals changed',
        who(by),
      ),
      HistoryBody_Joined(:final fingerprint) => (
        Glyph.add,
        c.textMuted,
        'Someone joined',
        feeder.clientName(fingerprint),
      ),
      HistoryBody_Revoked(:final fingerprint, :final by) => (
        Glyph.minus,
        c.textMuted,
        '${feeder.clientName(fingerprint)} removed',
        who(by),
      ),
      HistoryBody_ClockSet(:final by) => (
        Glyph.clock,
        c.textMuted,
        'Clock set',
        by is FedBy_Feeder ? 'From its time server' : who(by),
      ),
      HistoryBody_Lost(:final count) => (
        Glyph.warning,
        c.warning,
        "Some events weren't kept",
        '$count could not be recorded',
      ),
      HistoryBody_Gap() => (
        Glyph.info,
        c.textMuted,
        "Some events here weren't kept",
        '',
      ),
      HistoryBody_FeedingBlocked(reason: final reason?) => (
        Glyph.warning,
        c.warning,
        "Couldn't feed",
        blockedWords(reason),
      ),
      HistoryBody_FeedingBlocked(reason: null) => (
        Glyph.check,
        c.ok,
        'Can feed again',
        '',
      ),
      HistoryBody_MealSoon(:final minute, :final portions) => (
        Glyph.clock,
        c.textMuted,
        'Meal coming',
        '${minuteText(context, minute)} · ${portionsText(portions)}',
      ),
      HistoryBody_Supply(:final adapter) => (
        Glyph.power,
        c.textMuted,
        switch (adapter) {
          true => 'Back on mains',
          false => 'On batteries',
          null => 'Power changed',
        },
        '',
      ),
      HistoryBody_DispenseRefused(:final reason) => (
        Glyph.warning,
        c.textMuted,
        'A press was refused',
        blockedWords(reason),
      ),
      HistoryBody_Food(:final level) => (
        Glyph.food,
        level == FoodLevel.empty ? c.fault : c.textMuted,
        switch (level) {
          FoodLevel.empty => 'Hopper read empty',
          FoodLevel.present => 'Food in the hopper',
          FoodLevel.unread => 'Food not read',
        },
        '',
      ),
      HistoryBody_Other() => (Glyph.info, c.textMuted, '', ''),
    };
  }

  String _asker(BuildContext context, FedBy by) => switch (by) {
    FedBy_Plan(:final slot) => _meal(context, slot, null),
    FedBy_Client() => 'Feed now · ${whoText(feeder, by)}',
    _ => whoText(feeder, by),
  };

  /// `18:30 meal`, from when it was due or the slot's meal today, as an event
  /// names only the slot.
  String _meal(BuildContext context, int slot, int? dueMs) {
    final plans = feeder.card?.plans?.plans ?? const <PlanView?>[];
    final minute = slot < plans.length ? plans[slot]?.minute : null;
    if (dueMs != null) return '${clockTextMs(context, dueMs)} meal';
    return minute == null ? 'A meal' : '${minuteText(context, minute)} meal';
  }

  static String _missed(MissedReason reason) => switch (reason) {
    MissedReason_Late() => 'the feeder was off or had no time',
    MissedReason_Blocked(:final reason) => switch (reason) {
      Blocked.battery => 'the batteries were too low',
      _ => blockedWords(reason).toLowerCase(),
    },
    MissedReason_Interrupted() => 'the feeder stopped first',
  };
}
