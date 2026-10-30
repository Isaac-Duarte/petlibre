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
import 'clock_portion.dart';
import 'diagnostics.dart';
import 'people.dart';
import 'shell.dart' show confirmRestart;
import 'updates.dart';
import 'wifi.dart';

/// What a window comes to, in words.
String windowText(BuildContext context, Window window) => switch (window) {
  Window_Always() => 'Any time',
  Window_Between(:final start, :final end) =>
    '${minuteText(context, start)} to ${minuteText(context, end)}',
};

/// Change one setting, against the generation the screen shows.
Future<void> _put(
  BuildContext context,
  FeederController feeder,
  SettingsView settings,
  SettingsChanges changes,
) async {
  final reply = await feeder.ask(
    FeederRequest.putSettings(base: settings.generation, changes: changes),
  );
  if (context.mounted) showProblem(context, reply);
}

/// The feeder's settings (10.1), for a member what a member may change (10.3).
/// Each change goes back against the generation last read, so one made
/// meanwhile is refused rather than overwritten.
class SettingsTab extends StatelessWidget {
  const SettingsTab({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: feeder,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final c = context.colors;
    final settings = feeder.card?.settings;
    final owner = feeder.role == ClientRole.owner;
    if (settings == null) {
      return Center(
        child: Text(
          feeder.connection == Connection.online
              ? 'Reading the settings…'
              : "Can't read the settings right now.",
          style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
        ),
      );
    }
    final online = feeder.connection == Connection.online;

    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 16),
      children: [
        if (!owner) ...[
          Text.rich(
            TextSpan(
              style: TextStyle(
                fontSize: Sizes.base,
                height: 1.5,
                color: c.textMuted,
              ),
              children: [
                const TextSpan(text: "You're a "),
                TextSpan(
                  text: 'member',
                  style: TextStyle(fontWeight: Fonts.semibold, color: c.text),
                ),
                TextSpan(
                  text:
                      ' of ${feeder.known.name}. You can change lights, '
                      'sounds and reminders.',
                ),
              ],
            ),
          ),
          const SizedBox(height: Space.s3),
        ],
        const SectionLabel('Lights and sound'),
        Panel(
          child: Column(
            children: [
              _WindowRow(
                feeder: feeder,
                settings: settings,
                online: online,
                name: 'Lights',
                on: settings.lights,
                window: settings.lightsWindow,
                question: 'When can the lights be on?',
                toggle: (on) => SettingsChanges(lights: on),
                windowed: (window) => SettingsChanges(lightsWindow: window),
                first: true,
              ),
              _WindowRow(
                feeder: feeder,
                settings: settings,
                online: online,
                name: 'Sounds',
                on: settings.sound,
                window: settings.soundWindow,
                question: 'When can sounds play?',
                toggle: (on) => SettingsChanges(sound: on),
                windowed: (window) => SettingsChanges(soundWindow: window),
              ),
              _VolumeRow(feeder: feeder, settings: settings, online: online),
            ],
          ),
        ),
        const SizedBox(height: Space.s4),
        const SectionLabel('Reminders'),
        Panel(
          child: Column(
            children: [
              _ReminderRow(feeder: feeder, settings: settings, online: online),
              _DesiccantRow(feeder: feeder, settings: settings, online: online),
            ],
          ),
        ),
        const SizedBox(height: Space.s4),
        if (owner) _OwnerSection(feeder: feeder) else const _OwnerOnly(),
        const SizedBox(height: Space.s4),
        Row(
          children: [
            if (owner) ...[
              Expanded(
                child: OutlinedButton(
                  onPressed: online
                      ? () => unawaited(confirmRestart(context, feeder))
                      : null,
                  child: const Text('Restart'),
                ),
              ),
              const SizedBox(width: Space.s3),
            ],
            Expanded(
              child: OutlinedButton(
                style: OutlinedButton.styleFrom(foregroundColor: c.fault),
                onPressed: () => unawaited(_forget(context, feeder)),
                child: const Text('Remove from this phone'),
              ),
            ),
          ],
        ),
      ],
    );
  }

  /// This phone forgets the feeder. The feeder's list stays as it is, so an
  /// owner removes the key from People; a member cannot remove itself.
  static Future<void> _forget(
    BuildContext context,
    FeederController feeder,
  ) async {
    final name = feeder.known.name;
    final sure = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove $name from this phone?'),
        content: const Text(
          'It keeps feeding on its schedule. This phone stays on its People '
          'list until an owner removes it.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (sure != true || !context.mounted) return;
    Navigator.of(context).popUntil((route) => route.isFirst);
    await feeder.forgetHere();
  }
}

class _Tile extends StatelessWidget {
  const _Tile({required this.first, required this.child, this.onTap});

  final bool first;
  final Widget child;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => InkWell(
    onTap: onTap,
    child: Container(
      constraints: const BoxConstraints(minHeight: 52),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s4,
        vertical: Space.s2,
      ),
      decoration: BoxDecoration(
        border: first
            ? null
            : Border(top: BorderSide(color: context.colors.line)),
      ),
      child: child,
    ),
  );
}

/// A name, and what it stands at under it.
class _Words extends StatelessWidget {
  const _Words(this.name, [this.note]);

  final String name;
  final String? note;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(name, style: TextStyle(fontSize: 15, color: c.text)),
        if (note case final note?)
          Text(
            note,
            style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
          ),
      ],
    );
  }
}

class _WindowRow extends StatelessWidget {
  const _WindowRow({
    required this.feeder,
    required this.settings,
    required this.online,
    required this.name,
    required this.on,
    required this.window,
    required this.question,
    required this.toggle,
    required this.windowed,
    this.first = false,
  });

  final FeederController feeder;
  final SettingsView settings;
  final bool online;
  final String name;
  final bool on;
  final Window window;
  final String question;
  final SettingsChanges Function(bool) toggle;
  final SettingsChanges Function(Window) windowed;
  final bool first;

  @override
  Widget build(BuildContext context) => _Tile(
    first: first,
    onTap: online
        ? () async {
            final chosen = await showDialog<Window>(
              context: context,
              builder: (_) => _WindowDialog(question: question, window: window),
            );
            if (chosen != null && context.mounted) {
              await _put(context, feeder, settings, windowed(chosen));
            }
          }
        : null,
    child: Row(
      children: [
        Expanded(child: _Words(name, windowText(context, window))),
        Semantics(
          label: name,
          child: Switch(
            value: on,
            onChanged: online
                ? (next) =>
                      unawaited(_put(context, feeder, settings, toggle(next)))
                : null,
          ),
        ),
      ],
    ),
  );
}

/// When the lights or sounds may be on (10.2).
class _WindowDialog extends StatefulWidget {
  const _WindowDialog({required this.question, required this.window});

  final String question;
  final Window window;

  @override
  State<_WindowDialog> createState() => _WindowDialogState();
}

class _WindowDialogState extends State<_WindowDialog> {
  late bool _between = widget.window is Window_Between;
  late int _start = switch (widget.window) {
    Window_Between(:final start) => start,
    _ => 7 * 60,
  };
  late int _end = switch (widget.window) {
    Window_Between(:final end) => end,
    _ => 22 * 60,
  };

  Future<void> _pick(int minute, void Function(int) chosen) async {
    final picked = await showTimePicker(
      context: context,
      initialTime: TimeOfDay(hour: minute ~/ 60, minute: minute % 60),
    );
    if (picked != null) {
      setState(() => chosen(picked.hour * 60 + picked.minute));
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    scrollable: true,
    title: Text(widget.question),
    content: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        RadioGroup<bool>(
          groupValue: _between,
          onChanged: (between) => setState(() => _between = between ?? false),
          child: const Column(
            children: [
              RadioListTile<bool>(
                value: false,
                contentPadding: EdgeInsets.zero,
                title: Text('Any time'),
              ),
              RadioListTile<bool>(
                value: true,
                contentPadding: EdgeInsets.zero,
                title: Text('Only between'),
              ),
            ],
          ),
        ),
        if (_between)
          Row(
            children: [
              Expanded(
                child: OutlinedButton(
                  onPressed: () =>
                      unawaited(_pick(_start, (minute) => _start = minute)),
                  child: Text(minuteText(context, _start)),
                ),
              ),
              const Padding(
                padding: EdgeInsets.symmetric(horizontal: Space.s2),
                child: Text('and'),
              ),
              Expanded(
                child: OutlinedButton(
                  onPressed: () =>
                      unawaited(_pick(_end, (minute) => _end = minute)),
                  child: Text(minuteText(context, _end)),
                ),
              ),
            ],
          ),
        const SizedBox(height: Space.s3),
        Text(
          'A window can cross midnight. Until the feeder knows the time, it '
          'treats every window as open.',
          style: TextStyle(
            fontSize: Sizes.sm,
            height: 1.5,
            color: context.colors.textMuted,
          ),
        ),
      ],
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () => Navigator.pop(
          context,
          _between
              ? Window.between(start: _start, end: _end)
              : const Window.always(),
        ),
        child: const Text('Save'),
      ),
    ],
  );
}

class _VolumeRow extends StatefulWidget {
  const _VolumeRow({
    required this.feeder,
    required this.settings,
    required this.online,
  });

  final FeederController feeder;
  final SettingsView settings;
  final bool online;

  @override
  State<_VolumeRow> createState() => _VolumeRowState();
}

class _VolumeRowState extends State<_VolumeRow> {
  /// While a finger holds the slider; otherwise what the feeder said.
  double? _dragging;

  Future<void> _play() async {
    final reply = await widget.feeder.ask(
      const FeederRequest.play(tune: Tune.test),
    );
    if (mounted) showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final volume = _dragging ?? widget.settings.volume.toDouble();
    return _Tile(
      first: false,
      child: Row(
        children: [
          Text('Volume', style: TextStyle(fontSize: 15, color: c.text)),
          Expanded(
            child: Slider(
              value: volume,
              max: 100,
              onChanged: widget.online
                  ? (next) => setState(() => _dragging = next)
                  : null,
              onChangeEnd: (end) async {
                await _put(
                  context,
                  widget.feeder,
                  widget.settings,
                  SettingsChanges(volume: end.round()),
                );
                if (mounted) setState(() => _dragging = null);
              },
            ),
          ),
          SizedBox(
            width: 28,
            child: Text(
              '${volume.round()}',
              textAlign: TextAlign.end,
              style: TextStyle(fontSize: Sizes.base, color: c.text),
            ),
          ),
          IconButton(
            tooltip: 'Play a test sound',
            onPressed: widget.online ? () => unawaited(_play()) : null,
            icon: Icon(Icons.volume_up_outlined, color: c.textMuted),
          ),
        ],
      ),
    );
  }
}

class _ReminderRow extends StatelessWidget {
  const _ReminderRow({
    required this.feeder,
    required this.settings,
    required this.online,
  });

  final FeederController feeder;
  final SettingsView settings;
  final bool online;

  @override
  Widget build(BuildContext context) {
    final reminder = settings.mealReminder;
    return _Tile(
      first: true,
      onTap: online
          ? () async {
              final chosen = await showDialog<MealReminder>(
                context: context,
                builder: (context) => SimpleDialog(
                  title: const Text('Before each meal'),
                  children: [
                    for (final (name, value) in const [
                      ('Off', MealReminder.off()),
                      ('5 minutes ahead', MealReminder.before(minutes: 5)),
                      ('15 minutes ahead', MealReminder.before(minutes: 15)),
                      ('30 minutes ahead', MealReminder.before(minutes: 30)),
                      ('60 minutes ahead', MealReminder.before(minutes: 60)),
                    ])
                      SimpleDialogOption(
                        onPressed: () => Navigator.pop(context, value),
                        child: Text(name),
                      ),
                  ],
                ),
              );
              if (chosen != null && context.mounted) {
                await _put(
                  context,
                  feeder,
                  settings,
                  SettingsChanges(mealReminder: chosen),
                );
              }
            }
          : null,
      child: Row(
        children: [
          Expanded(
            child: _Words(
              'Before each meal',
              // The feeder records it: this app sends no notifications yet.
              switch (reminder) {
                MealReminder_Off() => 'Off',
                MealReminder_Before(:final minutes) =>
                  'Shows in History $minutes minutes ahead',
              },
            ),
          ),
          StrokeIcon(Glyph.chevron, size: 20, color: context.colors.textMuted),
        ],
      ),
    );
  }
}

class _DesiccantRow extends StatefulWidget {
  const _DesiccantRow({
    required this.feeder,
    required this.settings,
    required this.online,
  });

  final FeederController feeder;
  final SettingsView settings;
  final bool online;

  @override
  State<_DesiccantRow> createState() => _DesiccantRowState();
}

class _DesiccantRowState extends State<_DesiccantRow> {
  bool _asking = false;

  Future<void> _changed() async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(const FeederRequest.resetDesiccant());
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  Future<void> _period() async {
    final days = await showDialog<int>(
      context: context,
      builder: (context) => SimpleDialog(
        title: const Text('Change the desiccant every'),
        children: [
          for (final days in const [7, 14, 30, 60, 90])
            SimpleDialogOption(
              onPressed: () => Navigator.pop(context, days),
              child: Text('$days days'),
            ),
        ],
      ),
    );
    if (days != null && mounted) {
      await _put(
        context,
        widget.feeder,
        widget.settings,
        SettingsChanges(desiccantDays: days),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final desiccant = widget.feeder.card?.desiccant;
    final note = switch (desiccant) {
      Desiccant_Remaining(days: 0) => 'Change today',
      Desiccant_Remaining(days: 1) => 'Next change in 1 day',
      Desiccant_Remaining(:final days) when days > 1 =>
        'Next change in $days days',
      Desiccant_Remaining(:final days) => '${-days} days overdue',
      Desiccant_Unset() => 'Not started',
      Desiccant_Unknown() => 'Unknown until the clock is set',
      null => null,
    };
    return _Tile(
      first: false,
      onTap: widget.online ? () => unawaited(_period()) : null,
      child: Row(
        children: [
          Expanded(
            child: _Words(
              'Desiccant every ${widget.settings.desiccantDays} days',
              note,
            ),
          ),
          OutlinedButton(
            style: OutlinedButton.styleFrom(
              minimumSize: const Size(0, 40),
              padding: const EdgeInsets.symmetric(horizontal: Space.s3),
            ),
            onPressed: widget.online && !_asking ? _changed : null,
            child: const Text('Changed today'),
          ),
        ],
      ),
    );
  }
}

class _OwnerSection extends StatelessWidget {
  const _OwnerSection({required this.feeder});

  final FeederController feeder;

  void _open(BuildContext context, Widget screen) => unawaited(
    Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => screen)),
  );

  @override
  Widget build(BuildContext context) {
    final card = feeder.card;
    final zone = card?.time?.zone;
    final signal = switch (card?.rssi) {
      null => null,
      >= -60 => 'Strong signal',
      >= -72 => 'Fair signal',
      _ => 'Weak signal',
    };
    final people = feeder.clients.isEmpty
        ? null
        : '${feeder.clients.length} of $clientsMost';

    Widget link(
      String name,
      String? note,
      Widget screen, {
      bool first = false,
    }) => _Tile(
      first: first,
      onTap: () => _open(context, screen),
      child: Row(
        children: [
          Expanded(child: _Words(name, note)),
          StrokeIcon(Glyph.chevron, size: 20, color: context.colors.textMuted),
        ],
      ),
    );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SectionLabel('Feeder'),
        Panel(
          child: Column(
            children: [
              link('Wi-Fi', signal, WifiScreen(feeder: feeder), first: true),
              link(
                'Time zone',
                zone == null || zone.isEmpty ? 'Follows the phones' : zone,
                ClockPortionScreen(feeder: feeder),
              ),
              link('People', people, PeopleScreen(feeder: feeder)),
              link('Firmware', null, UpdatesScreen(feeder: feeder)),
              link('Diagnostics', null, DiagnosticsScreen(feeder: feeder)),
            ],
          ),
        ),
      ],
    );
  }
}

class _OwnerOnly extends StatelessWidget {
  const _OwnerOnly();

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SectionLabel('Owner only'),
        Panel(
          child: Column(
            children: [
              for (final (i, name) in const [
                'Wi-Fi',
                'Time zone',
                'Firmware updates',
                'Restart, remove people',
              ].indexed)
                _Tile(
                  first: i == 0,
                  child: Row(
                    children: [
                      StrokeIcon(Glyph.lock, size: 18, color: c.textMuted),
                      const SizedBox(width: Space.s3),
                      Text(
                        name,
                        style: TextStyle(
                          fontSize: Sizes.base,
                          color: c.textMuted,
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}
