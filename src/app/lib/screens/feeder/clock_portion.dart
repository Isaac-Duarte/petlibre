import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/ask_text.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import '../../widgets/when.dart';
import 'today.dart' show clockWords;

/// The clock, its zone and where it asks for the time, and what a portion
/// weighs (13.4).
class ClockPortionScreen extends StatefulWidget {
  const ClockPortionScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<ClockPortionScreen> createState() => _ClockPortionScreenState();
}

class _ClockPortionScreenState extends State<ClockPortionScreen> {
  static const _gramsMost = 50;

  CalibrationInfo? _calibration;
  bool _asking = false;

  @override
  void initState() {
    super.initState();
    unawaited(_read());
  }

  Future<void> _read() async {
    final reply = await widget.feeder.ask(const FeederRequest.getCalibration());
    if (!mounted) return;
    if (reply is FeederReply_Calibration) {
      setState(() => _calibration = reply.calibration);
    } else {
      showProblem(context, reply);
    }
  }

  /// One write, then a fresh reading of the portion.
  Future<void> _write(FeederRequest request) async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(request);
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  Future<void> _portion(PortionChange change) async {
    final calibration = _calibration;
    if (calibration == null) return;
    await _write(
      FeederRequest.putCalibration(
        base: calibration.generation,
        portion: change,
        bowl: calibration.bowl,
      ),
    );
    if (mounted) await _read();
  }

  Future<void> _time({ZoneChange? zone, String? ntp}) async {
    final time = widget.feeder.card?.time;
    if (time == null) return;
    await _write(
      FeederRequest.putTime(base: time.generation, zone: zone, ntp: ntp),
    );
  }

  Future<void> _address() async {
    final address = await askText(
      context,
      title: 'Ask which address?',
      label: 'IPv4 address',
      keyboard: TextInputType.number,
    );
    if (address != null && mounted) await _time(ntp: address);
  }

  Future<void> _weighed() async {
    final grams = await askText(
      context,
      title: 'What did 10 portions weigh?',
      label: 'Grams',
      keyboard: const TextInputType.numberWithOptions(decimal: true),
    );
    if (grams != null && mounted) {
      await _portion(PortionChange.weighed(grams: grams, portions: 10));
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.feeder,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final c = context.colors;
    final card = widget.feeder.card;
    final time = card?.time;
    final clock = time?.clockMs;
    final error = time?.errorMs;
    final zone = time?.zone;
    final label = TextStyle(
      fontSize: Sizes.base,
      fontWeight: Fonts.semibold,
      color: c.text,
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
          'Clock and portion size',
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
          const SectionLabel('Clock'),
          FactList([
            (
              'Feeder time',
              Text(clock == null ? 'Not set' : clockTextMs(context, clock)),
            ),
            if (error != null) ('Could be off by', Text(_errorText(error))),
            if (card != null)
              (
                'Last set',
                Flexible(
                  child: Text(
                    clockWords(context, card),
                    textAlign: TextAlign.end,
                  ),
                ),
              ),
          ]),
          const SizedBox(height: Space.s4),
          Text('Time zone', style: label),
          const SizedBox(height: Space.s1),
          Text(
            zone == null || zone.isEmpty
                ? 'None set: it follows the offset the last phone gave it.'
                : zone,
            style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
          ),
          const SizedBox(height: Space.s2),
          Row(
            children: [
              Expanded(
                child: OutlinedButton(
                  onPressed: _asking || time == null
                      ? null
                      : () => unawaited(_time(zone: const ZoneChange.phone())),
                  child: const Text("This phone's zone"),
                ),
              ),
              const SizedBox(width: Space.s2),
              Expanded(
                child: OutlinedButton(
                  onPressed: _asking || time == null
                      ? null
                      : () => unawaited(_time(zone: const ZoneChange.clear())),
                  child: const Text('Clear it'),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.s4),
          Text('Get the time from', style: label),
          const SizedBox(height: Space.s2),
          Wrap(
            spacing: Space.s2,
            children: [
              ChoiceChip(
                label: const Text('Router'),
                selected: time?.ntp is NtpView_Gateway,
                onSelected: _asking || time == null
                    ? null
                    : (_) => unawaited(_time(ntp: 'gateway')),
              ),
              ChoiceChip(
                label: const Text('Address'),
                selected: time?.ntp is NtpView_Address,
                onSelected: _asking || time == null
                    ? null
                    : (_) => unawaited(_address()),
              ),
              ChoiceChip(
                label: const Text('Phones only'),
                selected: time?.ntp is NtpView_Off,
                onSelected: _asking || time == null
                    ? null
                    : (_) => unawaited(_time(ntp: 'off')),
              ),
            ],
          ),
          if (time?.ntp case NtpView_Address(:final ip, :final port))
            Padding(
              padding: const EdgeInsets.only(top: Space.s1),
              child: Text(
                '$ip:$port',
                style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
              ),
            ),
          const SizedBox(height: Space.s5),
          const SectionLabel('Portion size'),
          _portionRow(context),
          const SizedBox(height: Space.s2),
          Text(
            'Feed 10 portions into an empty bowl, weigh it, and divide by 10. '
            'Depends on your food.',
            style: TextStyle(
              fontSize: Sizes.sm,
              height: 1.5,
              color: c.textMuted,
            ),
          ),
          const SizedBox(height: Space.s2),
          TextButton(
            onPressed: _asking || _calibration == null
                ? null
                : () => unawaited(_weighed()),
            child: const Text('I weighed 10 portions'),
          ),
        ],
      ),
    );
  }

  Widget _portionRow(BuildContext context) {
    final c = context.colors;
    final mg = _calibration?.portionMg;
    final grams = mg == null ? null : (mg / 1000).round();
    final can = !_asking && _calibration != null;
    void step(int next) =>
        unawaited(_portion(PortionChange.milligrams(mg: next * 1000)));
    return Row(
      children: [
        Expanded(
          child: Text(
            'One portion weighs',
            style: TextStyle(fontSize: 15, color: c.text),
          ),
        ),
        IconButton.outlined(
          tooltip: 'Less',
          onPressed: can && grams != null && grams > 1
              ? () => step(grams - 1)
              : null,
          icon: const StrokeIcon(Glyph.minus),
        ),
        SizedBox(
          width: 72,
          child: Text(
            mg == null ? 'Not set' : '${_gramsText(mg)} g',
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: Sizes.md,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
        ),
        IconButton.outlined(
          tooltip: 'More',
          onPressed: can && (grams ?? 0) < _gramsMost
              ? () => step((grams ?? 9) + 1)
              : null,
          icon: const StrokeIcon(Glyph.add),
        ),
      ],
    );
  }

  /// 9.8, or 11 when it is whole.
  static String _gramsText(int mg) =>
      mg % 1000 == 0 ? '${mg ~/ 1000}' : (mg / 1000).toStringAsFixed(1);

  static String _errorText(int errorMs) => errorMs < 1000
      ? 'under 1 s'
      : errorMs < 60000
      ? '${errorMs ~/ 1000} s'
      : '${errorMs ~/ 60000} min';
}
