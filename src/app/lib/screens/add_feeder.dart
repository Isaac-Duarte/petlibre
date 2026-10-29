import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../theme/theme.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';

/// The ways in. Only the routes that are built are listed; installing on a
/// stock feeder, invites and servers appear as they land.
class AddFeederScreen extends StatelessWidget {
  const AddFeederScreen({super.key});

  @override
  Widget build(BuildContext context) => StepScaffold(
    appBarTitle: 'Add a feeder',
    body: 'How is this feeder getting to you?',
    content: [
      RouteCard(
        glyph: Glyph.bluetooth,
        title: 'Set up a reset feeder',
        body:
            'It already runs Wellfed but was reset, or can\'t reach your '
            'Wi-Fi. Set up over Bluetooth.',
        onTap: () => context.push('/setup'),
      ),
      const SizedBox(height: Space.s3),
      RouteCard(
        glyph: Glyph.key,
        title: 'Recover a feeder',
        body:
            'It is on your Wi-Fi, but no phone can open it any more. Add '
            'this phone as an owner without erasing anything.',
        onTap: () => context.push('/recover'),
      ),
    ],
  );
}
