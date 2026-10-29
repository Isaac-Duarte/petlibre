import 'dart:convert';

import 'package:device_info_plus/device_info_plus.dart';
import 'package:flutter/material.dart';

import '../theme/theme.dart';
import '../widgets/layout.dart';

/// Name this phone: how feeders list it among their clients. The feeder
/// keeps at most 16 bytes.
class NameStep extends StatefulWidget {
  const NameStep({super.key, required this.initial, required this.onDone});

  final String initial;
  final ValueChanged<String> onDone;

  static const maxBytes = 16;

  @override
  State<NameStep> createState() => _NameStepState();
}

class _NameStepState extends State<NameStep> {
  final _name = TextEditingController();

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
    super.dispose();
  }

  bool get _valid {
    final bytes = utf8.encode(_name.text.trim()).length;
    return bytes >= 1 && bytes <= NameStep.maxBytes;
  }

  @override
  Widget build(BuildContext context) => StepScaffold(
    title: 'Name this phone',
    body: 'The feeder lists its owners by name, so you can tell them apart.',
    content: [
      TextField(
        controller: _name,
        autofocus: widget.initial.isEmpty,
        textInputAction: TextInputAction.done,
        decoration: const InputDecoration(labelText: 'Phone name'),
        onChanged: (_) => setState(() {}),
      ),
      const SizedBox(height: Space.s2),
      Text(
        'Up to 16 characters.',
        style: TextStyle(fontSize: Sizes.sm, color: context.colors.textMuted),
      ),
    ],
    actions: [
      FilledButton(
        onPressed: _valid ? () => widget.onDone(_name.text.trim()) : null,
        child: const Text('Continue'),
      ),
    ],
  );
}
