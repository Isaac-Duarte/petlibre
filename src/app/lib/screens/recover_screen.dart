import 'dart:async';

import 'package:flutter/material.dart';

import '../platform/discovery.dart';
import '../state/app_state.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/layout.dart';
import 'name_step.dart';
import 'pairing_view.dart';

enum _Step { name, pick, pairing }

/// LAN recovery: a feeder already on this network takes this phone as an
/// owner while its window is open (Lock, then Dispense, held).
class RecoverScreen extends StatefulWidget {
  const RecoverScreen({super.key, required this.app});

  final AppState app;

  @override
  State<RecoverScreen> createState() => _RecoverScreenState();
}

class _RecoverScreenState extends State<RecoverScreen> {
  late _Step _step = widget.app.label.isEmpty ? _Step.name : _Step.pick;
  PairingFlow? _flow;
  String? _address;

  @override
  void dispose() {
    _flow?.dispose();
    super.dispose();
  }

  Future<void> _pair(String address) async {
    _address = address;
    _flow?.dispose();
    final flow = PairingFlow(widget.app);
    setState(() {
      _flow = flow;
      _step = _Step.pairing;
    });
    await flow.runRecovery(address: address);
  }

  @override
  Widget build(BuildContext context) => switch (_step) {
    _Step.name => NameStep(
      initial: widget.app.label,
      onDone: (label) async {
        await widget.app.setLabel(label);
        setState(() => _step = _Step.pick);
      },
    ),
    _Step.pick => _Pick(onPicked: (address) => unawaited(_pair(address))),
    _Step.pairing => PairingView(
      flow: _flow!,
      ble: false,
      onRetry: () => unawaited(_pair(_address!)),
    ),
  };
}

/// Feeders answering on this network, or an address typed in.
class _Pick extends StatefulWidget {
  const _Pick({required this.onPicked});

  final ValueChanged<String> onPicked;

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
      appBarTitle: 'Become its owner again',
      title: 'Open the window, then pick the feeder',
      body:
          'On the feeder, unlock the panel, then hold Lock and Dispense '
          'together. It takes a new owner for 2 minutes.',
      content: [
        if (_found.isEmpty)
          Row(
            children: [
              const SizedBox.square(
                dimension: 16,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
              const SizedBox(width: Space.s3),
              Text(
                'Looking for feeders on this network…',
                style: TextStyle(color: c.textMuted, fontSize: Sizes.base),
              ),
            ],
          ),
        for (final feeder in _found.values)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.s2),
            child: Card(
              clipBehavior: Clip.antiAlias,
              child: ListTile(
                title: Text(feeder.name),
                subtitle: Text(
                  feeder.address,
                  style: const TextStyle(fontFamily: Fonts.mono),
                ),
                trailing: const Icon(Icons.chevron_right),
                onTap: () => widget.onPicked(feeder.address),
              ),
            ),
          ),
        const SizedBox(height: Space.s5),
        TextField(
          controller: _typed,
          keyboardType: TextInputType.url,
          decoration: const InputDecoration(
            labelText: 'Or its address',
            hintText: '192.168.1.20',
          ),
          onChanged: (_) => setState(() {}),
        ),
      ],
      actions: [
        OutlinedButton(
          onPressed: _typed.text.trim().isEmpty
              ? null
              : () => widget.onPicked(_typed.text.trim()),
          child: const Text('Use this address'),
        ),
      ],
    );
  }
}
