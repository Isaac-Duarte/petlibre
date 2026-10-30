import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/replies.dart';

/// What one feed's drive measured (13.2).
class MotorTraceScreen extends StatelessWidget {
  const MotorTraceScreen({super.key, required this.feeder, required this.feed});

  final FeederController feeder;
  final int feed;

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      automaticallyImplyLeading: false,
      leading: IconButton(
        tooltip: 'Back',
        onPressed: () => Navigator.pop(context),
        icon: StrokeIcon(Glyph.back, color: context.colors.text),
      ),
      title: Text('Feed $feed'),
    ),
    body: FutureBuilder<FeederReply>(
      future: feeder.ask(FeederRequest.readTrace(feed: feed)),
      builder: (context, snapshot) {
        final reply = snapshot.data;
        return reply == null
            ? const Center(child: CircularProgressIndicator())
            : Center(child: Text(replyProblem(reply) ?? 'Read.'));
      },
    ),
  );
}
