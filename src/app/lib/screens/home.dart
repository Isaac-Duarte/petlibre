import 'dart:async';

import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../src/rust/api/install.dart' show PendingInstall;
import '../src/rust/api/model.dart';
import '../src/rust/api/session.dart' as api;
import '../state/app_state.dart';
import '../state/feeder.dart';
import '../theme/theme.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import 'feed_sheet.dart';
import 'restore_screen.dart';

/// Feeder-first Home: one card per feeder this phone knows, large for one
/// feeder and compact for several.
class HomeScreen extends StatefulWidget {
  const HomeScreen({
    super.key,
    required this.app,
    required this.earlierInstall,
  });

  final AppState app;

  /// What an earlier install left, read from its records. A run that died
  /// before it ended (the app killed, say) has no other way to say so, and a
  /// feeder on bank 0 answers Ready with its stock not yet saved.
  final Future<PendingInstall> Function() earlierInstall;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  final _feeders = <String, FeederController>{};
  PendingInstall _earlier = PendingInstall.nothing;

  @override
  void initState() {
    super.initState();
    widget.app.addListener(_sync);
    _sync();
    unawaited(_readEarlier());
  }

  Future<void> _readEarlier() async {
    final earlier = await widget.earlierInstall();
    if (mounted) setState(() => _earlier = earlier);
  }

  Future<void> _openInstall() async {
    await context.push('/install');
    await _readEarlier();
  }

  /// One controller per known feeder, kept across rebuilds.
  void _sync() {
    final known = {for (final f in widget.app.feeders) f.identity: f};
    for (final gone
        in _feeders.keys.where((id) => !known.containsKey(id)).toList()) {
      _feeders.remove(gone)?.dispose();
    }
    for (final feeder in known.values) {
      _feeders.putIfAbsent(
        feeder.identity,
        () => FeederController(widget.app, feeder),
      );
    }
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.app.removeListener(_sync);
    for (final controller in _feeders.values) {
      controller.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final one = _feeders.length == 1;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        titleSpacing: 20,
        title: Text(
          'Feeders',
          style: TextStyle(
            fontSize: 22,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
        actions: [
          IconButton(
            tooltip: 'Add a feeder',
            icon: StrokeIcon(Glyph.add, color: c.text),
            onPressed: () => context.push('/add'),
          ),
          const SizedBox(width: Space.s2),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
        children: [
          if (_earlier != PendingInstall.nothing) ...[
            const Notice(
              tone: Tone.warning,
              lead: "An install didn't finish.",
              text:
                  'Its last steps may not have run, and the original firmware '
                  'may not be saved. Finish it before you rely on this feeder.',
            ),
            const SizedBox(height: Space.s2),
            FilledButton(
              onPressed: () => unawaited(_openInstall()),
              child: const Text('Finish installing'),
            ),
            const SizedBox(height: Space.s4),
          ],
          for (final (i, controller) in _feeders.values.indexed) ...[
            if (i > 0) const SizedBox(height: Space.s3),
            FeederCard(controller: controller, compact: !one),
            RestoreEntry(app: widget.app, feeder: controller.known),
          ],
        ],
      ),
    );
  }
}

class FeederCard extends StatelessWidget {
  const FeederCard({super.key, required this.controller, this.compact = false});

  final FeederController controller;

  /// One of several: a row with a small Feed button.
  final bool compact;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) => compact ? _compact(context) : _full(context),
  );

  Widget _full(BuildContext context) {
    final c = context.colors;
    final card = controller.card;
    final notices = _notices(context);
    return Panel(
      padding: const EdgeInsets.all(Space.s4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              FeederImage(size: 96, dimmed: _unreachable),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      controller.known.name,
                      style: TextStyle(
                        fontSize: 22,
                        fontWeight: Fonts.semibold,
                        color: c.text,
                      ),
                    ),
                    const SizedBox(height: Space.s2),
                    _chips(context),
                  ],
                ),
              ),
            ],
          ),
          if (card != null) ...[
            const SizedBox(height: Space.s4),
            _tiles(context, card),
          ],
          for (final notice in notices) ...[const SizedBox(height: 14), notice],
          const SizedBox(height: Space.s4),
          FilledButton(
            onPressed: controller.canFeed
                ? () => showFeedSheet(context, controller)
                : null,
            child: const Text('Feed now'),
          ),
          if (controller.feed.phase != FeedPhase.idle) ...[
            const SizedBox(height: Space.s1),
            TextButton(
              onPressed: () => showFeedSheet(context, controller),
              child: Text(_feedLink(controller.feed)),
            ),
          ],
        ],
      ),
    );
  }

  Widget _compact(BuildContext context) {
    final c = context.colors;
    final line = _line();
    final why = _why();
    return Panel(
      padding: const EdgeInsets.fromLTRB(10, 14, 14, 14),
      onTap: controller.feed.phase == FeedPhase.idle
          ? null
          : () => showFeedSheet(context, controller),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              FeederImage(size: 72, dimmed: _unreachable),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      controller.known.name,
                      style: TextStyle(
                        fontSize: 17,
                        fontWeight: Fonts.semibold,
                        color: c.text,
                      ),
                    ),
                    const SizedBox(height: 6),
                    _chips(context),
                    if (line != null) ...[
                      const SizedBox(height: 6),
                      Text(
                        line,
                        style: TextStyle(
                          fontSize: Sizes.sm,
                          color: c.textMuted,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              const SizedBox(width: Space.s2),
              FilledButton(
                style: FilledButton.styleFrom(
                  minimumSize: const Size(0, 44),
                  padding: const EdgeInsets.symmetric(horizontal: 14),
                  textStyle: const TextStyle(
                    fontSize: Sizes.base,
                    fontWeight: Fonts.semibold,
                  ),
                ),
                onPressed: controller.canFeed
                    ? () => showFeedSheet(context, controller)
                    : null,
                child: const Text('Feed'),
              ),
            ],
          ),
          if (why != null) ...[
            const SizedBox(height: Space.s3),
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: Text(
                why,
                style: TextStyle(
                  fontSize: Sizes.sm,
                  height: 1.45,
                  color: c.textMuted,
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  bool get _unreachable => controller.connection == Connection.offline;

  /// The live card: the status read in the session now open.
  api.Card? get _live =>
      controller.connection == Connection.online ? controller.card : null;

  Widget _chips(BuildContext context) {
    final c = context.colors;
    final card = controller.card;
    final live = _live;
    final feed = controller.feed;

    final status = switch (controller.connection) {
      Connection.looking => StatusChip('Looking…', color: c.textMuted),
      Connection.connecting => StatusChip('Connecting…', color: c.textMuted),
      Connection.offline => StatusChip("Can't reach it", color: c.textMuted),
      Connection.online when feed.phase == FeedPhase.running => StatusChip(
        'Feeding ${_portions(feed.portions)}',
        color: c.accent,
      ),
      Connection.online when live?.ready ?? false => StatusChip(
        'Ready',
        color: c.ok,
      ),
      Connection.online when live?.blocked == Blocked.feeding => StatusChip(
        'Feeding',
        color: c.accent,
      ),
      Connection.online => StatusChip("Can't feed now", color: c.warning),
    };

    return Wrap(
      spacing: 14,
      runSpacing: 6,
      children: [
        status,
        if (card != null) _food(context, card.food),
        if (card?.adapter == false)
          StatusChip('On battery', color: c.warning, glyph: Glyph.battery),
        if (live != null && !live.clockSet)
          StatusChip('No time', color: c.warning, glyph: Glyph.clock),
      ],
    );
  }

  Widget _food(BuildContext context, FoodLevel food) {
    final c = context.colors;
    final (text, color) = switch (food) {
      FoodLevel.present => ('Food in', c.text),
      FoodLevel.empty => ('Hopper empty', c.fault),
      FoodLevel.unread => ('Food not read', c.textMuted),
    };
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        FoodDot(
          present: switch (food) {
            FoodLevel.present => true,
            FoodLevel.empty => false,
            FoodLevel.unread => null,
          },
        ),
        const SizedBox(width: 6),
        Text(
          text,
          style: TextStyle(
            fontSize: Sizes.sm,
            fontWeight: Fonts.semibold,
            color: color,
          ),
        ),
      ],
    );
  }

  Widget _tiles(BuildContext context, api.Card card) {
    final (power, battery) = switch ((card.adapter, card.batteryPercent)) {
      (true, final percent?) => ('Mains', 'Battery $percent%'),
      (true, null) => ('Mains', 'Plugged in'),
      (false, final percent?) => ('Battery', '$percent%, Wi-Fi off'),
      (false, null) => ('Battery', 'Wi-Fi off'),
      (null, _) => ('Unknown', 'Not read yet'),
    };
    final signal = switch (card.rssi) {
      >= -60 => 'Good',
      >= -72 => 'Fair',
      _ => 'Weak',
    };
    return Row(
      children: [
        Expanded(child: _Tile('Power', power, battery)),
        const SizedBox(width: Space.s2),
        Expanded(
          child: _Tile('Panel', switch (card.locked) {
            true => 'Locked',
            false => 'Unlocked',
            null => 'Unknown',
          }, 'Child lock'),
        ),
        const SizedBox(width: Space.s2),
        Expanded(child: _Tile('Wi-Fi', signal, '${card.rssi} dBm')),
      ],
    );
  }

  List<Widget> _notices(BuildContext context) {
    final card = controller.card;
    final live = _live;
    final why = _why();
    return [
      if (live != null && !live.clockSet)
        const Notice(
          tone: Tone.warning,
          lead: 'Meals are on hold.',
          text:
              "The feeder doesn't know the time yet, so it won't guess when "
              'to feed. Feed now still works.',
        ),
      if (live?.blocked case final blocked? when blocked != Blocked.feeding)
        Notice(tone: Tone.warning, text: '${blockedWords(blocked)}.'),
      if (card?.fault case final fault?)
        Notice(
          tone: Tone.fault,
          lead: fault == FeedOutcome.uncertain
              ? "The last feed didn't finish."
              : 'The last feed failed.',
          text:
              'The feeder stopped without recording how much came out. Look '
              'at the bowl before you feed again.',
        ),
      if (card?.food == FoodLevel.empty)
        const Notice(
          tone: Tone.fault,
          lead: 'The hopper is empty.',
          text: 'Refill it, or meals and feeds drop nothing.',
        ),
      if (why != null) Notice(tone: Tone.neutral, text: why),
    ];
  }

  /// The compact card's second line.
  String? _line() {
    final feed = controller.feed;
    if (feed.phase == FeedPhase.running) return 'Started from this phone';
    if (controller.connection != Connection.offline) return null;
    final at = controller.cardAt;
    if (at == null) return 'Not heard from yet';
    final minutes = DateTime.now().difference(at).inMinutes;
    return minutes < 1 ? 'Last heard just now' : 'Last heard $minutes min ago';
  }

  /// Why the feeder is out of reach, when the card can tell.
  String? _why() {
    if (controller.card?.adapter == false) {
      return 'The power adapter is out, so it turned Wi-Fi off to save the '
          "batteries. Meals still run on time. You'll see it again when "
          "it's plugged back in.";
    }
    if (controller.connection == Connection.offline) {
      return "This phone can't reach it. Check that it's plugged in, the "
          'Wi-Fi light is steady, and this phone is on the same Wi-Fi.';
    }
    return null;
  }

  static String _portions(int n) => n == 1 ? '1 portion' : '$n portions';

  static String _feedLink(FeedState feed) => switch (feed.phase) {
    FeedPhase.sending || FeedPhase.running => 'Feeding…',
    FeedPhase.ended => 'Last feed: ${feed.result?.words ?? 'ended'}',
    FeedPhase.refused => 'The feeder did not feed',
    FeedPhase.unanswered || FeedPhase.lost => 'A feed may have run',
    FeedPhase.notSent => 'The last feed was not sent',
    FeedPhase.idle => '',
  };
}

String blockedWords(Blocked reason) => switch (reason) {
  Blocked.starting => 'Starting up',
  Blocked.journal => 'Its feed record could not open; restart the feeder',
  Blocked.fault => 'Stopped on a fault until it restarts',
  Blocked.resolving => 'Settling a feed a restart interrupted',
  Blocked.feeding => 'A feed is running',
  Blocked.joining => 'Joining Wi-Fi',
  Blocked.updating => 'Installing an update',
  Blocked.setup => 'In setup mode',
  Blocked.unserved => 'This firmware does not feed',
  Blocked.battery => 'Battery too low to feed',
  Blocked.restarting => 'Restarting',
};

/// A sunken fact: a label, the value, and a note under it.
class _Tile extends StatelessWidget {
  const _Tile(this.label, this.value, this.note);

  final String label;
  final String value;
  final String note;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final small = TextStyle(fontSize: Sizes.xs, color: c.textMuted);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: c.sunken,
        borderRadius: BorderRadius.circular(Radii.icon),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: small),
          const SizedBox(height: 2),
          Text(
            value,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: Sizes.md,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          Text(
            note,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: small,
          ),
        ],
      ),
    );
  }
}
