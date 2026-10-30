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
import 'meal_call.dart';
import 'schedule.dart' show CallLink;

/// Open the editor over the plans the owner sees; `slot` for an existing
/// meal, none for a new one.
Future<void> openMealEditor(
  BuildContext context, {
  required FeederController feeder,
  required PlansView held,
  int? slot,
}) => Navigator.of(context).push(
  MaterialPageRoute<void>(
    fullscreenDialog: true,
    builder: (_) => MealEditor(feeder: feeder, held: held, slot: slot),
  ),
);

const _everyDay = Weekday.values;
const _weekdays = [
  Weekday.mon,
  Weekday.tue,
  Weekday.wed,
  Weekday.thu,
  Weekday.fri,
];
const _weekend = [Weekday.sat, Weekday.sun];

/// Edit one meal (8.2). A save goes back against the table this opened on, so a
/// change meanwhile (another client's, or a one-off meal disabling itself as it
/// runs) is refused and asked about (8.4), never overwritten unseen.
class MealEditor extends StatefulWidget {
  const MealEditor({
    super.key,
    required this.feeder,
    required this.held,
    this.slot,
  });

  final FeederController feeder;
  final PlansView held;
  final int? slot;

  @override
  State<MealEditor> createState() => _MealEditorState();
}

class _MealEditorState extends State<MealEditor> {
  late int _minute;
  late int _portions;
  late Set<Weekday> _days;
  late int _call;
  late bool _enabled;
  bool _asking = false;

  /// What the next save goes against.
  late PlansView _held = widget.held;

  PlanView? get _original {
    final slot = widget.slot;
    return slot == null ? null : widget.held.plans[slot];
  }

  @override
  void initState() {
    super.initState();
    final plan = _original;
    _minute = plan?.minute ?? 8 * 60;
    _portions = plan?.portions ?? 1;
    _days = (plan?.days ?? _everyDay).toSet();
    _call = plan?.call ?? 0;
    _enabled = plan?.enabled ?? true;
  }

  PlanView get _plan => PlanView(
    minute: _minute,
    portions: _portions,
    days: [
      for (final day in Weekday.values)
        if (_days.contains(day)) day,
    ],
    enabled: _enabled,
    call: _call,
  );

  PlanEdit _edit(PlansView against) {
    final slot = widget.slot;
    final plan = _plan;
    // The slot emptied meanwhile: "Save mine" brings the meal back.
    return slot == null || against.plans[slot] == null
        ? PlanEdit.add(plan: plan)
        : PlanEdit.change(
            slot: slot,
            minute: plan.minute,
            portions: plan.portions,
            days: plan.days,
            call: plan.call,
            enabled: plan.enabled,
          );
  }

  Future<void> _save() async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(
      FeederRequest.editPlans(plans: _held, edit: _edit(_held)),
    );
    if (!mounted) return;
    setState(() => _asking = false);

    if (reply is FeederReply_Refused && reply.refusal is FeederRefusal_Stale) {
      await _changedMeanwhile(
        wrote: "Your change wasn't saved, so nothing was overwritten.",
        yours: (context) => [('Your version', _line(context, _plan))],
        keep: "Keep what's there",
        go: 'Save mine over it',
        again: _save,
      );
    } else if (showProblem(context, reply)) {
      Navigator.pop(context);
    }
  }

  /// The meals moved first (8.4): show them, and let the owner choose. Going on
  /// is a new write of their choosing, against what the feeder now holds; nothing
  /// here sends the edit by itself.
  Future<void> _changedMeanwhile({
    required String wrote,
    required List<(String, String)> Function(BuildContext) yours,
    required String keep,
    required String go,
    required Future<void> Function() again,
  }) async {
    final reply = await widget.feeder.ask(const FeederRequest.getPlans());
    if (!mounted) return;
    if (reply is! FeederReply_Plans) {
      showProblem(context, reply);
      return;
    }

    final slot = widget.slot;
    final theirs = slot == null ? null : reply.plans.plans[slot];
    final mine = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        scrollable: true,
        title: const Text('The meals changed while you were editing'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('Someone else changed them, or a one-off meal ran. $wrote'),
            const SizedBox(height: Space.s3),
            FactList(sunken: true, [
              if (slot != null)
                (
                  'Now on the feeder',
                  Flexible(
                    child: Text(
                      theirs == null ? 'Deleted' : _line(context, theirs),
                      textAlign: TextAlign.end,
                    ),
                  ),
                ),
              for (final (name, line) in yours(context))
                (name, Flexible(child: Text(line, textAlign: TextAlign.end))),
            ]),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: Text(keep),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(go),
          ),
        ],
      ),
    );
    if (!mounted) return;

    if (mine == true) {
      _held = reply.plans;
      await again();
    } else if (mine == false) {
      Navigator.pop(context);
    }
  }

  static String _line(BuildContext context, PlanView plan) =>
      '${minuteText(context, plan.minute)} · ${portionsText(plan.portions)}';

  Future<void> _delete() async {
    final slot = widget.slot;
    if (slot == null) return;
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(
      FeederRequest.editPlans(
        plans: _held,
        edit: PlanEdit.delete(slot: slot),
      ),
    );
    if (!mounted) return;
    setState(() => _asking = false);

    if (reply is FeederReply_Refused && reply.refusal is FeederRefusal_Stale) {
      await _changedMeanwhile(
        wrote: "This meal wasn't deleted.",
        yours: (_) => const [],
        keep: 'Keep it',
        go: 'Delete it',
        again: _delete,
      );
    } else if (reply is FeederReply_NotAsked &&
        reply.reason == NotAsked.emptySlot) {
      // Gone already, which is what the owner asked for.
      Navigator.pop(context);
    } else if (showProblem(context, reply)) {
      Navigator.pop(context);
    }
  }

  Future<void> _pickTime() async {
    final picked = await showTimePicker(
      context: context,
      initialTime: TimeOfDay(hour: _minute ~/ 60, minute: _minute % 60),
    );
    if (picked != null) {
      setState(() => _minute = picked.hour * 60 + picked.minute);
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final time = minuteText(context, _minute);
    final label = TextStyle(
      fontSize: Sizes.base,
      fontWeight: Fonts.semibold,
      color: c.text,
    );
    final muted = TextStyle(fontSize: Sizes.sm, color: c.textMuted);
    bool same(List<Weekday> days) =>
        _days.length == days.length && days.every(_days.contains);

    Widget choice<T>(
      List<(String, T)> options,
      bool Function(T) selected,
      void Function(T) choose,
    ) => Wrap(
      spacing: Space.s2,
      runSpacing: Space.s2,
      children: [
        for (final (name, value) in options)
          ChoiceChip(
            label: Text(name),
            selected: selected(value),
            onSelected: (_) => setState(() => choose(value)),
          ),
      ],
    );

    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Close',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.close, color: c.text),
        ),
        title: Text(
          widget.slot == null
              ? 'New meal'
              : 'Meal at ${_original == null ? time : minuteText(context, _original!.minute)}',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
        actions: [
          TextButton(
            onPressed: _asking ? null : _save,
            child: const Text('Save'),
          ),
          const SizedBox(width: Space.s2),
        ],
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.s4),
        children: [
          OutlinedButton(
            onPressed: _pickTime,
            style: OutlinedButton.styleFrom(minimumSize: const Size(0, 72)),
            child: Text(
              time,
              style: TextStyle(
                fontSize: Sizes.xl,
                fontWeight: Fonts.semibold,
                color: c.text,
              ),
            ),
          ),
          const SizedBox(height: Space.s4),
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Portions', style: label),
                    Text('1 to 48', style: muted),
                  ],
                ),
              ),
              IconButton.outlined(
                tooltip: 'One fewer',
                onPressed: _portions > 1
                    ? () => setState(() => _portions--)
                    : null,
                icon: const StrokeIcon(Glyph.minus),
              ),
              SizedBox(
                width: 56,
                child: Text(
                  '$_portions',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: Sizes.lg,
                    fontWeight: Fonts.semibold,
                    color: c.text,
                  ),
                ),
              ),
              IconButton.outlined(
                tooltip: 'One more',
                onPressed: _portions < 48
                    ? () => setState(() => _portions++)
                    : null,
                icon: const StrokeIcon(Glyph.add),
              ),
            ],
          ),
          const SizedBox(height: Space.s4),
          Text('Repeat', style: label),
          const SizedBox(height: Space.s2),
          choice<List<Weekday>>(
            const [
              ('Daily', _everyDay),
              ('Weekdays', _weekdays),
              ('Weekends', _weekend),
              ('Once', <Weekday>[]),
            ],
            same,
            (days) => _days = days.toSet(),
          ),
          const SizedBox(height: Space.s2),
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              for (final day in Weekday.values)
                _DayToggle(
                  day: day,
                  on: _days.contains(day),
                  onTap: () => setState(() {
                    if (!_days.remove(day)) _days.add(day);
                  }),
                ),
            ],
          ),
          const SizedBox(height: Space.s4),
          Text('Meal call', style: label),
          CallLink(
            onTap: () => unawaited(
              Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => MealCallScreen(feeder: widget.feeder),
                ),
              ),
            ),
          ),
          const SizedBox(height: Space.s2),
          choice<int>(
            const [
              ('Off', 0),
              ('1', 1),
              ('2', 2),
              ('3', 3),
              ('4', 4),
              ('5', 5),
            ],
            (plays) => plays == _call,
            (plays) => _call = plays,
          ),
          const SizedBox(height: Space.s3),
          Row(
            children: [
              Expanded(child: Text('On', style: label)),
              Switch(
                value: _enabled,
                onChanged: (on) => setState(() => _enabled = on),
              ),
            ],
          ),
          const SizedBox(height: Space.s2),
          Text(
            'Changes start from the next $time. A meal that already ran '
            "today won't run again.",
            style: muted.copyWith(height: 1.5),
          ),
          if (widget.slot != null) ...[
            const SizedBox(height: Space.s4),
            OutlinedButton(
              style: OutlinedButton.styleFrom(foregroundColor: c.fault),
              onPressed: _asking ? null : _delete,
              child: const Text('Delete meal'),
            ),
          ],
        ],
      ),
    );
  }
}

class _DayToggle extends StatelessWidget {
  const _DayToggle({required this.day, required this.on, required this.onTap});

  final Weekday day;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final letter = day.name[0].toUpperCase();
    return Semantics(
      button: true,
      selected: on,
      label: day.name,
      child: InkWell(
        customBorder: const CircleBorder(),
        onTap: onTap,
        child: Container(
          width: 40,
          height: 40,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: on ? c.accent : null,
            shape: BoxShape.circle,
            border: on ? null : Border.all(color: c.lineStrong),
          ),
          child: ExcludeSemantics(
            child: Text(
              letter,
              style: TextStyle(
                fontSize: Sizes.base,
                fontWeight: Fonts.semibold,
                color: on ? c.accentText : c.text,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
