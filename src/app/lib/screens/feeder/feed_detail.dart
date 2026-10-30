import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/model.dart';
import '../../src/rust/api/session.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/when.dart';
import 'history.dart' show whoText;
import 'motor_trace.dart';
import 'today.dart' show faultTitle;

/// One feed, in detail (9.2). An uncertain feed stays a range.
class FeedDetail extends StatelessWidget {
  const FeedDetail({super.key, required this.feeder, required this.result});

  final FeederController feeder;
  final FeedResult result;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final at = result.atMs;
    final when = at == null
        ? 'Time unknown'
        : '${dayText(DateTime.fromMillisecondsSinceEpoch(at))}, '
              '${clockTextMs(context, at)}';
    final all =
        result.outcome == FeedOutcome.done &&
        result.confirmed == result.requested;
    final title = result.uncertain
        ? 'Fed between ${result.confirmed} and ${result.atMost} portions'
        : 'Fed ${deliveredText(result)} portions';
    final detail = result.uncertain
        ? '${result.requested} asked for · '
              '${result.confirmed == 0 ? 'none' : result.confirmed} confirmed '
              '· each could have dropped'
        : '${result.requested} asked for';

    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Back',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.back, color: c.text),
        ),
        title: Text(
          when,
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.s4),
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: StrokeIcon(
              all ? Glyph.check : Glyph.warning,
              size: 40,
              weight: 2,
              color: all ? c.ok : c.warning,
            ),
          ),
          const SizedBox(height: Space.s3),
          Text(
            title,
            style: TextStyle(
              fontSize: Sizes.lg,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          const SizedBox(height: Space.s1),
          Text(
            detail,
            style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
          ),
          const SizedBox(height: Space.s4),
          FactList([
            if (!all)
              (
                result.uncertain ? "Why it's unsure" : 'Why it stopped',
                Flexible(
                  child: Text(
                    faultTitle(context, result.reason, null),
                    textAlign: TextAlign.end,
                  ),
                ),
              ),
            ('Started by', Text(whoText(feeder, result.by))),
            ('Food in the hopper', Text(_hopper(result.hopper))),
            ('Feed number', Text('${result.feed}')),
          ]),
          if (result.uncertain) ...[
            const SizedBox(height: Space.s4),
            Text(
              'When a feed is cut short, the feeder records the range it '
              'might have dropped and never runs it again, so your pet can\'t '
              'be double-fed.',
              style: TextStyle(
                fontSize: Sizes.base,
                height: 1.5,
                color: c.textMuted,
              ),
            ),
          ],
          const SizedBox(height: Space.s4),
          TextButton(
            onPressed: () => unawaited(
              Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) =>
                      MotorTraceScreen(feeder: feeder, feed: result.feed),
                ),
              ),
            ),
            child: const Text('Motor trace for this feed'),
          ),
        ],
      ),
    );
  }

  static String _hopper(FoodLevel level) => switch (level) {
    FoodLevel.present => 'Yes',
    FoodLevel.empty => 'Empty',
    FoodLevel.unread => 'Not checked',
  };
}
