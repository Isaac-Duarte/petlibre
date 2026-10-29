import 'dart:convert';

import 'package:device_info_plus/device_info_plus.dart';
import 'package:flutter/material.dart';

import '../theme/theme.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';

/// Name things: what this phone calls the feeder (only this phone keeps
/// it) and how the feeder lists this phone among its clients, which it
/// keeps in at most 16 bytes.
class NameStep extends StatefulWidget {
  const NameStep({
    super.key,
    required this.initial,
    required this.onDone,
    required this.action,
    this.feeder,
    this.stepLabel,
    this.progress,
    this.onBack,
  });

  /// The phone's name so far; empty until the owner names it.
  final String initial;

  /// The feeder's name so far, or `null` to not ask for one.
  final String? feeder;
  final String action;
  final String? stepLabel;
  final double? progress;
  final VoidCallback? onBack;

  /// The phone's name, then the feeder's when it was asked.
  final void Function(String label, String? feeder) onDone;

  static const maxBytes = 16;

  @override
  State<NameStep> createState() => _NameStepState();
}

class _NameStepState extends State<NameStep> {
  final _name = TextEditingController();
  late final _feeder = TextEditingController(text: widget.feeder);

  @override
  void initState() {
    super.initState();
    _name.text = widget.initial;
    if (widget.initial.isEmpty) _suggest();
  }

  /// The phone's model, cut to fit.
  Future<void> _suggest() async {
    final android = await DeviceInfoPlugin().androidInfo;
    if (!mounted || _name.text.isNotEmpty) return;
    var model = android.model;
    while (utf8.encode(model).length > NameStep.maxBytes) {
      model = model.substring(0, model.length - 1);
    }
    setState(() => _name.text = model);
  }

  @override
  void dispose() {
    _name.dispose();
    _feeder.dispose();
    super.dispose();
  }

  int get _bytes => utf8.encode(_name.text.trim()).length;

  bool get _valid =>
      _bytes >= 1 &&
      _bytes <= NameStep.maxBytes &&
      (widget.feeder == null || _feeder.text.trim().isNotEmpty);

  @override
  Widget build(BuildContext context) {
    final asksFeeder = widget.feeder != null;
    return StepScaffold(
      stepLabel: widget.stepLabel,
      progress: widget.progress,
      onLeading: widget.onBack,
      title: asksFeeder ? 'Name things' : 'Name this phone',
      content: [
        if (asksFeeder) ...[
          LabeledField(
            label: 'This feeder',
            hint: 'Only this phone uses it.',
            child: TextField(
              controller: _feeder,
              textCapitalization: TextCapitalization.words,
              textInputAction: TextInputAction.next,
              onChanged: (_) => setState(() {}),
            ),
          ),
          const SizedBox(height: 20),
        ],
        LabeledField(
          label: 'This phone, as the feeder shows it',
          hint: "Others see it in the feeder's People list and its history.",
          counter: '$_bytes/${NameStep.maxBytes}',
          child: TextField(
            controller: _name,
            autofocus: widget.initial.isEmpty && !asksFeeder,
            textInputAction: TextInputAction.done,
            onChanged: (_) => setState(() {}),
          ),
        ),
        const SizedBox(height: 20),
        const Notice(
          tone: Tone.neutral,
          glyph: Glyph.key,
          radius: Radii.card,
          text:
              "You'll be this feeder's **owner**. Its key stays on this "
              "phone; there's no account to sign in to.",
        ),
      ],
      actions: [
        FilledButton(
          onPressed: _valid
              ? () => widget.onDone(
                  _name.text.trim(),
                  asksFeeder ? _feeder.text.trim() : null,
                )
              : null,
          child: Text(widget.action),
        ),
      ],
    );
  }
}
