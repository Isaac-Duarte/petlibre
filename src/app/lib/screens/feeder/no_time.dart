import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';

/// The feeder has no clock, so its meals wait: say so, and offer this phone's.
class NoTimeNotice extends StatefulWidget {
  const NoTimeNotice({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<NoTimeNotice> createState() => _NoTimeNoticeState();
}

class _NoTimeNoticeState extends State<NoTimeNotice> {
  bool _asking = false;

  Future<void> _sync() async {
    setState(() => _asking = true);
    final reply = await widget.feeder.ask(const FeederRequest.syncClock());
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      const Notice(
        tone: Tone.warning,
        lead: 'Meals are on hold.',
        text:
            "The feeder doesn't know the time yet, so it won't guess when to "
            'feed. Feed now still works.',
      ),
      const SizedBox(height: Space.s2),
      OutlinedButton(
        onPressed: _asking ? null : _sync,
        child: const Text('Set the time from this phone'),
      ),
    ],
  );
}
