import 'dart:async';

import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../src/rust/api/model.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/code_tiles.dart';
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
      PairingPhase.finding => const _Waiting(
        title: 'Looking for your feeder',
        body:
            'Keep the phone near the feeder. A feeder in setup mode stays '
            'findable for 15 minutes.',
      ),
      PairingPhase.connecting => const _Waiting(
        title: 'Connecting',
        body: 'Starting a private session with the feeder.',
      ),
      PairingPhase.code => _Code(flow: flow),
      PairingPhase.joining => _Waiting(
        title: ble ? 'The feeder is joining your Wi-Fi' : 'Paired',
        body: ble
            ? 'It restarts without Bluetooth and joins the network you '
                  'gave it. Keep this phone on the same network.'
            : 'Saving the feeder on this phone.',
      ),
      PairingPhase.done => StepScaffold(
        title: 'You own this feeder',
        body: flow.enrolled == Enrolled.already
            ? 'This phone was already one of its owners; nothing changed.'
            : 'This phone is now an owner. It can feed, change settings '
                  'and invite others.',
        actions: [
          FilledButton(
            onPressed: () => context.go('/'),
            child: const Text('Go to the feeder'),
          ),
        ],
      ),
      PairingPhase.failed => _Failed(flow: flow, ble: ble, onRetry: onRetry),
    },
  );
}

class _Waiting extends StatelessWidget {
  const _Waiting({required this.title, required this.body});

  final String title;
  final String body;

  @override
  Widget build(BuildContext context) => StepScaffold(
    title: title,
    body: body,
    content: const [
      SizedBox(height: Space.s6),
      Center(child: CircularProgressIndicator()),
    ],
  );
}

class _Code extends StatefulWidget {
  const _Code({required this.flow});

  final PairingFlow flow;

  @override
  State<_Code> createState() => _CodeState();
}

class _CodeState extends State<_Code> {
  late final Timer _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final deadline = widget.flow.deadline;
    final left = deadline == null
        ? 0
        : deadline.difference(DateTime.now()).inSeconds.clamp(0, 999);
    return StepScaffold(
      title: 'Press these buttons on the feeder',
      body:
          'One at a time, left to right. The feeder ticks for each press. '
          'The code proves this phone is talking to your feeder and is '
          'used once.',
      content: [
        CodeTiles(widget.flow.code),
        const SizedBox(height: Space.s5),
        Text(
          left > 0 ? '$left s left' : 'Checking the code with the feeder…',
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: Sizes.md,
            fontWeight: Fonts.medium,
            color: left > 10 ? c.textMuted : c.warning,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
        const SizedBox(height: Space.s5),
        Text(
          'Pressed a wrong button? Let the time run out and start again; '
          'the feeder stays in setup mode.',
          style: TextStyle(fontSize: Sizes.sm, color: c.textMuted, height: 1.4),
        ),
      ],
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
      'On an unlocked feeder plugged into mains, hold Lock, then Mic, for '
          '5 seconds. Then try again.',
    ),
    PairingFailure.link => (
      'The Bluetooth link dropped',
      'Move closer to the feeder and try again. It stays in setup mode.',
    ),
    PairingFailure.refused => _refusal(flow.refusal),
    PairingFailure.mismatch => (
      'The code did not match',
      'Nothing was sent to the feeder. A press was probably mistyped; if '
          'it happens again, another device may be in the way. Try again '
          'with a new code.',
    ),
    PairingFailure.timeout => (
      'The feeder stopped answering',
      'Try again. If the code was showing, the window may have run out.',
    ),
    PairingFailure.windowClosed => (
      'The feeder is not taking an owner now',
      'On an unlocked panel, hold Lock, then Dispense, to open its '
          'window for 2 minutes. Then try again.',
    ),
    PairingFailure.notOnLan => (
      'Paired, but not on this network',
      'The feeder is saved on this phone, but it did not show up here. '
          'Check that this phone is on the network you gave the feeder; '
          'Home keeps looking.',
    ),
    PairingFailure.input => (
      'Check the name and Wi-Fi',
      'The phone name is 1 to 16 characters. A Wi-Fi password is 8 to 63 '
          'characters.',
    ),
    PairingFailure.other ||
    null => ('Something went wrong', flow.message ?? 'Try again.'),
  };

  static (String, String) _refusal(SetupRefusal? refusal) => switch (refusal) {
    SetupRefusal.busy => (
      'Another phone is pairing',
      'Wait for it to finish, then try again.',
    ),
    SetupRefusal.unconfirmed => (
      'The code was not keyed in in time',
      'The feeder takes presses for 60 seconds. Try again with a new code.',
    ),
    SetupRefusal.wrongCode => (
      'The code did not match',
      'A press was probably mistyped. Try again with a new code.',
    ),
    SetupRefusal.full => (
      'The feeder has 8 clients already',
      'An owner removes one under People first.',
    ),
    SetupRefusal.notWritable => (
      'The feeder cannot save right now',
      'Try again in a minute.',
    ),
    SetupRefusal.invalid || null => (
      'The feeder rejected the name or Wi-Fi',
      'Check the phone name and the Wi-Fi password.',
    ),
  };

  @override
  Widget build(BuildContext context) {
    final (title, body) = _words();
    final saved = flow.failure == PairingFailure.notOnLan;
    return StepScaffold(
      title: title,
      body: body,
      actions: [
        if (saved)
          FilledButton(
            onPressed: () => context.go('/'),
            child: const Text('Go to Home'),
          )
        else
          FilledButton(onPressed: onRetry, child: const Text('Try again')),
        OutlinedButton(
          onPressed: () => context.go('/'),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}
