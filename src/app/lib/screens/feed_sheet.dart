import 'dart:async';

import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../src/rust/api/model.dart';
import '../src/rust/api/session.dart';
import '../state/feeder.dart';
import '../theme/theme.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import '../widgets/when.dart';
import 'feeder/tabs.dart';

/// Feed now: confirm the portions, then follow the feed to its end. The
/// sheet shows only what the feeder said; an uncertain feed shows its range
/// and is never offered again as a retry.
Future<void> showFeedSheet(BuildContext context, FeederController feeder) =>
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      builder: (context) => ListenableBuilder(
        listenable: feeder,
        builder: (context, _) => SafeArea(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(Space.s5, 0, Space.s5, 28),
            child: switch (feeder.feed.phase) {
              FeedPhase.idle => _Confirm(feeder: feeder),
              _ => _Progress(feeder: feeder),
            },
          ),
        ),
      ),
    );

TextStyle _title(BuildContext context, {double size = 24}) => TextStyle(
  fontSize: size,
  fontWeight: Fonts.semibold,
  height: 1.25,
  color: context.colors.text,
);

String _portions(int n) => n == 1 ? 'portion' : 'portions';

String _time(BuildContext context, DateTime? at) =>
    at == null ? '' : clockText(context, at);

class _Confirm extends StatefulWidget {
  const _Confirm({required this.feeder});

  final FeederController feeder;

  @override
  State<_Confirm> createState() => _ConfirmState();
}

class _ConfirmState extends State<_Confirm> {
  static const _most = 48;
  int _portions = 1;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    Widget step(Glyph glyph, String tooltip, VoidCallback? onPressed) =>
        IconButton.outlined(
          tooltip: tooltip,
          constraints: const BoxConstraints.tightFor(width: 56, height: 56),
          style: IconButton.styleFrom(side: BorderSide(color: c.lineStrong)),
          onPressed: onPressed,
          icon: StrokeIcon(
            glyph,
            weight: 2,
            color: onPressed == null ? c.lineStrong : c.text,
          ),
        );

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Feed ${widget.feeder.known.name} now',
          style: _title(context, size: 22),
        ),
        const SizedBox(height: 20),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            step(
              Glyph.minus,
              'One fewer portion',
              _portions > 1 ? () => setState(() => _portions--) : null,
            ),
            const SizedBox(width: 28),
            SizedBox(
              width: 96,
              child: Column(
                children: [
                  Text(
                    '$_portions',
                    style: TextStyle(
                      fontSize: 56,
                      height: 1,
                      fontWeight: Fonts.display,
                      color: c.text,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                  const SizedBox(height: 6),
                  Text(
                    _portionsWord,
                    style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 28),
            step(
              Glyph.add,
              'One more portion',
              _portions < _most ? () => setState(() => _portions++) : null,
            ),
          ],
        ),
        const SizedBox(height: 20),
        Text(
          "One portion is one turn of the feeder's wheel, about 9 seconds. "
          "Up to $_most. Today's meals still run.",
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: Sizes.base,
            height: 1.5,
            color: c.textMuted,
          ),
        ),
        const SizedBox(height: 20),
        Row(
          children: [
            Expanded(
              child: OutlinedButton(
                onPressed: () => Navigator.pop(context),
                child: const Text('Cancel'),
              ),
            ),
            const SizedBox(width: Space.s3),
            Expanded(
              child: FilledButton(
                onPressed: widget.feeder.canFeed
                    ? () => widget.feeder.feedNow(_portions)
                    : null,
                child: Text('Feed $_portions $_portionsWord'),
              ),
            ),
          ],
        ),
      ],
    );
  }

  String get _portionsWord => _portions == 1 ? 'portion' : 'portions';
}

class _Progress extends StatelessWidget {
  const _Progress({required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) {
    final feed = feeder.feed;
    final result = feed.result;
    return switch (feed.phase) {
      FeedPhase.sending || FeedPhase.running => _running(context),
      FeedPhase.ended when result != null && result.uncertain => _uncertain(
        context,
        result,
      ),
      FeedPhase.ended when result != null => _ended(context, result),
      FeedPhase.refused || FeedPhase.notSent => _notFed(context),
      FeedPhase.unanswered || FeedPhase.lost => _unknown(context),
      _ => const SizedBox.shrink(),
    };
  }

  String get _name => feeder.known.name;

  void _close(BuildContext context) {
    feeder.dismissFeed();
    Navigator.pop(context);
  }

  Widget _sheet(
    BuildContext context, {
    Glyph? glyph,
    Color? color,
    required String title,
    required List<Widget> children,
  }) => Column(
    mainAxisSize: MainAxisSize.min,
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      if (glyph != null) ...[
        Align(
          alignment: Alignment.centerLeft,
          child: StrokeIcon(glyph, size: 40, color: color, weight: 2),
        ),
        const SizedBox(height: Space.s4),
      ],
      Text(title, style: _title(context)),
      for (final child in children) ...[
        const SizedBox(height: Space.s4),
        child,
      ],
    ],
  );

  Text _lead(BuildContext context, String text) => Text(
    text,
    style: TextStyle(fontSize: 15, height: 1.5, color: context.colors.text),
  );

  Text _muted(BuildContext context, String text, {TextAlign? align}) => Text(
    text,
    textAlign: align,
    style: TextStyle(
      fontSize: Sizes.base,
      height: 1.5,
      color: context.colors.textMuted,
    ),
  );

  Widget _running(BuildContext context) {
    final c = context.colors;
    final feed = feeder.feed;
    final running = feed.phase == FeedPhase.running;
    return _sheet(
      context,
      title: running ? 'Feeding $_name' : 'Sending…',
      children: [
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(Radii.pill),
              child: LinearProgressIndicator(
                minHeight: 10,
                backgroundColor: c.sunken,
                color: c.food,
              ),
            ),
            const SizedBox(height: Space.s2),
            _muted(context, '${feed.portions} ${_portions(feed.portions)}'),
          ],
        ),
        _muted(
          context,
          "You can close this. The result shows on $_name's card and in its "
          'history.',
        ),
        if (running) ...[
          OutlinedButton(
            style: OutlinedButton.styleFrom(foregroundColor: c.fault),
            onPressed: feeder.cancelFeed,
            child: const Text('Stop feeding'),
          ),
          if (feed.cancelUnconfirmed)
            Text(
              'The feeder did not confirm the stop; the feed may run to its '
              'end.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: Sizes.sm, color: c.warning),
            )
          else
            _muted(
              context,
              'Stopping keeps what already dropped in the bowl.',
              align: TextAlign.center,
            ),
        ],
      ],
    );
  }

  Widget _ended(BuildContext context, FeedResult result) {
    final c = context.colors;
    final all = result.confirmed == result.requested;
    final food = feeder.card?.food;
    return _sheet(
      context,
      glyph: all ? Glyph.check : Glyph.warning,
      color: all ? c.ok : c.warning,
      title:
          'Fed ${result.confirmed} of ${result.requested} '
          '${_portions(result.requested)}',
      children: [
        if (!all) _lead(context, _upper(result.words)),
        FactList(sunken: true, [
          ('Finished', Text(_time(context, feeder.feed.endedAt))),
          if (food != null && food != FoodLevel.unread)
            (
              'Food in the hopper',
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  FoodDot(present: food == FoodLevel.present),
                  const SizedBox(width: 6),
                  Text(food == FoodLevel.present ? 'Yes' : 'Empty'),
                ],
              ),
            ),
          ('Asked by', const Text('This phone')),
        ]),
        FilledButton(
          onPressed: () => _close(context),
          child: const Text('Close'),
        ),
      ],
    );
  }

  Widget _uncertain(BuildContext context, FeedResult result) {
    final c = context.colors;
    return _sheet(
      context,
      glyph: Glyph.warning,
      color: c.warning,
      title:
          'Fed between ${result.confirmed} and ${result.atMost} '
          '${_portions(result.atMost)}',
      children: [
        _lead(context, _uncertainWhy(result.reason)),
        Container(
          padding: const EdgeInsets.only(left: Space.s3),
          decoration: BoxDecoration(
            border: Border(left: BorderSide(color: c.warning, width: 3)),
          ),
          child: _muted(
            context,
            "Wellfed never repeats a feed it can't confirm. Look at the "
            'bowl before you feed again.',
          ),
        ),
        Row(
          children: [
            Expanded(
              child: OutlinedButton(
                onPressed: () {
                  _close(context);
                  unawaited(
                    context.push(
                      '/feeder/${feeder.known.identity}?tab=$historyTab',
                    ),
                  );
                },
                child: const Text('See history'),
              ),
            ),
            const SizedBox(width: Space.s3),
            Expanded(
              child: FilledButton(
                onPressed: () => _close(context),
                child: const Text('Close'),
              ),
            ),
          ],
        ),
      ],
    );
  }

  Widget _notFed(BuildContext context) {
    final c = context.colors;
    final feed = feeder.feed;
    final why = feed.phase == FeedPhase.notSent
        ? 'The connection dropped before the request went out. Try again '
              'once the feeder is online.'
        : _refusal(feed.refusal);
    return _sheet(
      context,
      glyph: Glyph.failed,
      color: c.fault,
      title: "$_name didn't feed",
      children: [
        _lead(context, why),
        _muted(context, 'Nothing was dispensed.'),
        Row(
          children: [
            Expanded(
              child: OutlinedButton(
                onPressed: () => _close(context),
                child: const Text('Close'),
              ),
            ),
            const SizedBox(width: Space.s3),
            Expanded(
              child: FilledButton(
                // Nothing went out or the feeder said no: a new request is
                // safe, from the confirm step.
                onPressed: feeder.dismissFeed,
                child: const Text('Start over'),
              ),
            ),
          ],
        ),
      ],
    );
  }

  Widget _unknown(BuildContext context) {
    final c = context.colors;
    final feed = feeder.feed;
    final sent = _time(context, feed.sentAt);
    final lead = feed.phase == FeedPhase.lost
        ? 'The feed started${sent.isEmpty ? '' : ' at $sent'}, then the '
              'connection dropped. The feeder finishes it on its own and '
              'keeps the result in its history.'
        : 'We sent the feed${sent.isEmpty ? '' : ' at $sent'}, then lost the '
              'connection before the feeder answered.';
    return _sheet(
      context,
      glyph: Glyph.wifiOff,
      color: c.textMuted,
      title: 'Did $_name feed?',
      children: [
        _lead(context, lead),
        _muted(
          context,
          "Wellfed won't send it again on its own. Look at the bowl before "
          'you feed again.',
        ),
        FilledButton(
          onPressed: () => _close(context),
          child: const Text('Close'),
        ),
      ],
    );
  }

  static String _upper(String words) =>
      words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);

  static String _uncertainWhy(FeedReason reason) => switch (reason) {
    FeedReason.interrupted =>
      "The feeder restarted during the feed, so it can't say how much "
          'dropped.',
    FeedReason.power =>
      "The power went out during the feed, so the feeder can't say how much "
          'dropped.',
    FeedReason.sensor || FeedReason.noFeedback || FeedReason.feedbackStream =>
      "The feeder lost track of its wheel, so it can't say how much dropped.",
    _ => 'The feeder stopped without recording how much dropped.',
  };

  static String _refusal(FeedRefusal? refusal) => switch (refusal) {
    FeedRefusal.forbidden => 'This phone may not feed this feeder.',
    FeedRefusal.busy => "It's busy saving a change. Try again in a moment.",
    FeedRefusal.fault =>
      'It stopped on a fault and feeds again after a restart.',
    FeedRefusal.portions => 'A feed is 1 to 48 portions.',
    FeedRefusal.notFound => 'That feed is not running.',
    FeedRefusal.unavailable => 'This firmware does not feed.',
    FeedRefusal.other || null => 'It refused the request.',
  };
}
