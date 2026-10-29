import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../src/rust/api/model.dart';
import '../src/rust/api/session.dart' as api;
import '../state/app_state.dart';
import '../state/feeder.dart';
import '../theme/theme.dart';
import '../widgets/layout.dart';
import 'feed_sheet.dart';

/// Feeder-first Home: one card per feeder this phone knows.
class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.app});

  final AppState app;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  final _feeders = <String, FeederController>{};

  @override
  void initState() {
    super.initState();
    widget.app.addListener(_sync);
    _sync();
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
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      title: const Text('Feeders'),
      actions: [
        IconButton(
          tooltip: 'Add a feeder',
          icon: const Icon(Icons.add),
          onPressed: () => context.push('/add'),
        ),
      ],
    ),
    body: ListView(
      padding: const EdgeInsets.all(Space.s4),
      children: [
        for (final (i, controller) in _feeders.values.indexed) ...[
          if (i > 0) const SizedBox(height: Space.s4),
          FeederCard(controller: controller),
        ],
      ],
    ),
  );
}

class FeederCard extends StatelessWidget {
  const FeederCard({super.key, required this.controller});

  final FeederController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) {
      final c = context.colors;
      final card = controller.card;
      return Card(
        child: Padding(
          padding: const EdgeInsets.all(Space.s4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          controller.known.name,
                          style: TextStyle(
                            fontSize: Sizes.lg,
                            fontWeight: Fonts.semibold,
                            color: c.text,
                          ),
                        ),
                        const SizedBox(height: Space.s1),
                        _connection(context),
                      ],
                    ),
                  ),
                  Image.asset(
                    context.isDark
                        ? 'assets/renders/card-dark-cutout.png'
                        : 'assets/renders/card-cutout.png',
                    height: 96,
                    excludeFromSemantics: true,
                  ),
                ],
              ),
              if (card != null) ...[
                const SizedBox(height: Space.s3),
                const Divider(),
                const SizedBox(height: Space.s3),
                ..._facts(context, card),
              ],
              const SizedBox(height: Space.s4),
              FilledButton(
                onPressed: controller.canFeed
                    ? () => showFeedSheet(context, controller)
                    : null,
                child: const Text('Feed now'),
              ),
              if (controller.feed.phase != FeedPhase.idle) ...[
                const SizedBox(height: Space.s2),
                TextButton(
                  onPressed: () => showFeedSheet(context, controller),
                  child: Text(_feedLink(controller.feed)),
                ),
              ],
            ],
          ),
        ),
      );
    },
  );

  Widget _connection(BuildContext context) {
    final c = context.colors;
    return switch (controller.connection) {
      Connection.online => StatusLine('Online', color: c.ok),
      Connection.looking => StatusLine(
        'Looking on this network…',
        color: c.textMuted,
      ),
      Connection.connecting => StatusLine('Connecting…', color: c.textMuted),
      Connection.offline => StatusLine(_offline(), color: c.warning),
    };
  }

  String _offline() {
    final at = controller.cardAt;
    if (at == null) return 'Offline';
    final minutes = DateTime.now().difference(at).inMinutes;
    return minutes < 1 ? 'Offline, just now' : 'Offline, seen $minutes min ago';
  }

  List<Widget> _facts(BuildContext context, api.Card card) {
    final c = context.colors;
    final blocked = card.blocked;
    return [
      _Fact('Food', switch (card.food) {
        FoodLevel.present => 'In the hopper',
        FoodLevel.empty => 'Empty',
        FoodLevel.unread => 'Not read yet',
      }, color: card.food == FoodLevel.empty ? c.warning : null),
      _Fact('Power', switch ((card.adapter, card.batteryPercent)) {
        (true, _) => 'Mains',
        (false, final percent?) => 'Battery, $percent% (Wi-Fi off)',
        (false, null) => 'Battery (Wi-Fi off)',
        (null, _) => 'Not read yet',
      }, color: card.adapter == false ? c.warning : null),
      _Fact('Panel', switch (card.locked) {
        true => 'Locked',
        false => 'Unlocked',
        null => 'Not known yet',
      }),
      if (!card.clockSet)
        _Fact('Clock', 'Not set: meals wait until it is', color: c.warning),
      if (blocked != null)
        _Fact('Feeding', blockedWords(blocked), color: c.warning),
      if (card.fault != null)
        _Fact(
          'Last feed',
          card.fault == FeedOutcome.uncertain
              ? 'Uncertain: see its history'
              : 'Failed: see its history',
          color: c.fault,
        ),
    ];
  }

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

class _Fact extends StatelessWidget {
  const _Fact(this.label, this.value, {this.color});

  final String label;
  final String value;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s1),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 88,
            child: Text(
              label,
              style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
            ),
          ),
          Expanded(
            child: Text(
              value,
              style: TextStyle(
                fontSize: Sizes.base,
                fontWeight: Fonts.medium,
                color: color ?? c.text,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
