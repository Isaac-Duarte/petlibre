import 'dart:async';

import 'package:flutter/material.dart';
import 'package:network_info_plus/network_info_plus.dart';
import 'package:permission_handler/permission_handler.dart';

import '../src/rust/api/setup.dart';
import '../state/app_state.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/code_tiles.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import 'name_step.dart';
import 'pairing_view.dart';

enum _Step { permissions, setupMode, wifi, names, pairing }

/// What setup asks Android for: Bluetooth to reach the feeder, and
/// location only because Android needs it to read the Wi-Fi's name.
const _permissions = [
  Permission.bluetoothScan,
  Permission.bluetoothConnect,
  Permission.locationWhenInUse,
];

/// BLE setup: put the feeder in setup mode, give it its Wi-Fi, name it and
/// this phone, then pick it and key in the code. The name and Wi-Fi come
/// first because the setup session carries them with the code.
class SetupScreen extends StatefulWidget {
  const SetupScreen({super.key, required this.app});

  final AppState app;

  @override
  State<SetupScreen> createState() => _SetupScreenState();
}

class _SetupScreenState extends State<SetupScreen> {
  _Step _step = _Step.setupMode;
  WifiCredentials? _wifi;
  String _feederName = PairingFlow.defaultFeederName;
  PairingFlow? _flow;

  @override
  void initState() {
    super.initState();
    unawaited(_askFirst());
  }

  /// The permissions screen only when something is still to grant.
  Future<void> _askFirst() async {
    for (final permission in _permissions) {
      if (!await permission.isGranted) {
        if (mounted) setState(() => _step = _Step.permissions);
        return;
      }
    }
  }

  @override
  void dispose() {
    _flow?.dispose();
    super.dispose();
  }

  Future<void> _grant() async {
    final granted = await _permissions.request();
    if (!mounted) return;
    final bluetooth = [
      granted[Permission.bluetoothScan],
      granted[Permission.bluetoothConnect],
    ];
    if (bluetooth.any((status) => status?.isGranted != true)) {
      await _bluetoothOff();
      return;
    }
    setState(() => _step = _Step.setupMode);
  }

  Future<void> _bluetoothOff() => showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: const Text('Nearby devices is off'),
      content: const Text(
        'Setup talks to the feeder over Bluetooth. Allow "Nearby '
        'devices" for Wellfed in Android settings, then try again.',
      ),
      actions: [
        TextButton(
          onPressed: () => unawaited(openAppSettings()),
          child: const Text('Open settings'),
        ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('OK'),
        ),
      ],
    ),
  );

  Future<void> _pair() async {
    final granted = await [
      Permission.bluetoothScan,
      Permission.bluetoothConnect,
    ].request();
    if (!mounted) return;
    if (granted.values.any((status) => !status.isGranted)) {
      await _bluetoothOff();
      return;
    }

    _flow?.dispose();
    final flow = PairingFlow(widget.app);
    setState(() {
      _flow = flow;
      _step = _Step.pairing;
    });
    await flow.runSetup(wifi: _wifi, name: _feederName);
  }

  void _to(_Step step) => setState(() => _step = step);

  @override
  Widget build(BuildContext context) => switch (_step) {
    _Step.permissions => _Permissions(onContinue: () => unawaited(_grant())),
    _Step.setupMode => StepScaffold(
      stepLabel: 'Step 1 of 4',
      progress: 0.25,
      leading: Leading.close,
      title: 'Put the feeder in setup mode',
      content: const [
        PanelPhoto(held: [1, 3]),
        SizedBox(height: Space.s4),
        NumberedSteps([
          'Plug the feeder in. If the lock light is on, hold **Lock** for '
              '5 seconds to unlock the panel.',
          'Hold **Lock** and **Mic** together for 5 seconds.',
          'The Wi-Fi light blinks slowly, about once a second. Setup mode '
              'lasts 15 minutes.',
        ]),
      ],
      actions: [
        FilledButton(
          onPressed: () => _to(_Step.wifi),
          child: const Text('The Wi-Fi light is blinking'),
        ),
      ],
    ),
    _Step.wifi => _WifiStep(
      initial: _wifi,
      onBack: () => _to(_Step.setupMode),
      onDone: (wifi) {
        _wifi = wifi;
        _to(_Step.names);
      },
    ),
    _Step.names => NameStep(
      initial: widget.app.label,
      feeder: _feederName,
      stepLabel: 'Step 3 of 4',
      progress: 0.75,
      onBack: () => _to(_Step.wifi),
      action: 'Find the feeder',
      onDone: (label, feeder) async {
        _feederName = feeder ?? PairingFlow.defaultFeederName;
        await widget.app.setLabel(label);
        unawaited(_pair());
      },
    ),
    _Step.pairing => PairingView(
      flow: _flow!,
      ble: true,
      onRetry: () => unawaited(_pair()),
    ),
  };
}

/// What setup will ask Android for, before it asks.
class _Permissions extends StatelessWidget {
  const _Permissions({required this.onContinue});

  final VoidCallback onContinue;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    Widget row(Glyph glyph, String title, String body) => Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        IconTile(glyph, size: 44),
        const SizedBox(width: Space.s4),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: TextStyle(
                  fontSize: Sizes.md,
                  fontWeight: Fonts.semibold,
                  color: c.text,
                ),
              ),
              const SizedBox(height: Space.s1),
              Text(
                body,
                style: TextStyle(
                  fontSize: Sizes.base,
                  height: 1.5,
                  color: c.textMuted,
                ),
              ),
            ],
          ),
        ),
      ],
    );

    return StepScaffold(
      title: 'Two permissions, used only on your network',
      content: [
        row(
          Glyph.bluetooth,
          'Nearby devices',
          'Bluetooth, only while you set a feeder up. A feeder turns '
              'Bluetooth off once setup finishes.',
        ),
        const SizedBox(height: 20),
        row(
          Glyph.wifi,
          'Wi-Fi network',
          'To find your feeders on the network and to fill in your Wi-Fi '
              'name during setup.',
        ),
      ],
      actions: [
        Text(
          'Nothing you do in Wellfed leaves your home network unless you '
          'add a server yourself.',
          style: TextStyle(fontSize: Sizes.sm, height: 1.5, color: c.textMuted),
        ),
        FilledButton(onPressed: onContinue, child: const Text('Continue')),
      ],
    );
  }
}

/// The network the feeder joins after setup. The phone's current network
/// is filled in when location is allowed; Android needs it to read the
/// name, and nothing else uses it.
class _WifiStep extends StatefulWidget {
  const _WifiStep({
    required this.initial,
    required this.onBack,
    required this.onDone,
  });

  final WifiCredentials? initial;
  final VoidCallback onBack;
  final ValueChanged<WifiCredentials?> onDone;

  @override
  State<_WifiStep> createState() => _WifiStepState();
}

class _WifiStepState extends State<_WifiStep> {
  late final _ssid = TextEditingController(text: widget.initial?.ssid);
  late final _password = TextEditingController(
    text: widget.initial?.passphrase,
  );
  bool _hidden = true;

  /// The name is typed, not the phone's current network.
  bool _typing = false;

  @override
  void initState() {
    super.initState();
    unawaited(_prefill());
  }

  Future<void> _prefill() async {
    if (!await Permission.locationWhenInUse.isGranted) {
      if (mounted) setState(() => _typing = true);
      return;
    }
    final name = await NetworkInfo().getWifiName();
    if (!mounted) return;
    if (name == null) {
      setState(() => _typing = true);
      return;
    }
    if (_ssid.text.isNotEmpty) return;
    // Android quotes the name.
    setState(() => _ssid.text = name.replaceAll(RegExp(r'^"|"$'), ''));
  }

  @override
  void dispose() {
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  bool get _valid =>
      _ssid.text.isNotEmpty &&
      _password.text.length >= 8 &&
      _password.text.length <= 63;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return StepScaffold(
      stepLabel: 'Step 2 of 4',
      progress: 0.5,
      onLeading: widget.onBack,
      title: 'Connect it to your Wi-Fi',
      content: [
        LabeledField(
          label: 'Network',
          child: _typing || _ssid.text.isEmpty
              ? TextField(
                  controller: _ssid,
                  autofocus: _typing,
                  textInputAction: TextInputAction.next,
                  decoration: const InputDecoration(hintText: 'Network name'),
                  onChanged: (_) => setState(() {}),
                )
              : InputDecorator(
                  decoration: InputDecoration(
                    prefixIcon: Padding(
                      padding: const EdgeInsets.only(left: 14, right: 10),
                      child: StrokeIcon(Glyph.wifi, size: 20, color: c.text),
                    ),
                    prefixIconConstraints: const BoxConstraints(),
                  ),
                  child: Text(
                    _ssid.text,
                    style: TextStyle(fontSize: Sizes.md, color: c.text),
                  ),
                ),
        ),
        const SizedBox(height: 18),
        LabeledField(
          label: 'Password',
          hint: "8 to 63 characters. The feeder keeps it; this phone doesn't.",
          child: TextField(
            controller: _password,
            obscureText: _hidden,
            decoration: InputDecoration(
              suffixIcon: IconButton(
                tooltip: _hidden ? 'Show password' : 'Hide password',
                icon: StrokeIcon(
                  _hidden ? Glyph.eye : Glyph.eyeOff,
                  size: 20,
                  color: c.textMuted,
                ),
                onPressed: () => setState(() => _hidden = !_hidden),
              ),
            ),
            onChanged: (_) => setState(() {}),
          ),
        ),
        if (!_typing && _ssid.text.isNotEmpty) ...[
          const SizedBox(height: Space.s2),
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              style: TextButton.styleFrom(padding: EdgeInsets.zero),
              onPressed: () => setState(() => _typing = true),
              child: const Text('Type a network name instead'),
            ),
          ),
        ],
      ],
      actions: [
        FilledButton(
          onPressed: _valid
              ? () => widget.onDone(
                  WifiCredentials(ssid: _ssid.text, passphrase: _password.text),
                )
              : null,
          child: const Text('Continue'),
        ),
        // A feeder already on the network only needs a new owner.
        TextButton(
          onPressed: () => widget.onDone(null),
          child: const Text("Keep the feeder's Wi-Fi"),
        ),
      ],
    );
  }
}
