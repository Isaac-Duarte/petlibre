import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_reactive_ble/flutter_reactive_ble.dart'
    show DiscoveredDevice;
import 'package:go_router/go_router.dart';

import '../src/rust/api/model.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/code_tiles.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';

/// What a pairing shows, phase by phase: the search, the code with its
/// countdown, the feeder rejoining, then success or why it stopped.
class PairingView extends StatelessWidget {
  const PairingView({
    super.key,
    required this.flow,
    required this.ble,
    required this.onRetry,
  });

  final PairingFlow flow;

  /// BLE setup (`true`) or LAN recovery: the wording differs.
  final bool ble;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: flow,
    builder: (context, _) => switch (flow.phase) {
      PairingPhase.finding => _Find(flow: flow),
      PairingPhase.connecting => _Waiting(
        stepLabel: ble ? 'Step 4 of 4' : null,
        title: 'Connecting',
        body: 'Starting a private session with the feeder.',
      ),
      PairingPhase.code => _Code(flow: flow, ble: ble),
      PairingPhase.joining =>
        ble
            ? _Joining(flow: flow)
            : const _Waiting(
                title: 'Code accepted',
                body: 'Saving the feeder on this phone.',
              ),
      PairingPhase.done => _Ready(flow: flow),
      PairingPhase.failed => _Failed(flow: flow, ble: ble, onRetry: onRetry),
    },
  );
}

class _Waiting extends StatelessWidget {
  const _Waiting({required this.title, required this.body, this.stepLabel});

  final String title;
  final String body;
  final String? stepLabel;

  @override
  Widget build(BuildContext context) => StepScaffold(
    stepLabel: stepLabel,
    progress: stepLabel == null ? null : 1,
    title: title,
    body: body,
    content: const [
      SizedBox(height: Space.s6),
      Center(child: CircularProgressIndicator()),
    ],
  );
}

/// Feeders in setup mode nearby; the owner picks one.
class _Find extends StatelessWidget {
  const _Find({required this.flow});

  final PairingFlow flow;

  /// Weaker than this, the owner is asked to move closer.
  static const _weak = -75;

  @override
  Widget build(BuildContext context) {
    final found = flow.found.values.toList()
      ..sort((a, b) => b.rssi.compareTo(a.rssi));
    return StepScaffold(
      stepLabel: 'Step 4 of 4',
      progress: 1,
      title: 'Looking for feeders in setup mode',
      content: [
        const Searching('Searching nearby…'),
        for (final (i, device) in found.indexed) ...[
          const SizedBox(height: Space.s4),
          _Found(
            device: device,
            closest: i == 0,
            weak: device.rssi < _weak,
            onConnect: () => flow.choose(device),
          ),
        ],
      ],
      actions: [
        if (found.length > 1)
          const Notice(
            tone: Tone.neutral,
            radius: Radii.card,
            lead: 'Two feeders showing?',
            text:
                'Connect to either. The code on the next screen only works '
                'on the one whose buttons you press.',
          ),
      ],
    );
  }
}

class _Found extends StatelessWidget {
  const _Found({
    required this.device,
    required this.closest,
    required this.weak,
    required this.onConnect,
  });

  final DiscoveredDevice device;
  final bool closest;
  final bool weak;
  final VoidCallback onConnect;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    const small = Size(0, 40);
    return Panel(
      highlight: closest,
      padding: const EdgeInsets.all(Space.s4),
      child: Row(
        children: [
          Opacity(opacity: weak ? 0.6 : 1, child: const FeederImage(size: 56)),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Wellfed feeder',
                  style: TextStyle(
                    fontSize: Sizes.md,
                    fontWeight: Fonts.semibold,
                    color: c.text,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  weak
                      ? 'In setup mode · weak signal, move closer'
                      : 'In setup mode · close by',
                  style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
                ),
              ],
            ),
          ),
          const SizedBox(width: Space.s2),
          if (closest)
            FilledButton(
              style: FilledButton.styleFrom(
                minimumSize: small,
                textStyle: const TextStyle(
                  fontSize: Sizes.base,
                  fontWeight: Fonts.semibold,
                ),
              ),
              onPressed: onConnect,
              child: const Text('Connect'),
            )
          else
            OutlinedButton(
              style: OutlinedButton.styleFrom(
                minimumSize: small,
                textStyle: const TextStyle(
                  fontSize: Sizes.base,
                  fontWeight: Fonts.semibold,
                ),
              ),
              onPressed: onConnect,
              child: const Text('Connect'),
            ),
        ],
      ),
    );
  }
}

class _Code extends StatefulWidget {
  const _Code({required this.flow, required this.ble});

  final PairingFlow flow;
  final bool ble;

  @override
  State<_Code> createState() => _CodeState();
}

class _CodeState extends State<_Code> {
  late final Timer _tick;

  /// The whole window, for the bar's fill.
  late final int _window = _left();

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  int _left() {
    final deadline = widget.flow.deadline;
    return deadline == null
        ? 0
        : deadline.difference(DateTime.now()).inSeconds.clamp(0, 999);
  }

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final left = _left();
    final clock = '${left ~/ 60}:${(left % 60).toString().padLeft(2, '0')}';
    return StepScaffold(
      stepLabel: widget.ble ? 'Step 4 of 4' : null,
      progress: widget.ble ? 1 : null,
      title: 'Press these buttons on the feeder, in order',
      body:
          'One press each. The feeder clicks with every press. This proves '
          "you're standing next to it.",
      content: [
        CodeTiles(widget.flow.code),
        const SizedBox(height: 18),
        const RingLegend(),
        const SizedBox(height: 18),
        Row(
          children: [
            Expanded(
              child: Bar(
                value: _window == 0 ? 0 : left / _window,
                color: left > 10 ? c.accent : c.warning,
              ),
            ),
            const SizedBox(width: Space.s3),
            Text(
              clock,
              style: TextStyle(
                fontSize: 15,
                fontWeight: Fonts.semibold,
                color: left > 10 ? c.text : c.warning,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ],
        ),
      ],
      actions: [
        Text(
          left > 0
              ? 'Pressed a wrong button? Let the time run out and start '
                    'again with a new code.'
              : 'Waiting for the feeder to confirm…',
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: Sizes.base,
            height: 1.5,
            color: c.textMuted,
          ),
        ),
      ],
    );
  }
}

/// BLE setup's last stretch: the feeder restarts onto its Wi-Fi and the
/// phone finds it there.
class _Joining extends StatelessWidget {
  const _Joining({required this.flow});

  final PairingFlow flow;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final network = flow.network;
    Widget done(String text) => Row(
      children: [
        SizedBox.square(
          dimension: 28,
          child: Center(
            child: StrokeIcon(Glyph.check, size: 22, color: c.ok, weight: 2.2),
          ),
        ),
        const SizedBox(width: Space.s3),
        Expanded(
          child: Text(text, style: TextStyle(fontSize: 15, color: c.text)),
        ),
      ],
    );

    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(28, 72, 28, 28),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Center(child: FeederImage(size: 180)),
              const SizedBox(height: 28),
              Headline(
                '${flow.feederName} is joining your Wi-Fi',
                size: 24,
                align: TextAlign.center,
              ),
              const SizedBox(height: 28),
              done('This phone added as owner'),
              const SizedBox(height: Space.s4),
              done(
                flow.wifiSaved == WifiSaved.unchanged
                    ? 'The feeder keeps its Wi-Fi'
                    : 'Wi-Fi saved on the feeder',
              ),
              const SizedBox(height: Space.s4),
              done('Feeder restarting without Bluetooth'),
              const SizedBox(height: Space.s4),
              Row(
                children: [
                  SizedBox.square(
                    dimension: 28,
                    child: CircularProgressIndicator(
                      strokeWidth: 3,
                      color: c.accent,
                      backgroundColor: c.sunken,
                    ),
                  ),
                  const SizedBox(width: Space.s3),
                  Text(
                    'Finding it on your network',
                    style: TextStyle(
                      fontSize: 15,
                      fontWeight: Fonts.semibold,
                      color: c.text,
                    ),
                  ),
                ],
              ),
              const Spacer(),
              Text(
                network == null
                    ? 'Keep this phone on its network. This usually takes '
                          'under a minute.'
                    : 'Keep this phone on $network. This usually takes '
                          'under a minute.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: Sizes.base,
                  height: 1.5,
                  color: c.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Ready extends StatelessWidget {
  const _Ready({required this.flow});

  final PairingFlow flow;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final name = flow.feederName;
    final network = flow.network;
    final body = switch ((flow.enrolled, network)) {
      (Enrolled.already, _) =>
        'This phone was already one of its owners; nothing changed.',
      (_, final network?) =>
        "It's on $network and you're its owner. You can feed it now from "
            'this phone.',
      _ => "You're its owner. You can feed it now from this phone.",
    };

    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(28, Space.s7, 28, 28),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: Center(
                  child: Stack(
                    children: [
                      const FeederImage(size: 290),
                      Positioned(
                        right: 0,
                        top: 0,
                        child: StrokeIcon(
                          Glyph.check,
                          size: 56,
                          color: c.ok,
                          weight: 2.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              Headline('$name is ready.', size: 36),
              const SizedBox(height: Space.s4),
              Text(
                body,
                style: TextStyle(
                  fontSize: Sizes.md,
                  height: 1.5,
                  color: c.textMuted,
                ),
              ),
              const SizedBox(height: Space.s5),
              FilledButton(
                onPressed: () => context.go('/'),
                child: Text('Go to $name'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Failed extends StatelessWidget {
  const _Failed({required this.flow, required this.ble, required this.onRetry});

  final PairingFlow flow;
  final bool ble;
  final VoidCallback onRetry;

  (String, String) _words() => switch (flow.failure) {
    PairingFailure.notFound => (
      'No feeder in setup mode nearby',
      'On an unlocked feeder that is plugged in, hold Lock and Mic together '
          'for 5 seconds, until the Wi-Fi light blinks slowly. Then try '
          'again.',
    ),
    PairingFailure.link => (
      'The Bluetooth link dropped',
      'Move closer to the feeder and try again. It stays in setup mode.',
    ),
    PairingFailure.refused => _refusal(flow.refusal),
    PairingFailure.mismatch => (
      "That code didn't match",
      'The feeder heard a different order of presses. Nothing was changed '
          'on it. If it happens again, another device may be in the way. '
          'Try again with a new code.',
    ),
    PairingFailure.timeout => (
      'The feeder stopped answering',
      'Try again. If the code was showing, its time may have run out.',
    ),
    PairingFailure.windowClosed => (
      'The feeder is not taking an owner now',
      'On an unlocked panel, hold Lock, then also Dispense, for 5 seconds. '
          'It takes a new owner for 2 minutes. Then try again.',
    ),
    PairingFailure.notOnLan => (
      'Paired, but not on this network',
      'The feeder is saved on this phone, but it did not show up here. '
          'Check that this phone is on the network you gave the feeder; '
          'Home keeps looking.',
    ),
    PairingFailure.input => (
      'Check the names and Wi-Fi',
      'The phone name is 1 to 16 characters. A Wi-Fi password is 8 to 63 '
          'characters.',
    ),
    PairingFailure.other ||
    null => ("Setup didn't finish", flow.message ?? 'Try again.'),
  };

  static (String, String) _refusal(SetupRefusal? refusal) => switch (refusal) {
    SetupRefusal.busy => (
      'Busy with another phone',
      'Only one phone sets it up at a time. Wait for it to finish, then try '
          'again.',
    ),
    SetupRefusal.unconfirmed => (
      'Time ran out',
      'The code works for 60 seconds. Try again with a new code.',
    ),
    SetupRefusal.wrongCode => (
      "That code didn't match",
      'The feeder heard a different order of presses and played its alert '
          'sound. Nothing was changed on it. Try again with a new code.',
    ),
    SetupRefusal.full => (
      'The feeder is full',
      'It holds 8 phones and servers. An owner removes one in its People '
          'list first.',
    ),
    SetupRefusal.notWritable => (
      "The feeder can't save right now",
      'Try again in a minute.',
    ),
    SetupRefusal.invalid || null => (
      'The feeder rejected the name or Wi-Fi',
      'Check the phone name and the Wi-Fi password.',
    ),
  };

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final (title, body) = _words();
    final saved = flow.failure == PairingFailure.notOnLan;
    return StepScaffold(
      leading: Leading.close,
      onLeading: () => context.go('/'),
      top: Align(
        alignment: Alignment.centerLeft,
        child: StrokeIcon(
          saved ? Glyph.wifiOff : Glyph.failed,
          size: 40,
          color: saved ? c.warning : c.fault,
          weight: 2,
        ),
      ),
      title: title,
      body: body,
      content: [
        if (saved)
          FilledButton(
            onPressed: () => context.go('/'),
            child: const Text('Go to Home'),
          )
        else
          FilledButton(
            onPressed: onRetry,
            child: const Text('Try again with a new code'),
          ),
        if (ble && !saved) ...[
          const SizedBox(height: Space.s6),
          const SectionLabel('Other reasons setup stops'),
          const SizedBox(height: Space.s3),
          const ReasonList([
            (
              'Time ran out',
              'The code works for 60 seconds. Ask for a new one.',
            ),
            (
              'The feeder is full',
              'It holds 8 phones and servers. Remove one in its People list.',
            ),
            (
              'Setup mode ended',
              'After 15 idle minutes the feeder leaves setup mode. Hold Lock '
                  'and Mic again.',
            ),
            ('Busy with another phone', 'Only one phone sets it up at a time.'),
          ]),
        ],
      ],
    );
  }
}
