import 'package:flutter/material.dart';

import '../src/rust/api/session.dart';
import '../state/feeder.dart';
import '../theme/theme.dart';

/// Feed now: confirm the portions, then follow the feed to its end. The
/// sheet shows only what the feeder said; an uncertain feed shows its range
/// and is never offered again as a retry.
Future<void> showFeedSheet(BuildContext context, FeederController feeder) =>
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      builder: (context) => ListenableBuilder(
        listenable: feeder,
        builder: (context, _) => Padding(
          padding: const EdgeInsets.fromLTRB(Space.s4, 0, Space.s4, Space.s5),
          child: switch (feeder.feed.phase) {
            FeedPhase.idle => _Confirm(feeder: feeder),
            _ => _Progress(feeder: feeder),
          },
        ),
      ),
    );

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
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Feed ${widget.feeder.known.name} now',
          style: TextStyle(
            fontSize: Sizes.lg,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
        const SizedBox(height: Space.s5),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            IconButton.outlined(
              tooltip: 'Fewer',
              iconSize: 28,
              onPressed: _portions > 1
                  ? () => setState(() => _portions--)
                  : null,
              icon: const Icon(Icons.remove),
            ),
            SizedBox(
              width: 120,
              child: Column(
                children: [
                  Text(
                    '$_portions',
                    style: TextStyle(
                      fontSize: Sizes.xxl,
                      fontWeight: Fonts.display,
                      color: c.text,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                  Text(
                    _portions == 1 ? 'portion' : 'portions',
                    style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
                  ),
                ],
              ),
            ),
            IconButton.outlined(
              tooltip: 'More',
              iconSize: 28,
              onPressed: _portions < _most
                  ? () => setState(() => _portions++)
                  : null,
              icon: const Icon(Icons.add),
            ),
          ],
        ),
        const SizedBox(height: Space.s5),
        FilledButton(
          onPressed: widget.feeder.canFeed
              ? () => widget.feeder.feedNow(_portions)
              : null,
          child: Text('Feed $_portions'),
        ),
        const SizedBox(height: Space.s2),
        OutlinedButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

class _Progress extends StatelessWidget {
  const _Progress({required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final feed = feeder.feed;
    final result = feed.result;

    final (title, body, color) = switch (feed.phase) {
      FeedPhase.sending => ('Sending…', null, c.textMuted),
      FeedPhase.running => (
        'Feeding ${feed.portions} '
            '${feed.portions == 1 ? 'portion' : 'portions'}',
        'The feeder reports when it is done.',
        c.accent,
      ),
      FeedPhase.ended when result != null && result.uncertain => (
        'Between ${result.confirmed} and ${result.atMost} portions',
        'The feeder stopped without recording how much came out. '
            'Check the bowl before feeding again.',
        c.warning,
      ),
      FeedPhase.ended when result != null => (
        result.words[0].toUpperCase() + result.words.substring(1),
        null,
        result.confirmed == result.requested ? c.ok : c.warning,
      ),
      FeedPhase.refused => (
        'The feeder did not feed',
        _refusal(feed.refusal),
        c.warning,
      ),
      FeedPhase.unanswered => (
        'The feed may have run',
        'No answer came back, so it was not sent again. Check the bowl '
            'before feeding again; the feeder keeps the result in its '
            'history.',
        c.warning,
      ),
      FeedPhase.notSent => (
        'Nothing was fed',
        'The connection dropped before the request went out. Try again '
            'once the feeder is online.',
        c.warning,
      ),
      FeedPhase.lost => (
        'Lost contact during the feed',
        'The feeder finishes it on its own and keeps the result in its '
            'history. Check the bowl before feeding again.',
        c.warning,
      ),
      _ => ('', null, c.text),
    };

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          title,
          style: TextStyle(
            fontSize: Sizes.lg,
            fontWeight: Fonts.semibold,
            color: color,
          ),
        ),
        if (body != null) ...[
          const SizedBox(height: Space.s2),
          Text(
            body,
            style: TextStyle(
              fontSize: Sizes.base,
              color: c.textMuted,
              height: 1.4,
            ),
          ),
        ],
        const SizedBox(height: Space.s5),
        if (feed.active) const LinearProgressIndicator(),
        if (feed.phase == FeedPhase.running) ...[
          const SizedBox(height: Space.s4),
          OutlinedButton(
            onPressed: feeder.cancelFeed,
            child: const Text('Stop feeding'),
          ),
        ],
        if (feed.cancelUnconfirmed) ...[
          const SizedBox(height: Space.s3),
          Text(
            'The feeder did not confirm the stop; the feed may run to its end.',
            style: TextStyle(fontSize: Sizes.sm, color: c.warning),
          ),
        ],
        if (!feed.active)
          FilledButton(
            onPressed: () {
              feeder.dismissFeed();
              Navigator.pop(context);
            },
            child: const Text('Done'),
          ),
      ],
    );
  }

  static String _refusal(FeedRefusal? refusal) => switch (refusal) {
    FeedRefusal.forbidden => 'This phone may not feed this feeder.',
    FeedRefusal.busy => 'It is busy saving a change. Try again in a moment.',
    FeedRefusal.fault =>
      'It stopped on a fault and feeds again after a restart.',
    FeedRefusal.portions => 'A feed is 1 to 48 portions.',
    FeedRefusal.notFound => 'That feed is not running.',
    FeedRefusal.unavailable => 'This firmware does not feed.',
    FeedRefusal.other || null => 'It refused the request.',
  };
}
