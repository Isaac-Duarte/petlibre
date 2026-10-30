import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../src/rust/api/session.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import '../../widgets/when.dart';
import 'today.dart' show faultTitle;

AppBar _bar(BuildContext context, String title) {
  final c = context.colors;
  return AppBar(
    toolbarHeight: 64,
    automaticallyImplyLeading: false,
    titleSpacing: 4,
    leading: IconButton(
      tooltip: 'Back',
      onPressed: () => Navigator.pop(context),
      icon: StrokeIcon(Glyph.back, color: c.text),
    ),
    title: Text(
      title,
      style: TextStyle(fontSize: 20, fontWeight: Fonts.semibold, color: c.text),
    ),
  );
}

/// The newest feeds this session has read from the history; a feed the feeder
/// kept no trace of says so when opened.
class MotorTracesScreen extends StatelessWidget {
  const MotorTracesScreen({super.key, required this.feeder});

  final FeederController feeder;

  static const _most = 10;

  @override
  Widget build(BuildContext context) {
    final feeds = [
      for (final entry in feeder.history.reversed)
        if (entry.body case HistoryBody_Fed(:final result)) (entry, result),
    ].take(_most).toList();
    return Scaffold(
      appBar: _bar(context, 'Motor traces'),
      body: feeds.isEmpty
          ? const Center(child: Text('No feeds yet.'))
          : ListView(
              children: [
                for (final (entry, result) in feeds)
                  ListTile(
                    title: Text('Feed ${result.feed}'),
                    subtitle: Text(
                      entry.atMs == null
                          ? deliveredText(result)
                          : '${clockTextMs(context, entry.atMs!)} · '
                                '${deliveredText(result)}',
                    ),
                    onTap: () => unawaited(
                      Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          builder: (_) => MotorTraceScreen(
                            feeder: feeder,
                            feed: result.feed,
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            ),
    );
  }
}

/// What one feed's drive measured (13.2).
class MotorTraceScreen extends StatefulWidget {
  const MotorTraceScreen({super.key, required this.feeder, required this.feed});

  final FeederController feeder;
  final int feed;

  @override
  State<MotorTraceScreen> createState() => _MotorTraceScreenState();
}

class _MotorTraceScreenState extends State<MotorTraceScreen> {
  late final _read = widget.feeder.ask(
    FeederRequest.readTrace(feed: widget.feed),
  );

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: _bar(context, 'Feed ${widget.feed}'),
    body: FutureBuilder<FeederReply>(
      future: _read,
      builder: (context, snapshot) => switch (snapshot.data) {
        null => const Center(child: CircularProgressIndicator()),
        FeederReply_Trace(:final trace) => _Trace(trace),
        FeederReply_Refused(refusal: FeederRefusal_NotFound()) => const Center(
          child: Text('The feeder kept no trace of this feed.'),
        ),
        final reply => Center(child: Text(replyProblem(reply) ?? '')),
      },
    ),
  );
}

class _Trace extends StatelessWidget {
  const _Trace(this.trace);

  final TraceView trace;

  static String _seconds(int ms) => '${(ms / 1000).toStringAsFixed(1)} s';

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final summary = trace.summary;
    final seconds = summary.driveMs / 1000;
    Widget axis(String name, TraceSeries series) => Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          name,
          style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
        ),
        const SizedBox(height: Space.s1),
        SizedBox(
          height: series == TraceSeries.current ? 96 : 24,
          width: double.infinity,
          child: CustomPaint(
            painter: _TracePainter(
              entries: trace.entries,
              series: series,
              spanMs: summary.driveMs,
              color: c.accent,
              line: c.line,
            ),
          ),
        ),
        const SizedBox(height: Space.s3),
      ],
    );

    return ListView(
      padding: const EdgeInsets.all(Space.s4),
      children: [
        Text(
          '${summary.completed} of ${summary.requested} portions · '
          '${seconds.toStringAsFixed(1)} s of motor time',
          style: TextStyle(fontSize: 15, color: c.text),
        ),
        const SizedBox(height: Space.s4),
        axis('Motor current', TraceSeries.current),
        axis('Wheel marks', TraceSeries.marks),
        axis('Food falling past the outlet', TraceSeries.beams),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            for (final part in [0.0, 1 / 3, 2 / 3, 1.0])
              Text(
                '${(seconds * part).round()} s',
                style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
              ),
          ],
        ),
        const SizedBox(height: Space.s4),
        FactList([
          if (trace.portionMs.isNotEmpty)
            (
              'Per portion',
              Flexible(
                child: Text(
                  trace.portionMs.map(_seconds).join(', '),
                  textAlign: TextAlign.end,
                ),
              ),
            ),
          (
            'Stopped because',
            Flexible(
              child: Text(
                summary.reason == FeedReason.done
                    ? 'All portions counted'
                    : faultTitle(context, summary.reason, null),
                textAlign: TextAlign.end,
              ),
            ),
          ),
        ]),
        if (summary.truncated) ...[
          const SizedBox(height: Space.s3),
          Text(
            'The feeder ran out of room, so this trace stops early.',
            style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
          ),
        ],
      ],
    );
  }
}

enum TraceSeries { current, marks, beams }

class _TracePainter extends CustomPainter {
  _TracePainter({
    required this.entries,
    required this.series,
    required this.spanMs,
    required this.color,
    required this.line,
  });

  final List<TraceEntry> entries;
  final TraceSeries series;
  final int spanMs;
  final Color color;
  final Color line;

  @override
  void paint(Canvas canvas, Size size) {
    final ink = Paint()
      ..color = color
      ..strokeWidth = 1.5
      ..style = PaintingStyle.stroke;
    canvas.drawLine(
      Offset(0, size.height),
      Offset(size.width, size.height),
      Paint()..color = line,
    );
    if (spanMs <= 0) return;
    double x(int atMs) => atMs / spanMs * size.width;

    switch (series) {
      case TraceSeries.current:
        final points = [
          for (final entry in entries)
            if (entry case TraceEntry_Current(:final atMs, :final value))
              (atMs, value),
        ];
        if (points.isEmpty) return;
        final top = points.map((p) => p.$2).reduce((a, b) => a > b ? a : b);
        if (top == 0) return;
        final path = Path();
        for (final (i, (atMs, value)) in points.indexed) {
          final y = size.height * (1 - value / top);
          if (i == 0) {
            path.moveTo(x(atMs), y);
          } else {
            path.lineTo(x(atMs), y);
          }
        }
        canvas.drawPath(path, ink);
      case TraceSeries.marks:
        for (final entry in entries) {
          if (entry case TraceEntry_Mark(:final atMs, high: true)) {
            canvas.drawLine(
              Offset(x(atMs), 0),
              Offset(x(atMs), size.height),
              ink,
            );
          }
        }
      case TraceSeries.beams:
        for (final entry in entries) {
          if (entry case TraceEntry_Beams(:final atMs, outletClear: false)) {
            canvas.drawLine(
              Offset(x(atMs), 0),
              Offset(x(atMs), size.height),
              ink,
            );
          }
        }
    }
  }

  @override
  bool shouldRepaint(_TracePainter old) => old.entries != entries;
}
