import 'dart:async';

import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../platform/install_ports.dart';
import '../src/rust/api/install.dart';
import '../src/rust/api/model.dart';
import '../state/app_state.dart';
import '../state/install.dart';
import '../theme/theme.dart';
import '../widgets/code_tiles.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import 'name_step.dart';
import 'setup_screen.dart';

/// Installing Wellfed on a feeder that runs stock, one screen per stage of
/// its [InstallFlow].
class InstallScreen extends StatefulWidget {
  const InstallScreen({super.key, required this.book, required this.ports});

  final FeederBook book;
  final InstallPorts ports;

  @override
  State<InstallScreen> createState() => _InstallScreenState();
}

class _InstallScreenState extends State<InstallScreen> {
  late final InstallFlow _flow = InstallFlow(widget.book, widget.ports);

  @override
  void initState() {
    super.initState();
    unawaited(_flow.load());
  }

  @override
  void dispose() {
    _flow.dispose();
    super.dispose();
  }

  /// Leaving with a run on stops it, so the owner says so first. The stop
  /// says what it left unsettled.
  Future<void> _leave() async {
    if (!_flow.running) {
      if (mounted) context.pop();
      return;
    }
    final stop = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Stop the install?'),
        content: const Text(
          'This phone stops serving the feeder. The install request is '
          'never sent a second time.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Keep going'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Stop'),
          ),
        ],
      ),
    );
    if (stop == true) _flow.stopRun();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _flow,
    builder: (context, _) => PopScope(
      canPop: !_flow.running,
      onPopInvokedWithResult: (popped, _) {
        if (!popped) unawaited(_leave());
      },
      child: switch (_flow.stage) {
        InstallStage.loading => const Scaffold(
          body: Center(child: CircularProgressIndicator()),
        ),
        InstallStage.before => _Before(flow: _flow),
        InstallStage.naming => NameStep(
          initial: widget.book.label,
          action: 'Continue',
          onBack: () => _flow.back(InstallStage.before),
          onDone: (label, _) => _flow.setLabel(label),
        ),
        InstallStage.wifi => WifiStep(
          initial: null,
          stepLabel: 'Steps 1–2 of 5',
          progress: 0.1,
          mayKeep: false,
          onBack: () => _flow.back(InstallStage.before),
          onDone: _flow.setHome,
        ),
        InstallStage.reset => _Reset(flow: _flow),
        InstallStage.handover => _Handover(flow: _flow, onLeave: _leave),
        InstallStage.confirm => _Confirm(flow: _flow, onLeave: _leave),
        InstallStage.sending => _Sending(flow: _flow),
        InstallStage.claim => _Claim(flow: _flow),
        InstallStage.finishing => _Finishing(flow: _flow),
        InstallStage.done => _Done(flow: _flow),
        InstallStage.stopped => _Stopped(flow: _flow),
      },
    ),
  );
}

/// A row with a check: what is needed, or what is known.
class _Fact extends StatelessWidget {
  const _Fact(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          StrokeIcon(Glyph.check, size: 20, color: c.ok, weight: 2.2),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              text,
              style: TextStyle(fontSize: 15, height: 1.45, color: c.text),
            ),
          ),
        ],
      ),
    );
  }
}

/// 2.1: what it takes.
class _Before extends StatelessWidget {
  const _Before({required this.flow});

  final InstallFlow flow;

  /// Why this phone cannot install: an APK with no image, or one whose
  /// sources cannot be reviewed. Refused here, before the feeder is touched.
  Widget? _refusal() {
    final (String lead, String text)? refusal = switch ((
      flow.image,
      flow.build,
    )) {
      (ImageCheck_Absent(), _) => (
        'This build has no firmware to install.',
        'Install a release of the app, or build it with its image.',
      ),
      (ImageCheck_NotPinned(), _) => (
        'This build pins no firmware.',
        'It holds an image it has no pin for, so it will not install it.',
      ),
      (ImageCheck_Mismatch(:final detail), _) => (
        "This build's firmware doesn't check out.",
        detail,
      ),
      (_, BuildCheck_Unreviewed(:final commit)) => (
        'This build cannot send an install.',
        'Only a build of a clean commit can, so its sources can be '
            'reviewed. This one is $commit, with changes.',
      ),
      _ => null,
    };
    if (refusal == null) return null;
    final (lead, text) = refusal;
    return Notice(tone: Tone.fault, lead: lead, text: text);
  }

  Future<void> _begin(BuildContext context) async {
    final allowed = await flow.begin();
    if (allowed || !context.mounted) return;
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Nearby devices is off'),
        content: const Text(
          'The install claims the feeder over Bluetooth. Allow "Nearby '
          'devices" for Wellfed in Android settings, then try again.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('OK'),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final refusal = _refusal();
    return StepScaffold(
      leading: Leading.close,
      appBarTitle: 'Install Wellfed',
      body:
          "Wellfed replaces the feeder's firmware. This phone keeps a backup "
          'of the original, so you can put it back later.',
      content: [
        const SectionLabel("You'll need"),
        const SizedBox(height: Space.s3),
        const _Fact('The feeder plugged in, no food being dispensed'),
        const _Fact('This phone on your home Wi-Fi, and its password'),
        const _Fact(
          'About 10 minutes with Wellfed open: this phone serves the firmware',
        ),
        const SizedBox(height: Space.s4),
        const Notice(
          tone: Tone.warning,
          lead: "Its meals don't come across.",
          text:
              "Write down any set in the vendor's app, and set them up again "
              'in Wellfed afterwards.',
        ),
        if (refusal != null) ...[const SizedBox(height: Space.s4), refusal],
        if (flow.bluetoothRefused) ...[
          const SizedBox(height: Space.s4),
          const _BluetoothRefused(),
        ],
        if (flow.mayStart && flow.pending == PendingInstall.requestOpen) ...[
          const SizedBox(height: Space.s4),
          const Notice(
            tone: Tone.warning,
            lead: 'An earlier install has not finished.',
            text:
                'Its feeder may be running Wellfed, waiting for its owner, '
                'or still on its original firmware. The install is never sent '
                'to it a second time.',
          ),
        ],
      ],
      actions: [
        if (flow.mayStart && flow.pending == PendingInstall.paired)
          OutlinedButton(
            onPressed: () => unawaited(flow.resume()),
            child: const Text('Pick up where it stopped'),
          ),
        if (flow.mayStart && flow.pending == PendingInstall.requestOpen) ...[
          OutlinedButton(
            onPressed: () => unawaited(flow.listenForOpenRequest()),
            child: const Text('Listen for the feeder'),
          ),
          OutlinedButton(
            onPressed: () => unawaited(flow.resume()),
            child: const Text('Look for it over Bluetooth'),
          ),
        ],
        FilledButton(
          onPressed: flow.mayStart ? () => unawaited(_begin(context)) : null,
          child: const Text('Continue'),
        ),
      ],
    );
  }
}

/// Android refused what a pick-up may need.
class _BluetoothRefused extends StatelessWidget {
  const _BluetoothRefused();

  @override
  Widget build(BuildContext context) => const Notice(
    tone: Tone.fault,
    lead: 'Nearby devices is off.',
    text:
        'The install claims the feeder over Bluetooth. Allow "Nearby devices" '
        'for Wellfed in Android settings, then try again.',
  );
}

/// 2.2: hold Reset. It erases the feeder's settings, so the owner says they
/// understand first.
class _Reset extends StatefulWidget {
  const _Reset({required this.flow});

  final InstallFlow flow;

  @override
  State<_Reset> createState() => _ResetState();
}

class _ResetState extends State<_Reset> {
  bool _understood = false;

  @override
  Widget build(BuildContext context) {
    final flow = widget.flow;
    final offWifi = flow.phoneIp == null;
    return StepScaffold(
      stepLabel: 'Step 1 of 5',
      progress: 0.2,
      leading: Leading.back,
      onLeading: () => flow.back(InstallStage.wifi),
      title: 'Reset the feeder',
      content: [
        const _Fact(
          'If the lock light is on, hold the lock button for 5 seconds.',
        ),
        const _Fact('Hold the rightmost button, Reset, for 5 seconds.'),
        const _Fact('It restarts and opens its own Wi-Fi network for setup.'),
        const SizedBox(height: Space.s4),
        const Notice(
          tone: Tone.warning,
          lead: 'Resetting erases its meals and Wi-Fi.',
          text:
              "Everything saved by the vendor's app goes. Write the meals down "
              'first if you want them back.',
        ),
        const SizedBox(height: Space.s3),
        CheckboxListTile(
          contentPadding: EdgeInsets.zero,
          controlAffinity: ListTileControlAffinity.leading,
          value: _understood,
          onChanged: (value) => setState(() => _understood = value ?? false),
          title: const Text('I understand its meals will be erased'),
        ),
        if (offWifi) ...[
          const SizedBox(height: Space.s3),
          const Notice(
            tone: Tone.fault,
            lead: "This phone isn't on Wi-Fi.",
            text: 'Join your home Wi-Fi, then press the button again.',
          ),
        ],
      ],
      actions: [
        FilledButton(
          onPressed: _understood ? () => unawaited(flow.startHandover()) : null,
          child: const Text('I held Reset'),
        ),
      ],
    );
  }
}

/// 2.3: this phone joins the feeder's network, sends the Wi-Fi once and comes
/// back, then waits for the feeder to reach it.
class _Handover extends StatelessWidget {
  const _Handover({required this.flow, required this.onLeave});

  final InstallFlow flow;
  final Future<void> Function() onLeave;

  @override
  Widget build(BuildContext context) {
    final stateText = switch (flow.handover) {
      Handover.starting => 'Getting ready…',
      Handover.join || Handover.joining => null,
      Handover.sending => 'Sending your Wi-Fi to the feeder…',
      Handover.waiting => 'Waiting for the feeder to join your Wi-Fi…',
    };
    return StepScaffold(
      stepLabel: 'Step 2 of 5',
      progress: 0.35,
      leading: Leading.close,
      onLeading: () => unawaited(onLeave()),
      title: 'Tell the feeder how to reach this phone',
      body:
          "This phone joins the feeder's setup network for a few seconds, "
          'sends your Wi-Fi once, then comes back to your network.',
      content: [
        const Notice(
          tone: Tone.neutral,
          lead: 'Your password goes unencrypted.',
          text:
              "The feeder's setup network has no password, so someone close "
              'by could read it during those few seconds. The feeder also '
              'needs a 2.4 GHz Wi-Fi.',
        ),
        if (flow.joinFailed) ...[
          const SizedBox(height: Space.s3),
          const Notice(
            tone: Tone.warning,
            lead: "This phone didn't join the feeder's network.",
            text:
                'Its name starts with PL-AF103. It can take a minute to appear '
                "after the reset. If this phone set it up in the vendor's app, "
                "forget that network in the phone's Wi-Fi settings, then try "
                'again. If it never appears, hold Reset again.',
          ),
        ],
        if (flow.restartAgain || flow.waitReason != null) ...[
          const SizedBox(height: Space.s4),
          _StillWaiting(flow: flow),
        ] else if (stateText != null) ...[
          const SizedBox(height: Space.s4),
          Searching(stateText),
        ],
      ],
      actions: [
        if (flow.handover == Handover.join || flow.handover == Handover.joining)
          FilledButton(
            onPressed: flow.handover == Handover.join
                ? () => unawaited(flow.joinFeeder())
                : null,
            child: const Text("Join the feeder's network"),
          ),
      ],
    );
  }
}

/// Why stock has not moved on, and the restart its lost reports need.
class _StillWaiting extends StatelessWidget {
  const _StillWaiting({required this.flow});

  final InstallFlow flow;

  @override
  Widget build(BuildContext context) {
    final text = switch (flow.waitReason) {
      WaitReason.notOnAdapter =>
        'A feeder connected, but it is on batteries. Plug it in.',
      WaitReason.noVersion || WaitReason.offline => 'A feeder is connecting…',
      _ => 'Waiting for the feeder to connect…',
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (flow.restartAgain) ...[
          const Notice(
            tone: Tone.warning,
            lead: 'Restart the feeder once more:',
            text:
                'unplug it for 10 seconds (take the batteries out too, if '
                'fitted) and plug it back in.',
          ),
          const SizedBox(height: Space.s4),
        ],
        Searching(text),
      ],
    );
  }
}

/// 2.4: trust on first use. The owner compares what connected with the
/// label and the router's list before anything is sent to it.
class _Confirm extends StatelessWidget {
  const _Confirm({required this.flow, required this.onLeave});

  final InstallFlow flow;
  final Future<void> Function() onLeave;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final mono = TextStyle(fontFamily: Fonts.mono, fontSize: 15, color: c.text);
    return StepScaffold(
      stepLabel: 'Step 3 of 5',
      progress: 0.5,
      leading: Leading.close,
      onLeading: () => unawaited(onLeave()),
      title: 'A feeder connected. Is it yours?',
      body:
          'Compare the device ID with the label under the feeder, and the '
          "address with your router's list of devices.",
      content: [
        FactList([
          ('Device ID', Text(flow.device ?? '', style: mono)),
          ('Address', Text(flow.deviceAddress ?? '', style: mono)),
        ]),
        const SizedBox(height: Space.s4),
        const _Fact('PLAF103'),
        const _Fact('Vendor firmware 1.1.15'),
        const _Fact('On its mains adapter'),
      ],
      actions: [
        FilledButton(
          onPressed: () => flow.recognize(yes: true),
          child: const Text('Yes, install Wellfed'),
        ),
        TextButton(
          onPressed: () => flow.recognize(yes: false),
          child: const Text("That's not my feeder"),
        ),
      ],
    );
  }
}

/// A screen with the feeder on it and no way back: the run goes on by
/// itself, and the notification speaks for it when the phone is elsewhere.
class _FeederPage extends StatelessWidget {
  const _FeederPage({required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => Scaffold(
    body: SafeArea(
      child: ListView(
        padding: const EdgeInsets.fromLTRB(28, 56, 28, 28),
        children: [
          Center(child: const FeederImage(size: 180)),
          const SizedBox(height: 28),
          Headline(title, size: 24, align: TextAlign.center),
          const SizedBox(height: 28),
          ...children,
        ],
      ),
    ),
  );
}

/// 2.5: stock downloads the replacement from this phone.
class _Sending extends StatelessWidget {
  const _Sending({required this.flow});

  final InstallFlow flow;

  String _status() {
    if (flow.restarting) return 'Restarting into Wellfed';
    final left = flow.timeLeft;
    if (left == null) return '${flow.percent}%';
    return '${flow.percent}% · about ${left.inSeconds + 1} s left';
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return _FeederPage(
      title: 'Installing Wellfed',
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                flow.restarting
                    ? 'The feeder is restarting'
                    : 'Sending firmware',
                style: TextStyle(
                  fontSize: 15,
                  fontWeight: Fonts.semibold,
                  color: c.text,
                ),
              ),
            ),
            Flexible(
              child: Text(
                _status(),
                style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
              ),
            ),
          ],
        ),
        const SizedBox(height: Space.s2),
        Bar(value: flow.restarting ? 1 : flow.percent / 100),
        const SizedBox(height: Space.s5),
        if (flow.holdRefused) ...[
          const Notice(
            tone: Tone.fault,
            lead: "Android won't keep Wellfed running in the background.",
            text: 'Keep it on the screen until this finishes.',
          ),
          const SizedBox(height: Space.s3),
        ],
        const Notice(
          tone: Tone.warning,
          lead: 'Keep Wellfed open.',
          text:
              'This phone is serving the firmware, and closing the app stops '
              "it. Don't unplug the feeder.",
        ),
        WaitWords(flow.waitWords, progress: flow.percent),
      ],
    );
  }
}

/// 2.6: this phone becomes the replacement's owner. A consumer image waits
/// on Bluetooth by itself; a bench image needs its recovery window opened.
/// Either way the code is the feeder's proof, as in setup.
class _Claim extends StatelessWidget {
  const _Claim({required this.flow});

  final InstallFlow flow;

  @override
  Widget build(BuildContext context) => StepScaffold(
    stepLabel: 'Step 4 of 5',
    progress: 0.75,
    leading: Leading.none,
    title: 'Wellfed is running. Now claim it.',
    content: [
      if (flow.windowAsked)
        const NumberedSteps([
          'Hold the **Lock** button, then also the **Dispense** button, for '
              '5 seconds. The lock light blinks fast.',
        ])
      else if (flow.code.isEmpty) ...[
        if (flow.settingUpAgain) ...[
          const NumberedSteps([
            'If the lock light is on, hold **Lock** for 5 seconds to unlock '
                'the panel.',
            'Hold **Lock** and **Mic** together for 5 seconds.',
            'The Wi-Fi light blinks slowly, about once a second. Setup mode '
                'lasts 15 minutes.',
          ]),
          const SizedBox(height: Space.s4),
        ],
        if (flow.setupHint && !flow.settingUpAgain) ...[
          const Notice(
            tone: Tone.warning,
            lead: 'Hold Lock + Mic for 5 s on the feeder.',
            text:
                'If the lock light is on, hold Lock for 5 seconds to unlock '
                'the panel first.',
          ),
          const SizedBox(height: Space.s4),
        ],
        Searching(
          flow.searchingBluetooth || flow.settingUpAgain
              ? 'Looking for the feeder over Bluetooth…'
              : 'Looking for the feeder on your Wi-Fi…',
        ),
        WaitWords(flow.waitWords),
      ] else ...[
        Text(
          'Press these buttons once each, in order:',
          style: bodyStyle(context),
        ),
        const SizedBox(height: Space.s3),
        CodeTiles(flow.code),
        const SizedBox(height: 18),
        const RingLegend(),
        const SizedBox(height: 18),
        CodeWindow(deadline: flow.deadline),
        const SizedBox(height: Space.s4),
        const Searching('Waiting for the feeder to confirm…'),
      ],
      if (flow.pressDispense) ...[
        const SizedBox(height: Space.s4),
        const Notice(
          tone: Tone.warning,
          lead: 'Press Dispense once.',
          text: 'The feeder takes the install\'s update key with that press.',
        ),
      ],
    ],
    actions: [
      if (flow.windowAsked)
        FilledButton(
          onPressed: flow.windowOpened,
          child: const Text('The lock light is blinking fast'),
        ),
    ],
  );
}

/// One line of the checklist.
class _Step extends StatelessWidget {
  const _Step(this.text, this.state, {this.fraction});

  final String text;
  final CheckState state;

  /// How far a running step is, when it knows.
  final double? fraction;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final running = state == CheckState.running;
    final percent = fraction == null ? null : (fraction! * 100).round();
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s3),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              SizedBox.square(
                dimension: 22,
                child: switch (state) {
                  CheckState.done => StrokeIcon(
                    Glyph.check,
                    size: 20,
                    color: c.ok,
                    weight: 2.2,
                  ),
                  CheckState.running => CircularProgressIndicator(
                    strokeWidth: 2.5,
                    color: c.accent,
                    backgroundColor: c.sunken,
                  ),
                  CheckState.waiting => Center(
                    child: Container(
                      width: 8,
                      height: 8,
                      decoration: BoxDecoration(
                        color: c.lineStrong,
                        shape: BoxShape.circle,
                      ),
                    ),
                  ),
                },
              ),
              const SizedBox(width: Space.s3),
              Expanded(
                child: Text(
                  text,
                  style: TextStyle(
                    fontSize: 15,
                    fontWeight: running ? Fonts.semibold : null,
                    color: state == CheckState.waiting ? c.textMuted : c.text,
                  ),
                ),
              ),
              if (running && percent != null)
                Text(
                  '$percent%',
                  style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
                ),
            ],
          ),
          if (running && fraction != null) ...[
            const SizedBox(height: Space.s2),
            Bar(value: fraction!),
          ],
        ],
      ),
    );
  }
}

/// 2.7: the backup, the promotion and the confirmation, one line each.
class _Finishing extends StatelessWidget {
  const _Finishing({required this.flow});

  final InstallFlow flow;

  @override
  Widget build(BuildContext context) {
    final title = switch (flow.promoting) {
      CheckState.waiting => 'Backing up the original firmware',
      _ => 'Making Wellfed permanent',
    };
    return StepScaffold(
      stepLabel: 'Step 5 of 5',
      progress: 1,
      leading: Leading.none,
      title: title,
      content: [
        const _Step('This phone is its owner', CheckState.done),
        if (flow.joining != CheckState.waiting)
          _Step('Finding it on your network', flow.joining),
        _Step(
          'Copying the original firmware',
          flow.copying,
          fraction: flow.copied,
        ),
        _Step('Check the copy matches', flow.checked),
        _Step(
          'Make Wellfed permanent · about 4 min',
          flow.promoting,
          fraction: flow.written,
        ),
        _Step('Restart and confirm', flow.confirming),
        const SizedBox(height: Space.s2),
        Text(
          'Wellfed only becomes permanent once the backup checks out. Until '
          'then, the original firmware is still on the feeder.',
          style: bodyStyle(context),
        ),
        const SizedBox(height: Space.s4),
        const Notice(
          tone: Tone.neutral,
          lead: 'Keep the feeder plugged in.',
          text: 'If this phone loses it, Wellfed picks up where it stopped.',
        ),
        WaitWords(
          flow.waitWords,
          progress: (
            flow.joining,
            flow.copied,
            flow.copying,
            flow.checked,
            flow.written,
            flow.promoting,
            flow.confirming,
          ),
        ),
      ],
    );
  }
}

/// 2.8: done. Names the feeder.
class _Done extends StatefulWidget {
  const _Done({required this.flow});

  final InstallFlow flow;

  @override
  State<_Done> createState() => _DoneState();
}

class _DoneState extends State<_Done> {
  final _name = TextEditingController(text: InstallFlow.defaultFeederName);
  String? _shareFailed;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _go() async {
    final name = _name.text.trim();
    await widget.flow.finish(
      name.isEmpty ? InstallFlow.defaultFeederName : name,
    );
    if (mounted) context.go('/');
  }

  Future<void> _save() async {
    try {
      await widget.flow.saveBackup();
    } catch (error) {
      if (mounted) setState(() => _shareFailed = '$error');
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final flow = widget.flow;
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(28, Space.s7, 28, 28),
          children: [
            Center(
              child: Stack(
                children: [
                  const FeederImage(size: 250),
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
            const Headline('Your feeder runs Wellfed.', size: 32),
            const SizedBox(height: Space.s2),
            Text('Set up its meals here next.', style: bodyStyle(context)),
            const SizedBox(height: Space.s5),
            LabeledField(
              label: 'Name it',
              child: TextField(
                controller: _name,
                textCapitalization: TextCapitalization.words,
              ),
            ),
            const SizedBox(height: Space.s4),
            const SizedBox(height: Space.s5),
            FilledButton(
              onPressed: () => unawaited(_go()),
              child: const Text('Go to the feeder'),
            ),
            if (flow.backupPath != null) ...[
              const SizedBox(height: Space.s2),
              TextButton(
                onPressed: () => unawaited(_save()),
                child: const Text('Save the backup somewhere else'),
              ),
            ],
            if (_shareFailed != null)
              Notice(
                tone: Tone.fault,
                lead: "The backup wasn't saved elsewhere.",
                text: _shareFailed!,
              ),
          ],
        ),
      ),
    );
  }
}

/// 2.9: why it stopped, what the feeder is left with, and what next. An
/// uncertain install is listened for and never sent again.
class _Stopped extends StatelessWidget {
  const _Stopped({required this.flow});

  final InstallFlow flow;

  /// The design's other ways an install stops, for the owner who meets one
  /// later. The one on the screen is not listed again.
  static const _others = [
    (
      'Different vendor firmware',
      'Wellfed installs over 1.1.15 only, for now. Nothing on the feeder '
          'changed.',
    ),
    (
      'Two feeders answered',
      "We can't tell which one is yours. Unplug the others and start again.",
    ),
    (
      "The backup didn't match",
      "Wellfed won't make itself permanent without a good backup. The feeder "
          'keeps its original firmware.',
    ),
  ];

  (String, String) _words(StopCase stop) => switch (stop) {
    StopCase_Uncertain() => switch (flow.listening) {
      Listening.stockStayed when flow.stop?.resume == Resume.fromStart => (
        "It didn't install",
        'The feeder came back on its original firmware, and nothing changed '
            'on it. You can start again.',
      ),
      Listening.stockStayed => (
        'A new install has to wait',
        'The feeder is on its original firmware, but it has not stayed connected '
            'long enough to rule out the earlier install. A new one isn\'t '
            'possible until it has stayed connected to this phone for 10 minutes '
            'without a break. Leave it plugged in, keep Wellfed open and listen '
            'again.',
      ),
      Listening.silent => (
        'Still no word from the feeder',
        'It may have installed, or it may be off the network. Nothing was '
            'sent a second time. If it shows no Wi-Fi light, look for it '
            'over Bluetooth.',
      ),
      Listening.failed => (
        "We can't listen for the feeder",
        flow.listenerDetail ?? 'Something on this phone stopped it.',
      ),
      _ when flow.listeningForEarlier => (
        'Waiting to hear from the feeder',
        'This phone never heard how an earlier install ended, even if you have '
            'put the original firmware back since. Leave the feeder plugged in. '
            'That install closes 20 minutes after it went out, once the feeder '
            'has reported its version and been quiet for 10. Nothing is sent a '
            'second time.',
      ),
      _ => (
        "We don't know yet if it installed",
        'The feeder stopped answering partway through. That can be normal '
            'while it restarts. Leave it plugged in. Wellfed listens for up '
            'to 30 minutes and never sends the install a second time.',
      ),
    },
    StopCase_UncertainWrite(:final write) => (
      "We don't know if that last step ran",
      switch (write) {
        UncertainWrite.updateKey =>
          "The feeder didn't answer about the update key, and it is never "
              'sent a second time.',
        UncertainWrite.promotion =>
          'The feeder stopped answering while Wellfed became permanent. Leave '
              'it plugged in: picking up reads where it got to.',
      },
    ),
    StopCase_OtherFirmware(:final version) => (
      'Different vendor firmware',
      'This feeder runs $version. Wellfed installs over 1.1.15 only, for now. '
          'Nothing on the feeder changed.',
    ),
    StopCase_TwoFeeders() => (
      'Two feeders answered',
      "We can't tell which one is yours. Unplug the others and start again.",
    ),
    StopCase_BackupMismatch() => (
      "The backup didn't match",
      "Wellfed won't make itself permanent without a good backup. The feeder "
          'keeps its original firmware.',
    ),
    StopCase_NotYourFeeder() => (
      "That wasn't your feeder",
      'Nothing was sent to it. Check what else on your network could use that '
          'name, then start again.',
    ),
    StopCase_NoFeederCame() => (
      'No feeder connected',
      'Nothing was sent. Check that it is plugged in, then start again.',
    ),
    StopCase_HandoverFailed() => (
      "This phone couldn't finish the handover",
      "Nothing was installed. Check that the feeder's setup network is up and "
          'that this phone is on your Wi-Fi, then start again. If its setup '
          'network is gone, hold Reset again first.',
    ),
    StopCase_NotConfirmed() => (
      "Wellfed hasn't confirmed yet",
      'It is installed and may still be checking itself. Stay near it and pick '
          'up where it stopped to look again.',
    ),
    StopCase_FrameNotTaken() => (
      "The feeder didn't reach this phone",
      'It should keep nothing and open its setup network again. '
          'Check the Wi-Fi password, that your Wi-Fi has a 2.4 GHz band, and '
          'that this phone stayed on it.',
    ),
    StopCase_EarlierRequestOpen() => (
      'An earlier install is still open',
      'This phone never heard how it ended, even if you have put the original '
          'firmware back since, so Wellfed will not ask again yet. Listen with '
          'the feeder plugged in: 20 minutes after that install, once it has '
          'reported its version and been quiet for 10, you can start again.',
    ),
    StopCase_RequestRefused(:final why) => switch (why) {
      RequestRefusal.stale => (
        "The feeder's reports got too old",
        'Wellfed only sends the install right after the feeder reports in. '
            'Nothing was sent. Restart the feeder and start again.',
      ),
      RequestRefusal.offline => (
        'The feeder went offline',
        'It left this phone before the install went out, and nothing was '
            'sent. Check that it is plugged in, then start again.',
      ),
      RequestRefusal.imageServer => (
        "This phone wasn't ready to serve the firmware",
        'Nothing was sent. Start again.',
      ),
      RequestRefusal.other => (
        "The install wasn't sent",
        'Nothing reached the feeder. See below for what this phone said.',
      ),
    },
    StopCase_StockRefused(:final code) => (
      'The feeder refused the install',
      code == null
          ? 'It keeps running its original firmware.'
          : 'It keeps running its original firmware (error $code).',
    ),
    StopCase_ClockUnset() => (
      "The feeder's clock isn't set",
      'It refused before downloading, and nothing was sent. Restart the '
          'feeder and start again.',
    ),
    StopCase_NoSetupFeeder() => (
      'No feeder is waiting over Bluetooth',
      'It may still be starting. Keep this phone near it and try again.',
    ),
    StopCase_SetupFailed(:final refusal) => (
      "This phone couldn't claim it",
      switch (refusal) {
        SetupRefusal.wrongCode =>
          'The feeder heard a different order of presses, or another device '
              'is in the way. Try again with a new code.',
        SetupRefusal.unconfirmed =>
          'The code works for 60 seconds, and the time ran out. Try again with '
              'a new code.',
        SetupRefusal.busy =>
          'Another phone is setting it up. Wait, then try again.',
        SetupRefusal.full =>
          'It holds 8 phones and servers. An owner removes one first.',
        SetupRefusal.notWritable =>
          "It can't save right now. Try again in a minute.",
        SetupRefusal.invalid => 'It rejected the phone name or the Wi-Fi. Check them and start again.',
        null =>
          'The Bluetooth exchange broke off. Move closer and try again. It '
              'runs Wellfed and still holds its original firmware.',
      },
    ),
    StopCase_OtherFeeder() => (
      'That was a different feeder',
      'The code was keyed in on another feeder than the one this install is '
          'for. That one now lists this phone as its owner and has your Wi-Fi, '
          "and nothing else was done to it. Put this install's feeder in setup "
          'mode, then pick up and key in the code on its buttons.',
    ),
    StopCase_NotAnswering() => (
      'The feeder stopped answering',
      'It paired with this phone, but does not answer on this network. Check '
          'that this phone is on the network you gave it.',
    ),
    StopCase_NotJoined() => (
      "It didn't join your Wi-Fi",
      'It runs Wellfed and this phone is its owner, but it never appeared on '
          'your network. The Wi-Fi password may be wrong, or this phone may be '
          'on another network. Give it the Wi-Fi again: you put the feeder back '
          'in setup mode with its buttons, and this phone shows how.',
    ),
    StopCase_PhoneOffWifi() => (
      "This phone isn't on Wi-Fi",
      'Nothing was sent. Join your home network, then try again.',
    ),
    StopCase_WifiRefused() => (
      "That Wi-Fi can't be given to a feeder",
      'Nothing was sent. Check its name and password, and enter them again.',
    ),
    StopCase_NoWifiGiven() => (
      'This install needs your Wi-Fi',
      'Nothing was sent. The feeder has no other way to join your network. '
          'Enter its name and password.',
    ),
    StopCase_PromotionFailed() => (
      "Wellfed didn't become permanent",
      'The feeder runs Wellfed for now and still holds its original firmware.',
    ),
    StopCase_FellBack() => (
      'It went back after restarting',
      "The new copy didn't confirm, so the feeder returned to the one it was "
          'running.',
    ),
    StopCase_ServerFailed() => (
      "This phone couldn't serve the feeder",
      'Another app may hold the port it needs. Nothing was sent.',
    ),
    StopCase_Cancelled() => (
      'You stopped the install',
      'Nothing is waiting on it.',
    ),
    StopCase_Other() => ('The install stopped', 'See below for what it said.'),
  };

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final stop = flow.stop;
    if (stop == null) return const SizedBox.shrink();
    final uncertain = stop.case_ is StopCase_Uncertain;
    final (title, body) = _words(stop.case_);
    final listening = uncertain && flow.listening == Listening.running;
    final minutes = flow.listened.inMinutes;
    final others = [
      for (final other in _others)
        if (other.$1 != title) other,
    ];

    return StepScaffold(
      leading: Leading.close,
      onLeading: () => context.go('/'),
      top: Align(
        alignment: Alignment.centerLeft,
        child: StrokeIcon(
          uncertain ? Glyph.clock : Glyph.failed,
          size: 40,
          color: uncertain ? c.warning : c.fault,
          weight: 2,
        ),
      ),
      title: title,
      body: body,
      content: [
        if (listening) ...[
          Searching(
            'Listening for the feeder · $minutes of '
            '${flow.listenWait.inMinutes} min',
          ),
          const SizedBox(height: Space.s2),
          Bar(
            value: flow.listenWait.inSeconds == 0
                ? 0
                : flow.listened.inSeconds / flow.listenWait.inSeconds,
          ),
          if (flow.stockSeen) ...[
            const SizedBox(height: Space.s3),
            const Notice(
              tone: Tone.neutral,
              lead: 'The feeder came back on its original firmware.',
              text: 'Wellfed is making sure before it asks again.',
            ),
          ],
          const SizedBox(height: Space.s5),
        ],
        if (flow.bluetoothRefused) ...[
          const _BluetoothRefused(),
          const SizedBox(height: Space.s4),
        ],
        ..._nextSteps(context, stop),
        if (stop.detail.isNotEmpty && !listening) ...[
          const SizedBox(height: Space.s4),
          Text(
            stop.detail,
            style: TextStyle(
              fontFamily: Fonts.mono,
              fontSize: Sizes.xs,
              height: 1.4,
              color: c.textMuted,
            ),
          ),
        ],
        const SizedBox(height: Space.s6),
        const SectionLabel('Other ways an install stops'),
        const SizedBox(height: Space.s3),
        ReasonList(others),
      ],
    );
  }

  /// What the owner can do from here. Nothing here sends a write the run
  /// marked uncertain: a pick-up reads where the feeder got to, and a new
  /// request goes only after the ledger closed the old one.
  List<Widget> _nextSteps(BuildContext context, InstallEnd_Stopped stop) {
    Widget primary(String label, VoidCallback onPressed) =>
        FilledButton(onPressed: onPressed, child: Text(label));
    Widget secondary(String label, VoidCallback onPressed) =>
        OutlinedButton(onPressed: onPressed, child: Text(label));
    Widget home() => TextButton(
      onPressed: () => context.go('/'),
      child: const Text('Go to Home'),
    );

    return switch (stop.case_) {
      StopCase_Uncertain() => switch (flow.listening) {
        Listening.running => const [],
        Listening.stockStayed when stop.resume == Resume.fromStart => [
          primary('Start again', () => unawaited(flow.again())),
        ],
        Listening.stockStayed => [
          primary('Listen again', () => unawaited(flow.listen())),
          const SizedBox(height: Space.s2),
          home(),
        ],
        _ => [
          primary('Listen again', () => unawaited(flow.listen())),
          const SizedBox(height: Space.s2),
          secondary(
            'Look for it over Bluetooth',
            () => unawaited(flow.resume()),
          ),
          home(),
        ],
      },
      StopCase_EarlierRequestOpen() => [
        primary('Listen for it', () => unawaited(flow.listenForOpenRequest())),
      ],
      StopCase_ClockUnset() => [
        primary('Start again', () => unawaited(flow.again())),
      ],
      StopCase_TwoFeeders() || StopCase_NotYourFeeder() => [
        primary('Start again', () => unawaited(flow.again(forget: true))),
      ],
      StopCase_NotJoined() => [
        primary('Enter the Wi-Fi again', flow.newWifiForSetup),
        const SizedBox(height: Space.s2),
        secondary('Keep waiting for it', () => unawaited(flow.keepWaiting())),
      ],
      StopCase_FrameNotTaken() => [
        primary('Enter the Wi-Fi again', flow.changeWifi),
        secondary('Try again', () => unawaited(flow.again())),
      ],
      StopCase_WifiRefused() || StopCase_NoWifiGiven() => [
        primary('Enter the Wi-Fi again', flow.changeWifi),
      ],
      _ => switch (stop.resume) {
        Resume.fromStart => [
          primary('Start again', () => unawaited(flow.again())),
        ],
        Resume.fromBank0 => [
          primary('Pick up where it stopped', () => unawaited(flow.again())),
        ],
        Resume.nothing => [home()],
      },
    };
  }
}
