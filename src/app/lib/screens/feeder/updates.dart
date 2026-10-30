import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/install.dart';
import '../../state/feeder.dart';
import '../../state/update.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import '../restore_screen.dart';
import 'update_end.dart';

/// The firmware on this feeder and the update this app carries (12.1, 12.2).
/// One press offers it once. The run belongs to the feeder's controller, so
/// leaving and coming back shows it as it stands rather than a second offer.
class UpdatesScreen extends StatefulWidget {
  const UpdatesScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<UpdatesScreen> createState() => _UpdatesScreenState();
}

class _UpdatesScreenState extends State<UpdatesScreen> {
  ImageCheck? _image;
  UpdateInfo? _state;

  FeederController get _feeder => widget.feeder;
  UpdateFlow get _flow => widget.feeder.update;

  @override
  void initState() {
    super.initState();
    unawaited(_read());
  }

  Future<void> _read() async {
    final image = await _feeder.ports.updateImage();
    final reply = await _feeder.ask(const FeederRequest.updateState());
    if (!mounted) return;
    setState(() {
      _image = image;
      if (reply is FeederReply_Update) _state = reply.update;
    });
  }

  Future<void> _cancel() async {
    final reply = await _flow.cancel();
    if (mounted) showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([_flow, _feeder]),
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    if (_flow.end case final end? when _flow.stage == UpdateStage.ended) {
      return UpdateEndScreen(feeder: _feeder, end: end);
    }
    final offering = _flow.stage == UpdateStage.idle;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Back',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.back, color: context.colors.text),
        ),
        title: Text(
          offering ? 'Firmware' : 'Updating ${_feeder.known.name}',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: context.colors.text,
          ),
        ),
      ),
      body: offering ? _offer(context) : _progress(context),
    );
  }

  Widget _offer(BuildContext context) {
    final c = context.colors;
    final name = _feeder.known.name;
    final card = _feeder.card;
    final build = card?.build;
    final state = _state;
    final offered = switch (_image) {
      ImageCheck_Ready(:final build, :final version) => (build, version),
      _ => null,
    };
    final current = offered != null && build != null && offered.$1 == build;
    final can = offered != null && !current && _feeder.cardIsCurrent;

    return ListView(
      padding: const EdgeInsets.all(Space.s4),
      children: [
        const SectionLabel('Firmware'),
        const SizedBox(height: Space.s2),
        FactList([
          // The floor, not what runs: a feeder on bank 0 may run older.
          if (state != null && state.minVersion > 0)
            ('Newest confirmed version', Text('${state.minVersion}')),
          if (build != null) ('Build', Text(_short(build))),
          if (card != null) ('Running from', Text('Bank ${card.bank}')),
        ]),
        const SizedBox(height: Space.s4),
        if (_image == null)
          Text(
            'Looking at what this app carries…',
            style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
          )
        else if (current)
          Notice(
            tone: Tone.neutral,
            glyph: Glyph.check,
            text: '$name runs the version this app carries.',
          )
        else if (offered == null)
          const Notice(
            tone: Tone.neutral,
            glyph: Glyph.info,
            text: 'This app carries no update it can install.',
          )
        else ...[
          const SectionLabel('New'),
          const SizedBox(height: Space.s2),
          FactList([
            ('Version', Text('${offered.$2}')),
            ('Build', Text(_short(offered.$1))),
          ]),
          const SizedBox(height: Space.s3),
          Text(
            'Takes about a minute. Feeding waits while it installs, and meals '
            "due in that minute run when it's done.",
            style: TextStyle(fontSize: 15, height: 1.5, color: c.text),
          ),
          const SizedBox(height: Space.s2),
          Text(
            "If the new version doesn't start properly, $name goes to its "
            'other copy of Wellfed by itself. That copy can feed, and may be '
            'older than the one it runs now.',
            style: TextStyle(fontSize: 15, height: 1.5, color: c.text),
          ),
          const SizedBox(height: Space.s4),
          FilledButton(
            onPressed: can
                ? () => unawaited(
                    _flow.start(build: offered.$1, version: offered.$2),
                  )
                : null,
            child: Text('Install on $name'),
          ),
        ],
        const SizedBox(height: Space.s5),
        const SectionLabel('Original firmware'),
        const SizedBox(height: Space.s2),
        OutlinedButton(
          onPressed: () => unawaited(
            Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => RestoreScreen(
                  feeder: _feeder.known,
                  ports: _feeder.ports.restore,
                ),
              ),
            ),
          ),
          child: const Text('Restore original firmware'),
        ),
      ],
    );
  }

  Widget _progress(BuildContext context) {
    final c = context.colors;
    final flow = _flow;
    final name = _feeder.known.name;
    final sending = flow.stage == UpdateStage.sending;
    final accepted = flow.length > 0;
    final percent = accepted ? (flow.written * 100 ~/ flow.length) : 0;
    return ListView(
      padding: const EdgeInsets.all(Space.s4),
      children: [
        _Line(
          done: accepted || !sending,
          active: !accepted && sending,
          text: '$name accepted the update',
        ),
        _Line(
          done: !sending,
          active: accepted && sending,
          text: sending
              ? (flow.reconnecting ? 'Reconnecting…' : 'Sending $percent%')
              : 'Sent',
        ),
        _Line(
          done: false,
          active: !sending,
          text: 'Restart on the new version',
        ),
        const _Line(
          done: false,
          active: false,
          text: 'Check it can feed, then keep it',
        ),
        const SizedBox(height: Space.s4),
        Text(
          'Keep the feeder plugged in. If this phone loses the connection, the '
          'feeder runs its other copy of Wellfed, which can feed and may be '
          'older than the one it ran.',
          style: TextStyle(fontSize: Sizes.sm, height: 1.5, color: c.textMuted),
        ),
        if (flow.holdRefused) ...[
          const SizedBox(height: Space.s3),
          const Notice(
            tone: Tone.warning,
            glyph: Glyph.warning,
            text: 'Keep this app open until it finishes.',
          ),
        ],
        if (sending) ...[
          const SizedBox(height: Space.s4),
          OutlinedButton(
            onPressed: flow.cancelling ? null : () => unawaited(_cancel()),
            child: const Text('Cancel update'),
          ),
        ],
      ],
    );
  }

  static String _short(String build) =>
      build.length > 12 ? build.substring(0, 12) : build;
}

class _Line extends StatelessWidget {
  const _Line({required this.done, required this.active, required this.text});

  final bool done;
  final bool active;
  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s2),
      child: Row(
        children: [
          SizedBox(
            width: 24,
            height: 24,
            child: done
                ? StrokeIcon(Glyph.check, size: 20, color: c.ok)
                : active
                ? const CircularProgressIndicator(strokeWidth: 2)
                : null,
          ),
          const SizedBox(width: Space.s3),
          Expanded(
            child: Text(
              text,
              style: TextStyle(
                fontSize: 15,
                color: done || active ? c.text : c.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
