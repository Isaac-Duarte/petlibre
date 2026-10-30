import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../state/fleet.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/replies.dart';
import 'history.dart';
import 'schedule.dart';
import 'settings_tab.dart';
import 'today.dart';

/// One feeder: its name, the menu, and the tabs (board 7).
class FeederShell extends StatelessWidget {
  const FeederShell({super.key, required this.feeder, this.initialTab = 0});

  /// The feeder `identity` of `fleet`, or a plain way back if it is gone: a
  /// link can outlive a feeder this phone has forgotten.
  static Widget of(FeederFleet fleet, String identity, {int initialTab = 0}) {
    final feeder = fleet[identity];
    return feeder == null
        ? const _Gone()
        : FeederShell(feeder: feeder, initialTab: initialTab);
  }

  final FeederController feeder;
  final int initialTab;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return DefaultTabController(
      length: 4,
      initialIndex: initialTab,
      child: Scaffold(
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
            feeder.known.name,
            style: TextStyle(
              fontSize: 20,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          actions: [
            PopupMenuButton<void>(
              tooltip: 'More',
              icon: Icon(Icons.more_vert, color: c.text),
              itemBuilder: (context) => [
                PopupMenuItem(
                  onTap: () => unawaited(confirmRestart(context, feeder)),
                  child: const Text('Restart'),
                ),
              ],
            ),
            const SizedBox(width: Space.s1),
          ],
          bottom: TabBar(
            labelColor: c.accent,
            unselectedLabelColor: c.textMuted,
            indicatorColor: c.accent,
            indicatorWeight: 3,
            indicatorSize: TabBarIndicatorSize.tab,
            dividerColor: c.line,
            labelStyle: const TextStyle(
              fontSize: Sizes.base,
              fontWeight: Fonts.semibold,
            ),
            unselectedLabelStyle: const TextStyle(
              fontSize: Sizes.base,
              fontWeight: Fonts.medium,
            ),
            tabs: const [
              Tab(text: 'Today'),
              Tab(text: 'Schedule'),
              Tab(text: 'History'),
              Tab(text: 'Settings'),
            ],
          ),
        ),
        body: TabBarView(
          children: [
            TodayTab(feeder: feeder),
            ScheduleTab(feeder: feeder),
            HistoryTab(feeder: feeder),
            SettingsTab(feeder: feeder),
          ],
        ),
      ),
    );
  }
}

/// Ask, then restart. The feeder refuses a restart while a feed runs; one
/// that goes unanswered may have gone through, and is not sent again.
Future<void> confirmRestart(
  BuildContext context,
  FeederController feeder,
) async {
  final sure = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text('Restart ${feeder.known.name}?'),
      content: const Text(
        "It's back in about a minute. It can't feed during the restart.",
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Restart'),
        ),
      ],
    ),
  );
  if (sure != true) return;

  final reply = await feeder.ask(const FeederRequest.reboot());
  if (context.mounted && showProblem(context, reply)) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text('${feeder.known.name} is restarting.')),
    );
  }
}

class _Gone extends StatelessWidget {
  const _Gone();

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(),
    body: const Center(child: Text('This phone no longer knows that feeder.')),
  );
}
