import 'dart:async';

import 'package:flutter/material.dart';

import '../platform/discovery.dart';
import '../platform/pairing_ports.dart';
import '../state/app_state.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/code_tiles.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import 'name_step.dart';
import 'pairing_view.dart';

enum _Step { name, pick, pairing }

/// LAN recovery: a feeder already on this network takes this phone as an
/// owner while its window is open (Lock, then Dispense, held).
class RecoverScreen extends StatefulWidget {
  const RecoverScreen({super.key, required this.app, required this.ports});

  final FeederBook app;
  final PairingPorts ports;

  @override
  State<RecoverScreen> createState() => _RecoverScreenState();
}

class _RecoverScreenState extends State<RecoverScreen> {
  late _Step _step = widget.app.label.isEmpty ? _Step.name : _Step.pick;
  PairingFlow? _flow;
  (String, String)? _picked;

  @override
  void dispose() {
    _flow?.dispose();
    super.dispose();
  }

  /// Pair with the feeder at `address`, saved under `name`.
  Future<void> _pair(String address, String name) async {
    _picked = (address, name);
    _flow?.dispose();
    final flow = PairingFlow(widget.app, widget.ports);
    setState(() {
      _flow = flow;
      _step = _Step.pairing;
    });
    await flow.runRecovery(address: address, name: name);
  }

  @override
  Widget build(BuildContext context) => switch (_step) {
    _Step.name => NameStep(
      initial: widget.app.label,
      action: 'Continue',
      onDone: (label, _) async {
        await widget.app.setLabel(label);
        if (mounted) setState(() => _step = _Step.pick);
      },
    ),
    _Step.pick => _Pick(
      onPicked: (address, name) => unawaited(_pair(address, name)),
    ),
    _Step.pairing => PairingView(
      flow: _flow!,
      ble: false,
      onRetry: () => unawaited(_pair(_picked!.$1, _picked!.$2)),
      onNewCode: () => unawaited(_pair(_picked!.$1, _picked!.$2)),
    ),
  };
}

/// How to open the window, then the feeders answering on this network, or
/// an address typed in.
class _Pick extends StatefulWidget {
  const _Pick({required this.onPicked});

  final void Function(String address, String name) onPicked;

  @override
  State<_Pick> createState() => _PickState();
}

class _PickState extends State<_Pick> {
  final _found = <String, FoundFeeder>{};
  final _typed = TextEditingController();
  StreamSubscription<FoundFeeder>? _browsing;

  @override
  void initState() {
    super.initState();
    _browsing = Discovery.browse().listen(
      (feeder) => setState(() => _found[feeder.identity] = feeder),
      onError: (Object _) {},
    );
  }

  @override
  void dispose() {
    unawaited(_browsing?.cancel());
    _typed.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return StepScaffold(
      appBarTitle: 'Recover a feeder',
      body:
          'Lost every phone that could open it? If the feeder is still on '
          'your Wi-Fi, you can add this phone as an owner without erasing '
          'anything.',
      content: [
        const PanelPhoto(held: [1, 2], caption: 'Lock, then Dispense'),
        const SizedBox(height: 18),
        const NumberedSteps([
          'Unlock the panel if the lock light is on.',
          'Hold **Lock**, then also **Dispense**, for 5 seconds. The lock '
              'light blinks fast.',
          'Within 2 minutes, pick the feeder below and press the code it '
              'shows, just like setup.',
        ]),
        const SizedBox(height: Space.s5),
        if (_found.isEmpty)
          const Searching('Looking for feeders on this network…'),
        for (final feeder in _found.values) ...[
          Panel(
            onTap: () => widget.onPicked(feeder.address, feeder.name),
            padding: const EdgeInsets.all(Space.s4),
            child: Row(
              children: [
                const FeederImage(size: 48),
                const SizedBox(width: Space.s3),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        feeder.name,
                        style: TextStyle(
                          fontSize: Sizes.md,
                          fontWeight: Fonts.semibold,
                          color: c.text,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        feeder.address,
                        style: TextStyle(
                          fontFamily: Fonts.mono,
                          fontSize: Sizes.sm,
                          color: c.textMuted,
                        ),
                      ),
                    ],
                  ),
                ),
                StrokeIcon(Glyph.chevron, size: 20, color: c.textMuted),
              ],
            ),
          ),
          const SizedBox(height: Space.s2),
        ],
        const SizedBox(height: Space.s5),
        LabeledField(
          label: 'Or its address',
          child: TextField(
            controller: _typed,
            keyboardType: TextInputType.url,
            decoration: const InputDecoration(hintText: '192.168.1.20'),
            onChanged: (_) => setState(() {}),
          ),
        ),
      ],
      actions: [
        OutlinedButton(
          onPressed: _typed.text.trim().isEmpty
              ? null
              : () => widget.onPicked(
                  _typed.text.trim(),
                  PairingFlow.defaultFeederName,
                ),
          child: const Text('Use this address'),
        ),
      ],
    );
  }
}
