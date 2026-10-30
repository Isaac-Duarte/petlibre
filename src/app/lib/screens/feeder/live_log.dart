import 'dart:async';

import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/replies.dart';

/// The feeder's record lines as they come (13.3). Owner only: the API has no
/// way to stop a tail, so the feeder goes on pushing until the session ends,
/// and the controller drops what no screen is listening for.
class LiveLogScreen extends StatefulWidget {
  const LiveLogScreen({super.key, required this.feeder, this.share = _share});

  final FeederController feeder;
  final Future<void> Function(String text) share;

  static Future<void> _share(String text) async {
    await SharePlus.instance.share(ShareParams(text: text));
  }

  @override
  State<LiveLogScreen> createState() => _LiveLogScreenState();
}

class _LiveLogScreenState extends State<LiveLogScreen> {
  /// A long tail would otherwise grow for as long as the screen stays open.
  static const _kept = 2000;

  final _lines = <String>[];
  String? _problem;

  @override
  void initState() {
    super.initState();
    widget.feeder.onLog = _heard;
    unawaited(_start());
  }

  @override
  void dispose() {
    widget.feeder.onLog = null;
    super.dispose();
  }

  Future<void> _start() async {
    final reply = await widget.feeder.ask(
      const FeederRequest.tailLog(after: null),
    );
    if (mounted) setState(() => _problem = replyProblem(reply));
  }

  void _heard(int dropped, List<String> lines) {
    setState(() {
      if (dropped > 0) _lines.add('… $dropped lines dropped');
      _lines.addAll(lines);
      if (_lines.length > _kept) _lines.removeRange(0, _lines.length - _kept);
    });
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Scaffold(
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
          'Live log',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
        actions: [
          IconButton(
            tooltip: 'Share the log',
            onPressed: _lines.isEmpty
                ? null
                : () => unawaited(widget.share(_lines.join('\n'))),
            icon: Icon(Icons.share_outlined, color: c.text),
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: _problem != null
                ? Center(child: Text(_problem!))
                : ListView.builder(
                    reverse: true,
                    padding: const EdgeInsets.all(Space.s3),
                    itemCount: _lines.length,
                    itemBuilder: (context, i) => Text(
                      _lines[_lines.length - 1 - i],
                      style: TextStyle(
                        fontFamily: 'monospace',
                        fontSize: 12,
                        height: 1.5,
                        color: c.text,
                      ),
                    ),
                  ),
          ),
          Padding(
            padding: const EdgeInsets.all(Space.s3),
            child: Text(
              'Owner only. Streams while this screen is open.',
              style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}
