import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import 'clock_portion.dart';
import 'live_log.dart';
import 'motor_trace.dart';

/// The feeder's own numbers, for an owner (13.1). Reads only, each once when
/// the screen opens.
class DiagnosticsScreen extends StatefulWidget {
  const DiagnosticsScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<DiagnosticsScreen> createState() => _DiagnosticsScreenState();
}

class _DiagnosticsScreenState extends State<DiagnosticsScreen> {
  Counters? _counters;
  Memory? _memory;
  Flash? _flash;
  String? _problem;

  @override
  void initState() {
    super.initState();
    unawaited(_read());
  }

  Future<void> _read() async {
    for (final section in const [
      StatsSection.counters,
      StatsSection.memory,
      StatsSection.flash,
    ]) {
      final reply = await widget.feeder.ask(
        FeederRequest.stats(section: section),
      );
      if (!mounted) return;
      setState(() {
        switch (reply) {
          case FeederReply_Stats(stats: Stats_Counters(field0: final value)):
            _counters = value;
          case FeederReply_Stats(stats: Stats_Memory(field0: final value)):
            _memory = value;
          case FeederReply_Stats(stats: Stats_Flash(field0: final value)):
            _flash = value;
          default:
            _problem ??= replyProblem(reply);
        }
      });
    }
  }

  void _open(Widget screen) => unawaited(
    Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => screen)),
  );

  /// `3 d 4 h`, `4 h 12 min` or `12 min`.
  static String upText(int seconds) {
    final minutes = seconds ~/ 60;
    final hours = minutes ~/ 60;
    final days = hours ~/ 24;
    if (days > 0) return '$days d ${hours % 24} h';
    if (hours > 0) return '$hours h ${minutes % 60} min';
    return '$minutes min';
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final feeder = widget.feeder;
    final counters = _counters;
    final boot = counters?.boot;
    final lifetime = counters?.lifetime;
    final resets = counters?.resets;
    final restarts = resets == null
        ? null
        : resets.powerOn +
              resets.watchdog +
              resets.software +
              resets.brownout +
              resets.other;
    final storage = (_flash?.lifetime ?? _flash?.boot)?.storage;
    final worst = storage == null || storage.isEmpty
        ? null
        : storage.reduce((a, b) => a > b ? a : b);
    final card = feeder.card;

    Widget tile(String name, String value) => Expanded(
      child: Panel(
        padding: const EdgeInsets.all(Space.s3),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              value,
              style: TextStyle(
                fontSize: 22,
                fontWeight: Fonts.semibold,
                color: c.text,
              ),
            ),
            Text(
              name,
              style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
            ),
          ],
        ),
      ),
    );

    Widget link(String name, String? note, Widget screen) => ListTile(
      contentPadding: const EdgeInsets.symmetric(horizontal: Space.s4),
      title: Text(name),
      subtitle: note == null ? null : Text(note),
      trailing: StrokeIcon(Glyph.chevron, size: 20, color: c.textMuted),
      onTap: () => _open(screen),
    );

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
          'Diagnostics',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.s4),
        children: [
          if (_problem case final problem?) ...[
            Notice(tone: Tone.warning, glyph: Glyph.warning, text: problem),
            const SizedBox(height: Space.s3),
          ],
          const SectionLabel('Since it last started'),
          const SizedBox(height: Space.s2),
          Row(
            children: [
              tile('feeds', '${boot?.feeds ?? '–'}'),
              const SizedBox(width: Space.s2),
              tile("didn't finish", '${boot?.feedsFailed ?? '–'}'),
            ],
          ),
          const SizedBox(height: Space.s2),
          Text(
            'History has every feed it kept, from before that too.',
            style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
          ),
          // The store saves hourly, so an unclean restart loses what came since
          // and a new store starts from zero: these are floors, not totals.
          if (lifetime != null || restarts != null) ...[
            const SizedBox(height: Space.s4),
            const SectionLabel('Counted so far, at least'),
            const SizedBox(height: Space.s2),
            FactList([
              if (lifetime != null) ('Feeds', Text('${lifetime.feeds}')),
              if (restarts != null) ('Starts', Text('$restarts')),
            ]),
          ],
          const SizedBox(height: Space.s4),
          const SectionLabel('Right now'),
          const SizedBox(height: Space.s2),
          FactList([
            if (counters != null)
              ('Up for', Text(upText(counters.boot.uptimeS))),
            if (card?.rssi case final rssi?)
              ('Wi-Fi signal', Text('$rssi dBm')),
            if (_memory case final memory?)
              ('Free memory, lowest', Text('${memory.heapMin ~/ 1024} KB')),
            if (worst != null) ('Flash erases, worst sector', Text('$worst')),
            if (card != null) ('Running from', Text('Bank ${card.bank}')),
          ]),
          const SizedBox(height: Space.s4),
          const SectionLabel('Look closer'),
          const SizedBox(height: Space.s2),
          Panel(
            child: Column(
              children: [
                link(
                  'Motor traces',
                  'Recent feeds',
                  MotorTracesScreen(feeder: feeder),
                ),
                link('Live log', null, LiveLogScreen(feeder: feeder)),
                link(
                  'Clock and portion size',
                  null,
                  ClockPortionScreen(feeder: feeder),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
