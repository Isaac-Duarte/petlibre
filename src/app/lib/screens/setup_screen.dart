import 'dart:async';

import 'package:flutter/material.dart';
import 'package:network_info_plus/network_info_plus.dart';
import 'package:permission_handler/permission_handler.dart';

import '../src/rust/api/setup.dart';
import '../state/app_state.dart';
import '../state/pairing.dart';
import '../theme/theme.dart';
import '../widgets/layout.dart';
import 'name_step.dart';
import 'pairing_view.dart';

enum _Step { setupMode, name, wifi, pairing }

/// BLE setup: put the feeder in setup mode, name this phone, give the
/// feeder its Wi-Fi, then pair with the code.
class SetupScreen extends StatefulWidget {
  const SetupScreen({super.key, required this.app});

  final AppState app;

  @override
  State<SetupScreen> createState() => _SetupScreenState();
}

class _SetupScreenState extends State<SetupScreen> {
  _Step _step = _Step.setupMode;
  WifiCredentials? _wifi;
  PairingFlow? _flow;

  @override
  void dispose() {
    _flow?.dispose();
    super.dispose();
  }

  Future<void> _pair() async {
    final granted = await [
      Permission.bluetoothScan,
      Permission.bluetoothConnect,
    ].request();
    if (!mounted) return;
    if (granted.values.any((status) => !status.isGranted)) {
      await showDialog<void>(
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
      return;
    }

    _flow?.dispose();
    final flow = PairingFlow(widget.app);
    setState(() {
      _flow = flow;
      _step = _Step.pairing;
    });
    await flow.runSetup(wifi: _wifi);
  }

  @override
  Widget build(BuildContext context) => switch (_step) {
    _Step.setupMode => StepScaffold(
      appBarTitle: 'Set up a feeder',
      title: 'Put the feeder in setup mode',
      body:
          'Plug the feeder into mains and make sure the panel is unlocked. '
          'Hold Lock, then Mic, together for 5 seconds.',
      content: [
        Image.asset(
          'assets/renders/panel.png',
          height: 220,
          semanticLabel: 'The feeder panel: Lock, Dispense, Mic and Reset',
        ),
      ],
      actions: [
        FilledButton(
          onPressed: () => setState(() => _step = _Step.name),
          child: const Text('It is in setup mode'),
        ),
      ],
    ),
    _Step.name => NameStep(
      initial: widget.app.label,
      onDone: (label) async {
        await widget.app.setLabel(label);
        setState(() => _step = _Step.wifi);
      },
    ),
    _Step.wifi => _WifiStep(
      onDone: (wifi) {
        _wifi = wifi;
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

/// The network the feeder joins after setup. The phone's current network
/// is filled in when location is allowed; Android needs it to read the
/// name, and nothing else uses it.
class _WifiStep extends StatefulWidget {
  const _WifiStep({required this.onDone});

  final ValueChanged<WifiCredentials?> onDone;

  @override
  State<_WifiStep> createState() => _WifiStepState();
}

class _WifiStepState extends State<_WifiStep> {
  final _ssid = TextEditingController();
  final _password = TextEditingController();
  bool _keep = false;
  bool _hidden = true;

  @override
  void initState() {
    super.initState();
    unawaited(_prefill());
  }

  Future<void> _prefill() async {
    final status = await Permission.locationWhenInUse.request();
    if (!status.isGranted) return;
    final name = await NetworkInfo().getWifiName();
    if (!mounted || name == null || _ssid.text.isNotEmpty) return;
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
      _keep || (_ssid.text.isNotEmpty && _password.text.length >= 8);

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return StepScaffold(
      title: 'Which Wi-Fi should it join?',
      body:
          'The feeder joins this network after setup. Keep your phone on '
          'it too, so the app can find the feeder.',
      content: [
        TextField(
          controller: _ssid,
          enabled: !_keep,
          decoration: const InputDecoration(labelText: 'Network name'),
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: Space.s3),
        TextField(
          controller: _password,
          enabled: !_keep,
          obscureText: _hidden,
          decoration: InputDecoration(
            labelText: 'Password',
            suffixIcon: IconButton(
              tooltip: _hidden ? 'Show password' : 'Hide password',
              icon: Icon(_hidden ? Icons.visibility : Icons.visibility_off),
              onPressed: () => setState(() => _hidden = !_hidden),
            ),
          ),
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: Space.s3),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          value: _keep,
          onChanged: (keep) => setState(() => _keep = keep),
          title: Text(
            "Keep the feeder's Wi-Fi",
            style: TextStyle(color: c.text, fontSize: Sizes.base),
          ),
          subtitle: Text(
            'For a feeder already on your network.',
            style: TextStyle(color: c.textMuted, fontSize: Sizes.sm),
          ),
        ),
      ],
      actions: [
        FilledButton(
          onPressed: _valid
              ? () => widget.onDone(
                  _keep
                      ? null
                      : WifiCredentials(
                          ssid: _ssid.text,
                          passphrase: _password.text,
                        ),
                )
              : null,
          child: const Text('Find the feeder'),
        ),
      ],
    );
  }
}
