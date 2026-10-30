import 'package:flutter/material.dart';

import '../platform/restore_ports.dart';
import '../src/rust/api/restore.dart';
import '../src/rust/api/store.dart';
import '../state/app_state.dart';
import '../state/install.dart' show CheckState;
import '../state/restore.dart';
import '../theme/theme.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';

/// Where Home reaches a restore: one line under a feeder's card.
class RestoreEntry extends StatelessWidget {
  const RestoreEntry({super.key, required this.app, required this.feeder});

  final AppState app;
  final KnownFeeder feeder;

  @override
  Widget build(BuildContext context) => Align(
    alignment: Alignment.centerLeft,
    child: TextButton(
      onPressed: () => Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) =>
              RestoreScreen(feeder: feeder, ports: AppRestorePorts(app.key)),
        ),
      ),
      child: const Text('Restore original firmware'),
    ),
  );
}

/// 12.4: putting the vendor's firmware back from the backup the install kept.
class RestoreScreen extends StatefulWidget {
  const RestoreScreen({super.key, required this.feeder, required this.ports});

  final KnownFeeder feeder;
  final RestorePorts ports;

  @override
  State<RestoreScreen> createState() => _RestoreScreenState();
}

class _RestoreScreenState extends State<RestoreScreen> {
  late final RestoreFlow _flow = RestoreFlow(
    widget.ports,
    identity: identityBytes(hex: widget.feeder.identity),
    feederName: widget.feeder.name,
    address: widget.feeder.address,
  );

  @override
  void initState() {
    super.initState();
    _flow.load();
  }

  @override
  void dispose() {
    _flow.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => RestoreView(flow: _flow);
}

/// The flow's screens, whichever stage it is at.
class RestoreView extends StatelessWidget {
  const RestoreView({super.key, required this.flow});

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: flow,
    builder: (context, _) => PopScope(
      canPop: !flow.committed,
      child: switch (flow.stage) {
        RestoreStage.loading => const Scaffold(body: SizedBox.shrink()),
        RestoreStage.noBackup => _NoBackup(flow),
        RestoreStage.before => _Before(flow),
        RestoreStage.running => _Running(flow),
        RestoreStage.done => _Done(flow),
        RestoreStage.uncertain => _Uncertain(flow),
        RestoreStage.stopped => _Stopped(flow),
      },
    ),
  );
}

const _title = 'Restore original firmware';

class _NoBackup extends StatelessWidget {
  const _NoBackup(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) => StepScaffold(
    appBarTitle: _title,
    title: 'No backup on this phone',
    body:
        "The copy of the vendor firmware made during install isn't on this "
        'phone. If you saved a copy when the install ended, pick that file.',
    content: [
      if (flow.notStock != null)
        Notice(
          tone: Tone.fault,
          lead: "That isn't the original firmware.",
          text:
              "It doesn't match the vendor's 1.1.15 exactly, so it can't go "
              'back on the feeder. It may be damaged or partial.',
        ),
    ],
    actions: [
      FilledButton(
        onPressed: flow.pickFile,
        child: const Text('Pick the file'),
      ),
    ],
  );
}

class _Before extends StatelessWidget {
  const _Before(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final name = flow.feederName;
    return StepScaffold(
      appBarTitle: _title,
      body:
          'Puts back the vendor firmware that was on $name before Wellfed, '
          'from the backup made during install.',
      content: [
        FactList([
          ('Backup', Text(_backupWords(flow))),
          ('Version', Text('Vendor ${flow.version ?? ''}')),
          (
            'Checked',
            Text('Matches the original', style: TextStyle(color: c.ok)),
          ),
        ]),
        const SizedBox(height: Space.s4),
        _Warning(
          "Meals and people set up in Wellfed don't carry over. Set the "
          "feeder up again in the vendor's app.",
        ),
        const SizedBox(height: Space.s3),
        _Warning(
          "After this, Wellfed can't talk to $name. If it doesn't come back "
          "on the vendor firmware, this app can't help from here.",
        ),
      ],
      actions: [
        CheckboxListTile(
          value: flow.understood,
          onChanged: (value) => flow.setUnderstood(value ?? false),
          contentPadding: EdgeInsets.zero,
          controlAffinity: ListTileControlAffinity.leading,
          title: const Text('I understand'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(
            backgroundColor: c.fault,
            foregroundColor: c.background,
          ),
          onPressed: flow.understood ? flow.start : null,
          child: const Text(_title),
        ),
      ],
    );
  }

  static String _backupWords(RestoreFlow flow) {
    if (flow.picked) return 'From a picked file';
    final at = flow.backupDate;
    if (at == null) return 'On this phone';
    const months = [
      'Jan',
      'Feb',
      'Mar',
      'Apr',
      'May',
      'Jun',
      'Jul',
      'Aug',
      'Sep',
      'Oct',
      'Nov',
      'Dec',
    ];
    return '${at.day} ${months[at.month - 1]}, on this phone';
  }
}

class _Warning extends StatelessWidget {
  const _Warning(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        StrokeIcon(Glyph.warning, size: 18, color: c.warning),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            text,
            style: TextStyle(fontSize: Sizes.base, height: 1.5, color: c.text),
          ),
        ),
      ],
    );
  }
}

/// 12.2's steps, for the way back.
class _Running extends StatelessWidget {
  const _Running(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) => StepScaffold(
    appBarTitle: 'Restoring ${flow.feederName}',
    leading: flow.committed ? Leading.none : Leading.back,
    content: [
      if (flow.pressDispense) ...[
        const Notice(
          tone: Tone.warning,
          lead: 'Press Dispense on the feeder, once.',
          text: 'You have 60 seconds. The press shows you have the feeder in hand.',
        ),
        const SizedBox(height: Space.s4),
      ],
      Panel(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 2),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Line('Press Dispense on the feeder', flow.pressing),
              _Line('Sending', flow.sending, fraction: flow.written),
              _Line('Restart on the original firmware', flow.restarting),
            ],
          ),
        ),
      ),
      const SizedBox(height: Space.s4),
      Text(
        flow.sessionDropped
            ? 'The connection dropped. Trying to get it back. Keep the feeder '
                  'plugged in.'
            : 'Keep the feeder plugged in, and this phone on the same Wi-Fi. '
                  "Once this has started, there's no stopping it from here.",
        style: bodyStyle(context),
      ),
      if (flow.holdRefused) ...[
        const SizedBox(height: Space.s4),
        const Notice(
          tone: Tone.warning,
          text: 'Keep this screen open: Android would not let this run behind.',
        ),
      ],
    ],
  );
}

/// One line of the steps.
class _Line extends StatelessWidget {
  const _Line(this.text, this.state, {this.fraction});

  final String text;
  final CheckState state;

  /// How far a running step is, when it knows.
  final double? fraction;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final running = state == CheckState.running;
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
              if (running && fraction != null)
                Text(
                  '${(fraction! * 100).round()}%',
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

class _Done extends StatelessWidget {
  const _Done(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) => StepScaffold(
    leading: Leading.none,
    title: 'The original firmware is going back',
    body:
        '${flow.feederName} is restarting on vendor ${flow.version ?? ''}. '
        "Wellfed can't talk to it any more. What it does next depends on how "
        'you installed Wellfed.',
    content: const [
      ReasonList([
        (
          'If you installed by holding Reset',
          'It still looks for this phone, and keeps trying forever. Hold '
              "Reset for about 5 seconds, then set it up in the vendor's app.",
        ),
        (
          'If you installed through your router',
          "It goes back to the vendor's cloud once the DNS record is gone. "
              'Remove the record from your router.',
        ),
      ]),
    ],
    actions: [_Close()],
  );
}

class _Uncertain extends StatelessWidget {
  const _Uncertain(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) => StepScaffold(
    leading: Leading.none,
    title: "We can't tell if it went through",
    body: switch (flow.uncertain) {
      UncertainWhy.stopped =>
        'This was stopped as the firmware was starting to go across, or after '
            'it had. ${flow.feederName} may already be on the vendor firmware, '
            'or still on Wellfed. Nothing more is sent.',
      UncertainWhy.failed =>
        'This phone hit a problem as the firmware was starting to go across, '
            'or after it had. ${flow.feederName} may already be on the vendor '
            'firmware, or still on Wellfed. Nothing more is sent.',
      _ =>
        '${flow.feederName} stopped answering after the firmware started '
            'going across. It may already be on the vendor firmware, or still '
            'on Wellfed. Nothing more is sent.',
    },
    content: [
      const Notice(
        tone: Tone.warning,
        lead: 'Keep it plugged in, and give it a few minutes.',
        text:
            'If it shows up as usual in Wellfed, it kept Wellfed. If it never '
            "does, it's likely on the vendor firmware: hold Reset for about 5 "
            "seconds and set it up in the vendor's app.",
      ),
      if (flow.detail case final detail?) ...[
        const SizedBox(height: Space.s3),
        _Small(detail),
      ],
    ],
    actions: [_Close()],
  );
}

class _Stopped extends StatelessWidget {
  const _Stopped(this.flow);

  final RestoreFlow flow;

  @override
  Widget build(BuildContext context) {
    final (title, body) = _words(flow.stop, flow.feederName);
    return StepScaffold(
      leading: Leading.none,
      title: title,
      body: body,
      content: [if (flow.detail case final detail?) _Small(detail)],
      actions: [
        if (flow.mayRetry)
          FilledButton(onPressed: flow.again, child: const Text('Try again')),
        _Close(outlined: flow.mayRetry),
      ],
    );
  }

  static (String, String) _words(RestoreStop? stop, String name) =>
      switch (stop) {
        RestoreStop.noPress => (
          'Dispense was not pressed in time',
          '$name keeps running Wellfed, and nothing was sent.',
        ),
        RestoreStop.enrollmentUnknown => (
          "We don't know if the press got through",
          '$name stopped answering just then. It keeps running Wellfed, and '
              'trying again finds out first, without asking for a second '
              'press if it took.',
        ),
        RestoreStop.keyRefused => (
          "$name wouldn't take the press",
          'It may be busy with another request, or its list of phones and '
              'keys may be full. It keeps running Wellfed, and nothing was '
              'sent.',
        ),
        RestoreStop.notOwner => (
          'Only the owner can do this',
          "This phone isn't the owner of $name, which keeps running Wellfed.",
        ),
        RestoreStop.notAnswering => (
          "$name isn't answering",
          'Nothing was sent. Check that it is plugged in and this phone is '
              'on the same Wi-Fi.',
        ),
        RestoreStop.noUpdates => (
          "$name can't take this",
          'The firmware it runs takes no updates from this app. Nothing was '
              'sent.',
        ),
        RestoreStop.backupRefused => (
          "That isn't the original firmware",
          "It doesn't match the vendor's 1.1.15 exactly, so nothing was sent.",
        ),
        RestoreStop.keyFailed => (
          "This phone couldn't set up the restore",
          '$name keeps running Wellfed, and nothing was sent. The backup is '
              'fine: this is about space or storage on this phone.',
        ),
        RestoreStop.keyGone => (
          'The restore lost its key',
          '$name keeps running Wellfed, and nothing was sent. An earlier '
              'restore on this phone cleared the key as this one was about to '
              'use it. Try again; you may be asked to press Dispense once more.',
        ),
        RestoreStop.refused => (
          '$name said no',
          "It keeps running Wellfed, though the vendor's copy may be partly "
              "written. This isn't sent again from here.",
        ),
        RestoreStop.cancelled => (
          'Stopped',
          '$name keeps running Wellfed, and nothing was sent.',
        ),
        RestoreStop.other || null => (
          'The restore stopped',
          '$name most likely keeps running Wellfed.',
        ),
      };
}

class _Small extends StatelessWidget {
  const _Small(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: TextStyle(fontSize: Sizes.sm, color: context.colors.textMuted),
  );
}

class _Close extends StatelessWidget {
  const _Close({this.outlined = false});

  final bool outlined;

  @override
  Widget build(BuildContext context) {
    void close() => Navigator.of(context).maybePop();
    return outlined
        ? OutlinedButton(onPressed: close, child: const Text('Done'))
        : FilledButton(onPressed: close, child: const Text('Done'));
  }
}
