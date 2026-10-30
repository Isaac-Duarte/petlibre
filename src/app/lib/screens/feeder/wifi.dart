import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';

/// Change the feeder's Wi-Fi (10.4). The credentials are stored for its next
/// join, and it rejoins only on a restart, so saving them is followed by one.
/// Each goes out once. A save whose answer never came is not followed by
/// anything: the owner looks first. The feeder never reports the network's
/// name, so this says only how it is doing.
class WifiScreen extends StatefulWidget {
  const WifiScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<WifiScreen> createState() => _WifiScreenState();
}

class _WifiScreenState extends State<WifiScreen> {
  final _network = TextEditingController();
  final _password = TextEditingController();
  bool _asking = false;

  @override
  void dispose() {
    _network.dispose();
    _password.dispose();
    super.dispose();
  }

  Future<void> _switch() async {
    final feeder = widget.feeder;
    final base = feeder.card?.wifiGeneration;
    if (base == null) return;

    setState(() => _asking = true);
    final saved = await feeder.ask(
      FeederRequest.putWifi(
        base: base,
        ssid: _network.text,
        passphrase: _password.text,
      ),
    );
    if (!mounted) return;
    if (!showProblem(context, saved)) {
      setState(() => _asking = false);
      return;
    }

    final restart = await feeder.ask(const FeederRequest.reboot());
    if (!mounted) return;
    setState(() => _asking = false);
    final problem = replyProblem(restart);
    final messenger = ScaffoldMessenger.of(context);
    if (problem != null) {
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            'The network is saved, but the restart: $problem Restart it to '
            'join.',
          ),
        ),
      );
      return;
    }
    messenger.showSnackBar(
      SnackBar(content: Text('${feeder.known.name} is restarting to join.')),
    );
    Navigator.pop(context);
  }

  /// Forget the network, once. It stays on the network it has until a restart,
  /// and after one only Bluetooth setup brings it back.
  Future<void> _forget() async {
    final feeder = widget.feeder;
    final base = feeder.card?.wifiGeneration;
    if (base == null) return;

    final sure = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Forget this network?'),
        content: const Text(
          'After its next restart the feeder stays off Wi-Fi until you set it '
          'up again over Bluetooth. Meals keep running either way.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Forget'),
          ),
        ],
      ),
    );
    if (sure != true || !mounted) return;

    setState(() => _asking = true);
    final reply = await feeder.ask(FeederRequest.forgetWifi(base: base));
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final feeder = widget.feeder;
    final rssi = feeder.card?.rssi;
    final signal = switch (rssi) {
      null => null,
      >= -60 => 'a strong signal',
      >= -72 => 'a fair signal',
      _ => 'a weak signal',
    };
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
          'Wi-Fi',
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
          if (signal != null)
            Text(
              '${feeder.known.name} is online with $signal.',
              style: TextStyle(fontSize: 15, height: 1.5, color: c.text),
            ),
          const SizedBox(height: Space.s4),
          TextField(
            controller: _network,
            autocorrect: false,
            decoration: const InputDecoration(labelText: 'New network'),
          ),
          const SizedBox(height: Space.s3),
          TextField(
            controller: _password,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            decoration: const InputDecoration(labelText: 'Password'),
          ),
          const SizedBox(height: Space.s4),
          const Notice(
            tone: Tone.neutral,
            glyph: Glyph.info,
            text:
                "The feeder restarts to join. If it can't, set it up again "
                'over Bluetooth: hold Lock and Mic for 5 seconds. Meals keep '
                'running either way.',
          ),
          const SizedBox(height: Space.s4),
          FilledButton(
            onPressed: _asking ? null : _switch,
            child: const Text('Switch network'),
          ),
          const SizedBox(height: Space.s2),
          TextButton(
            onPressed: _asking ? null : _forget,
            child: const Text('Forget this network'),
          ),
        ],
      ),
    );
  }
}
