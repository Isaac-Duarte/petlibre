import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../src/rust/api/session.dart' as api;
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import '../../widgets/when.dart';
import '../feed_sheet.dart';
import '../home.dart' show blockedWords;
import 'lights_help.dart';
import 'no_time.dart';
import 'shell.dart' show confirmRestart;

/// How the feeder stands today, its facts and the lock (7.1), or what
/// stopped it (7.3). It shows only what the session has read; a fact not read
/// yet says so.
class TodayTab extends StatelessWidget {
  const TodayTab({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: feeder,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final card = feeder.card;
    final live = feeder.connection == Connection.online ? card : null;
    final stopped = live?.blocked == Blocked.fault;

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: ListView(
              children: [
                _Summary(feeder: feeder),
                const SizedBox(height: Space.s3),
                if (live != null && !live.clockSet) ...[
                  NoTimeNotice(feeder: feeder),
                  const SizedBox(height: Space.s3),
                ],
                if (stopped) ...[
                  _Stopped(feeder: feeder),
                  const SizedBox(height: Space.s3),
                ] else if (card?.fault case final fault?) ...[
                  _LastFeedFailed(fault: fault),
                  const SizedBox(height: Space.s3),
                ],
                if (card != null) ...[
                  _Facts(feeder: feeder, card: card),
                  const SizedBox(height: Space.s3),
                ],
                _LightsLink(),
                if (stopped) ...[
                  const SizedBox(height: Space.s3),
                  Text(
                    'Other faults that show here: the bowl sensor stopped '
                    'answering, the outlet is blocked, the motor drew too '
                    'much current.',
                    style: TextStyle(
                      fontSize: Sizes.sm,
                      height: 1.5,
                      color: context.colors.textMuted,
                    ),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(height: Space.s3),
          FilledButton(
            onPressed: feeder.canFeed
                ? () => showFeedSheet(context, feeder)
                : null,
            child: Text(
              stopped ? 'Feed now · unavailable until restart' : 'Feed now',
            ),
          ),
        ],
      ),
    );
  }
}

class _Summary extends StatelessWidget {
  const _Summary({required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final card = feeder.connection == Connection.online ? feeder.card : null;
    final (dot, title) = switch (feeder.connection) {
      Connection.looking => (c.textMuted, 'Looking…'),
      Connection.connecting => (c.textMuted, 'Connecting…'),
      Connection.offline => (c.textMuted, "Can't reach it"),
      Connection.online when feeder.feed.active => (c.accent, 'Feeding'),
      Connection.online when card?.ready ?? false => (c.ok, 'Ready to feed'),
      Connection.online when card?.blocked == Blocked.feeding => (
        c.accent,
        'Feeding',
      ),
      Connection.online when card?.blocked == Blocked.fault => (
        c.fault,
        "Can't feed",
      ),
      Connection.online => (c.warning, "Can't feed now"),
    };
    final small = TextStyle(fontSize: Sizes.base, color: c.textMuted);
    final next = feeder.card?.nextMeal;
    final last = feeder.card?.lastFed;
    final blocked = card?.blocked;

    return Row(
      children: [
        FeederImage(size: 120, dimmed: feeder.connection == Connection.offline),
        const SizedBox(width: Space.s4),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 10,
                    height: 10,
                    decoration: BoxDecoration(
                      color: dot,
                      shape: BoxShape.circle,
                    ),
                  ),
                  const SizedBox(width: Space.s2),
                  Flexible(
                    child: Text(
                      title,
                      style: TextStyle(
                        fontSize: 20,
                        fontWeight: Fonts.semibold,
                        color: c.text,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: Space.s1),
              if (blocked == Blocked.fault)
                Text('Meals are held until it restarts', style: small)
              else if (blocked != null && blocked != Blocked.feeding)
                Text('${blockedWords(blocked)}.', style: small),
              if (next != null)
                Text(
                  'Next meal ${minuteText(context, next.minute)} · '
                  '${portionsText(next.portions)}',
                  style: small,
                ),
              if (last != null) Text(_lastFed(context, last), style: small),
            ],
          ),
        ),
      ],
    );
  }

  static String _lastFed(BuildContext context, api.FeedResult last) {
    final at = last.atMs;
    final when = at == null ? '' : ' ${clockTextMs(context, at)} ·';
    return 'Last fed$when ${deliveredText(last)}';
  }
}

class _Facts extends StatelessWidget {
  const _Facts({required this.feeder, required this.card});

  final FeederController feeder;
  final api.Card card;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final last = card.lastFed?.atMs;
    final at = last == null ? '' : ' at ${clockTextMs(context, last)}';
    final (food, foodColor) = switch (card.food) {
      FoodLevel.present => ('In the hopper$at', c.food),
      FoodLevel.empty => ('Hopper empty$at', c.fault),
      FoodLevel.unread => ('Not read yet', c.textMuted),
    };
    final online = feeder.connection == Connection.online;

    return Panel(
      child: Column(
        children: [
          _Row(
            Glyph.food,
            'Food',
            first: true,
            value: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    color: foodColor,
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 6),
                Flexible(child: _value(context, food)),
              ],
            ),
          ),
          _Row(Glyph.power, 'Power', value: _value(context, _power(card))),
          _Row(Glyph.wifi, 'Wi-Fi', value: _value(context, _signal(card.rssi))),
          _Row(
            Glyph.clock,
            'Clock',
            value: _value(context, clockWords(context, card)),
          ),
          _Row(
            Glyph.droplet,
            'Desiccant',
            value: _DesiccantValue(feeder: feeder, card: card, online: online),
          ),
          _LockRow(feeder: feeder, card: card, online: online),
        ],
      ),
    );
  }

  Widget _value(BuildContext context, String text) => Text(
    text,
    textAlign: TextAlign.end,
    style: TextStyle(fontSize: Sizes.base, color: context.colors.text),
  );

  static String _power(api.Card card) =>
      switch ((card.adapter, card.batteryPercent)) {
        (true, final percent?) => 'Mains · batteries $percent%',
        (true, null) => 'Mains',
        (false, final percent?) => 'Batteries $percent%',
        (false, null) => 'Batteries',
        (null, _) => 'Not read yet',
      };

  /// Every session in the app goes straight to the feeder.
  static String _signal(int rssi) {
    final strength = switch (rssi) {
      >= -60 => 'Strong',
      >= -72 => 'Fair',
      _ => 'Weak',
    };
    return '$strength · direct to this phone';
  }
}

/// Who last set the feeder's clock, and when.
String clockWords(BuildContext context, api.Card card) {
  if (!card.clockSet) return 'Not set';
  final time = card.time;
  if (time == null) return 'Set';
  final by = switch ((time.last, time.ntp)) {
    (TimeSource.ntp, NtpView_Gateway()) => 'Set by your router',
    (TimeSource.ntp, _) => 'Set by a time server',
    (TimeSource.owner || TimeSource.client, _) => 'Set by a phone',
    (null, _) => 'Set',
  };
  final at = time.lastAtMs;
  return at == null ? by : '$by at ${clockTextMs(context, at)}';
}

class _Row extends StatelessWidget {
  const _Row(this.glyph, this.label, {required this.value, this.first = false});

  final Glyph glyph;
  final String label;
  final Widget value;
  final bool first;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Container(
      constraints: const BoxConstraints(minHeight: 48),
      padding: const EdgeInsets.symmetric(horizontal: Space.s4, vertical: 8),
      decoration: BoxDecoration(
        border: first ? null : Border(top: BorderSide(color: c.line)),
      ),
      child: Row(
        children: [
          StrokeIcon(glyph, size: 22, color: c.textMuted),
          const SizedBox(width: 14),
          Text(label, style: TextStyle(fontSize: 15, color: c.text)),
          const SizedBox(width: Space.s3),
          Expanded(
            child: Align(alignment: Alignment.centerRight, child: value),
          ),
        ],
      ),
    );
  }
}

class _DesiccantValue extends StatefulWidget {
  const _DesiccantValue({
    required this.feeder,
    required this.card,
    required this.online,
  });

  final FeederController feeder;
  final api.Card card;
  final bool online;

  @override
  State<_DesiccantValue> createState() => _DesiccantValueState();
}

class _DesiccantValueState extends State<_DesiccantValue> {
  bool _asking = false;

  Future<void> _changed() async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(const FeederRequest.resetDesiccant());
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final desiccant = widget.card.desiccant;
    final (words, color) = switch (desiccant) {
      Desiccant_Unset() => ('Not started', c.textMuted),
      Desiccant_Unknown() => ('Unknown until the clock is set', c.textMuted),
      Desiccant_Remaining(days: 0) => ('Change today', c.warning),
      Desiccant_Remaining(:final days) when days > 0 => (
        days == 1 ? 'Change in 1 day' : 'Change in $days days',
        c.text,
      ),
      Desiccant_Remaining(:final days) => (
        days == -1 ? '1 day overdue' : '${-days} days overdue',
        c.warning,
      ),
    };
    final offer = switch (desiccant) {
      Desiccant_Unset() => true,
      Desiccant_Remaining(:final due) => due,
      Desiccant_Unknown() => false,
    };

    return Wrap(
      alignment: WrapAlignment.end,
      crossAxisAlignment: WrapCrossAlignment.center,
      spacing: Space.s2,
      children: [
        Text(
          words,
          style: TextStyle(fontSize: Sizes.base, color: color),
        ),
        if (offer)
          TextButton(
            onPressed: widget.online && !_asking ? _changed : null,
            child: const Text('Changed today'),
          ),
      ],
    );
  }
}

class _LockRow extends StatefulWidget {
  const _LockRow({
    required this.feeder,
    required this.card,
    required this.online,
  });

  final FeederController feeder;
  final api.Card card;
  final bool online;

  @override
  State<_LockRow> createState() => _LockRowState();
}

class _LockRowState extends State<_LockRow> {
  bool _asking = false;

  Future<void> _set(bool locked) async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(
      FeederRequest.setLock(locked: locked),
    );
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    final locked = widget.card.locked;
    return _Row(
      Glyph.lock,
      'Lock the buttons',
      value: Switch(
        // What the feeder last said, not what was asked: an answer moves it.
        value: locked ?? false,
        onChanged: widget.online && locked != null && !_asking ? _set : null,
      ),
    );
  }
}

class _LightsLink extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return InkWell(
      onTap: () => unawaited(
        Navigator.of(context)
            .push(MaterialPageRoute<void>(builder: (_) => const LightsHelp())),
      ),
      child: Padding(
        padding: const EdgeInsets.all(Space.s1),
        child: Row(
          children: [
            StrokeIcon(Glyph.info, size: 20, color: c.accent),
            const SizedBox(width: 10),
            Flexible(
              child: Text(
                'What do the lights on the feeder mean?',
                style: TextStyle(
                  fontSize: Sizes.base,
                  fontWeight: Fonts.semibold,
                  color: c.accent,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// What a feed's reason comes to for an owner, with the time when known.
String faultTitle(BuildContext context, FeedReason reason, int? atMs) {
  final what = switch (reason) {
    FeedReason.jammed => 'The wheel jammed',
    FeedReason.outletBlocked => 'The outlet is blocked',
    FeedReason.overcurrent => 'The motor drew too much current',
    FeedReason.sensor => 'A sensor stopped answering',
    FeedReason.noFeedback ||
    FeedReason.feedbackStream => 'The feeder lost track of its wheel',
    FeedReason.markStuck => 'The wheel sensor stuck',
    FeedReason.power => 'The power went out during a feed',
    FeedReason.interrupted => 'The feeder restarted during a feed',
    FeedReason.output => 'The motor did not respond',
    FeedReason.clock => 'The clock jumped during a feed',
    FeedReason.observationGap => 'The feeder could not watch its motor',
    FeedReason.timeout => 'A feed ran past its limit',
    FeedReason.cancelled => 'A feed was stopped',
    FeedReason.done => 'A feed ended',
  };
  return atMs == null ? what : '$what at ${clockTextMs(context, atMs)}';
}

/// How far the fault's feed got, when its history has come in.
String? _stoppedAfter(api.FaultView fault) {
  final fed = fault.result;
  if (fed == null) return null;
  return fed.uncertain
      ? 'It stopped somewhere between ${deliveredText(fed)} portions.'
      : 'It stopped after ${deliveredText(fed)} portions.';
}

/// A fault holds feeding until a restart (7.3).
class _Stopped extends StatelessWidget {
  const _Stopped({required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final fault = feeder.card?.fault;
    final after = fault == null ? null : _stoppedAfter(fault);
    final title = fault == null
        ? 'A fault stopped feeding'
        : faultTitle(context, fault.reason, fault.result?.atMs);
    final stuck = fault?.reason == FeedReason.jammed;
    final detail = [
      ?after,
      if (stuck) 'Something may be stuck in the chute.',
    ].join(' ');
    final muted = TextStyle(
      fontSize: Sizes.base,
      height: 1.5,
      color: c.textMuted,
    );
    final step = TextStyle(fontSize: Sizes.base, height: 1.6, color: c.text);

    return Container(
      padding: const EdgeInsets.all(Space.s4),
      decoration: BoxDecoration(
        color: c.faultSoft,
        borderRadius: BorderRadius.circular(Radii.card),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            style: TextStyle(
              fontSize: 17,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          if (detail.isNotEmpty) ...[
            const SizedBox(height: Space.s2),
            Text(detail, style: muted),
          ],
          const SizedBox(height: Space.s2),
          for (final (i, line) in const [
            'Unplug the feeder and take out the batteries.',
            'Empty the hopper and clear the chute above the bowl.',
            'Plug it back in. It restarts and can feed again.',
          ].indexed)
            Text('${i + 1}. $line', style: step),
          const SizedBox(height: Space.s3),
          OutlinedButton(
            onPressed: () => unawaited(confirmRestart(context, feeder)),
            child: const Text('Restart it from here'),
          ),
        ],
      ),
    );
  }
}

/// A feed ended on a fault, and the feeder can feed again.
class _LastFeedFailed extends StatelessWidget {
  const _LastFeedFailed({required this.fault});

  final api.FaultView fault;

  @override
  Widget build(BuildContext context) => Notice(
    tone: Tone.fault,
    lead: '${faultTitle(context, fault.reason, fault.result?.atMs)}.',
    text:
        '${_stoppedAfter(fault) ?? 'The last feed did not finish.'} Look at '
        'the bowl before you feed again.',
  );
}
