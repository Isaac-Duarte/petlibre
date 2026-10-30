import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/install.dart';
import '../../src/rust/api/update.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import 'update_end.dart';

enum _Stage { offer, sending, restarting, ended }

/// The firmware on this feeder and the update this app carries (12.1, 12.2).
/// One press offers it once. Nothing here sends it again: an update that ended
/// without certainty ends on a screen that says so, and only one that never
/// reached the feeder offers another try.
class UpdatesScreen extends StatefulWidget {
  const UpdatesScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<UpdatesScreen> createState() => _UpdatesScreenState();
}

class _UpdatesScreenState extends State<UpdatesScreen> {
  ImageCheck? _image;
  UpdateInfo? _state;
  _Stage _stage = _Stage.offer;
  int _written = 0;
  int _length = 0;
  bool _reconnecting = false;
  UpdateEnd? _end;
  bool _cancelling = false;
  StreamSubscription<UpdateProgress>? _run;

  FeederController get _feeder => widget.feeder;

  @override
  void initState() {
    super.initState();
    unawaited(_read());
  }

  @override
  void dispose() {
    // The driver's thread goes on: a feeder mid-install wants its chunks, and
    // leaving this screen is not a cancel.
    unawaited(_run?.cancel());
    super.dispose();
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

  void _install() {
    final address = _feeder.known.address;
    if (address == null) return;
    setState(() => _stage = _Stage.sending);
    _run = _feeder.ports
        .pushUpdate(_feeder.known.identity, address)
        .listen(
          _heard,
          // Rust ends every run with a value; a stream that closes without one
          // lost its thread, and what the feeder did is not known here.
          onDone: () {
            if (mounted && _stage != _Stage.ended) {
              setState(() {
                _stage = _Stage.ended;
                _end = const UpdateEnd.uncertain();
              });
            }
          },
        );
  }

  void _heard(UpdateProgress progress) {
    if (!mounted) return;
    setState(() {
      switch (progress) {
        case UpdateProgress_Sending(:final written, :final length):
          _written = written;
          _length = length;
          _reconnecting = false;
        case UpdateProgress_Reconnecting():
          _reconnecting = true;
        case UpdateProgress_Activated():
          _stage = _Stage.restarting;
          _reconnecting = false;
        case UpdateProgress_Ended(:final end):
          _stage = _Stage.ended;
          _end = end;
      }
    });
  }

  Future<void> _cancel() async {
    setState(() => _cancelling = true);
    final reply = await _feeder.ask(const FeederRequest.cancelUpdate());
    if (!mounted) return;
    setState(() => _cancelling = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    if (_end case final end? when _stage == _Stage.ended) {
      return UpdateEndScreen(
        name: _feeder.known.name,
        end: end,
        onTryAgain: end is UpdateEnd_NotOffered
            ? () => setState(() {
                _end = null;
                _stage = _Stage.offer;
              })
            : null,
      );
    }
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
          _stage == _Stage.offer
              ? 'Firmware'
              : 'Updating ${_feeder.known.name}',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: context.colors.text,
          ),
        ),
      ),
      body: _stage == _Stage.offer ? _offer(context) : _progress(context),
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
    final can =
        offered != null &&
        !current &&
        _feeder.connection == Connection.online &&
        _feeder.known.address != null;

    return ListView(
      padding: const EdgeInsets.all(Space.s4),
      children: [
        const SectionLabel('Firmware'),
        const SizedBox(height: Space.s2),
        FactList([
          if (state != null && state.minVersion > 0)
            ('Version', Text('${state.minVersion}')),
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
            "If the new version doesn't start properly, $name goes back to "
            'this one by itself.',
            style: TextStyle(fontSize: 15, height: 1.5, color: c.text),
          ),
          const SizedBox(height: Space.s4),
          FilledButton(
            onPressed: can ? _install : null,
            child: Text('Install on $name'),
          ),
        ],
        const SizedBox(height: Space.s5),
        const SectionLabel('Original firmware'),
        const SizedBox(height: Space.s2),
        // Returning to stock opens from here. It is its own flow, with its own
        // fresh key and a press on the feeder.
        const OutlinedButton(
          onPressed: null,
          child: Text('Put the original firmware back'),
        ),
      ],
    );
  }

  Widget _progress(BuildContext context) {
    final c = context.colors;
    final name = _feeder.known.name;
    final sending = _stage == _Stage.sending;
    final accepted = _length > 0;
    final percent = accepted ? (_written * 100 ~/ _length) : 0;
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
              ? (_reconnecting ? 'Reconnecting…' : 'Sending $percent%')
              : 'Sent',
        ),
        _Line(
          done: false,
          active: !sending,
          text: 'Restart on the new version',
        ),
        _Line(
          done: false,
          active: false,
          text: 'Check it can feed, then keep it',
        ),
        const SizedBox(height: Space.s4),
        Text(
          'Keep the feeder plugged in. If this phone loses the connection, the '
          "feeder keeps the version it's running.",
          style: TextStyle(fontSize: Sizes.sm, height: 1.5, color: c.textMuted),
        ),
        if (sending) ...[
          const SizedBox(height: Space.s4),
          OutlinedButton(
            onPressed: _cancelling ? null : () => unawaited(_cancel()),
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
