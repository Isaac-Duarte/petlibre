import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../theme/theme.dart';
import '../widgets/layout.dart';

/// The ways in. Only the routes that are built are listed; installing on a
/// stock feeder, invites and servers appear as they land.
class AddFeederScreen extends StatelessWidget {
  const AddFeederScreen({super.key});

  @override
  Widget build(BuildContext context) => StepScaffold(
    title: 'Add a feeder',
    body: 'Wellfed firmware is already on the feeder. Pick how to reach it.',
    content: [
      _Route(
        title: 'Set up a feeder',
        body:
            'A new or reset feeder. You hold two buttons, the phone gives '
            'it your Wi-Fi over Bluetooth, and you key in a code.',
        onTap: () => context.push('/setup'),
      ),
      const SizedBox(height: Space.s3),
      _Route(
        title: 'Become its owner again',
        body:
            'The feeder is already on your Wi-Fi but this phone is not '
            'paired. You hold two buttons and key in a code.',
        onTap: () => context.push('/recover'),
      ),
    ],
  );
}

class _Route extends StatelessWidget {
  const _Route({required this.title, required this.body, required this.onTap});

  final String title;
  final String body;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Card(
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(Space.s4),
          child: Row(
            children: [
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
                        color: c.textMuted,
                        height: 1.4,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s3),
              Icon(Icons.chevron_right, color: c.textMuted),
            ],
          ),
        ),
      ),
    );
  }
}
