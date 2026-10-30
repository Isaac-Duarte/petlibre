import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import '../../widgets/when.dart';
import 'meal_editor.dart';
import 'no_time.dart';

/// The most meals a feeder keeps: what Rust reads back, all ten slots.
int mealsMost(PlansView plans) => plans.plans.length;

/// A meal's days in words.
String daysText(List<Weekday> days) {
  if (days.isEmpty) return 'once';
  if (days.length == 7) return 'every day';
  const week = [
    Weekday.mon,
    Weekday.tue,
    Weekday.wed,
    Weekday.thu,
    Weekday.fri,
  ];
  const weekend = [Weekday.sat, Weekday.sun];
  if (days.length == 5 && week.every(days.contains)) return 'Mon to Fri';
  if (days.length == 2 && weekend.every(days.contains)) return 'weekends';
  return [
    for (final day in Weekday.values)
      if (days.contains(day))
        '${day.name[0].toUpperCase()}${day.name.substring(1)}',
  ].join(', ');
}

String _calls(int plays) => switch (plays) {
  1 => 'meal call once',
  2 => 'meal call twice',
  _ => 'meal call $plays times',
};

/// The feeder's meals (8.1), with its limit and hold (8.3). The table is the
/// feeder's: this shows the last it said, and each change goes back against
/// that reading.
class ScheduleTab extends StatelessWidget {
  const ScheduleTab({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: feeder,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final c = context.colors;
    final card = feeder.card;
    final plans = card?.plans;
    if (plans == null) {
      return Center(
        child: Text(
          feeder.connection == Connection.online
              ? 'Reading the meals…'
              : "Can't read the meals right now.",
          style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
        ),
      );
    }

    final meals = [
      for (final (slot, plan) in plans.plans.indexed)
        if (plan != null) (slot, plan),
    ]..sort((a, b) => a.$2.minute.compareTo(b.$2.minute));
    final most = mealsMost(plans);
    final full = meals.length >= most;
    final online = feeder.connection == Connection.online;
    final next = card?.nextMeal;
    final muted = TextStyle(fontSize: Sizes.base, color: c.textMuted);

    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: ListView(
              children: [
                if (online && !(card?.clockSet ?? true)) ...[
                  NoTimeNotice(feeder: feeder),
                  const SizedBox(height: Space.s3),
                ],
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        next == null
                            ? 'No meal is coming'
                            : 'Next: ${_day(next.atMs)} at '
                                  '${minuteText(context, next.minute)}',
                        style: TextStyle(
                          fontSize: Sizes.base,
                          fontWeight: Fonts.semibold,
                          color: c.text,
                        ),
                      ),
                    ),
                    Flexible(
                      child: Text(
                        full
                            ? "$most of $most meals · the feeder's limit"
                            : '${meals.length} of $most meals',
                        textAlign: TextAlign.end,
                        style: muted,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: Space.s3),
                if (meals.isNotEmpty)
                  Panel(
                    child: Column(
                      children: [
                        for (final (i, (slot, plan)) in meals.indexed)
                          _MealRow(
                            feeder: feeder,
                            plans: plans,
                            slot: slot,
                            plan: plan,
                            first: i == 0,
                            online: online,
                          ),
                      ],
                    ),
                  ),
                const SizedBox(height: Space.s3),
                Text(
                  'The feeder keeps these itself and feeds on time even when '
                  'no phone is around. A meal more than 15 minutes late is '
                  'skipped and shows in History.',
                  style: muted.copyWith(height: 1.5),
                ),
              ],
            ),
          ),
          const SizedBox(height: Space.s3),
          FilledButton(
            onPressed: online && !full
                ? () => unawaited(
                    openMealEditor(context, feeder: feeder, held: plans),
                  )
                : null,
            child: Text(full ? 'Add meal · delete one first' : 'Add meal'),
          ),
        ],
      ),
    );
  }

  static String _day(int atMs) {
    final day = dayText(DateTime.fromMillisecondsSinceEpoch(atMs));
    return day == 'Today' || day == 'Tomorrow' ? day.toLowerCase() : day;
  }
}

class _MealRow extends StatefulWidget {
  const _MealRow({
    required this.feeder,
    required this.plans,
    required this.slot,
    required this.plan,
    required this.first,
    required this.online,
  });

  final FeederController feeder;
  final PlansView plans;
  final int slot;
  final PlanView plan;
  final bool first;
  final bool online;

  @override
  State<_MealRow> createState() => _MealRowState();
}

class _MealRowState extends State<_MealRow> {
  bool _asking = false;

  Future<void> _set(bool enabled) async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(
      FeederRequest.editPlans(
        plans: widget.plans,
        edit: PlanEdit.enabled(slot: widget.slot, enabled: enabled),
      ),
    );
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final plan = widget.plan;
    final time = minuteText(context, plan.minute);
    final parts = [
      portionsText(plan.portions),
      daysText(plan.days),
      if (plan.call > 0) _calls(plan.call),
    ].join(' · ');

    return InkWell(
      onTap: widget.online
          ? () => unawaited(
              openMealEditor(
                context,
                feeder: widget.feeder,
                held: widget.plans,
                slot: widget.slot,
              ),
            )
          : null,
      child: Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.s4,
          vertical: Space.s2,
        ),
        decoration: BoxDecoration(
          border: widget.first ? null : Border(top: BorderSide(color: c.line)),
        ),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    time,
                    style: TextStyle(
                      fontSize: Sizes.md,
                      fontWeight: Fonts.semibold,
                      color: plan.enabled ? c.text : c.textMuted,
                    ),
                  ),
                  Text(
                    parts,
                    style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
                  ),
                ],
              ),
            ),
            Semantics(
              label: '$time ${plan.enabled ? 'on' : 'off'}',
              child: Switch(
                // What the feeder last said: an answer moves it.
                value: plan.enabled,
                onChanged: widget.online && !_asking ? _set : null,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A row that says what the meal call is, for the editor and the settings.
class CallLink extends StatelessWidget {
  const CallLink({super.key, required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: Space.s1),
        child: Row(
          children: [
            Expanded(
              child: Text(
                'Your recording, before the food drops',
                style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
              ),
            ),
            StrokeIcon(Glyph.chevron, size: 18, color: c.textMuted),
          ],
        ),
      ),
    );
  }
}
