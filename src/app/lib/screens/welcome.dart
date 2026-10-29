import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import '../theme/theme.dart';

class WelcomeScreen extends StatelessWidget {
  const WelcomeScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(Space.s4),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Spacer(),
              Image.asset(
                context.isDark
                    ? 'assets/renders/card-dark-cutout.png'
                    : 'assets/renders/card-cutout.png',
                height: 280,
                semanticLabel: 'A feeder',
              ),
              const SizedBox(height: Space.s6),
              Text(
                'Your feeder, on your network',
                style: TextStyle(
                  fontSize: Sizes.xxl,
                  fontWeight: Fonts.display,
                  color: c.text,
                  height: 1.1,
                ),
              ),
              const SizedBox(height: Space.s3),
              Text(
                'Wellfed talks to your feeder directly over Wi-Fi. '
                'No account, no cloud.',
                style: TextStyle(
                  fontSize: Sizes.md,
                  color: c.textMuted,
                  height: 1.45,
                ),
              ),
              const Spacer(),
              FilledButton(
                onPressed: () => context.push('/add'),
                child: const Text('Add a feeder'),
              ),
              const SizedBox(height: Space.s4),
              Text(
                'An independent project, not affiliated with PETLIBRO.',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: Sizes.xs, color: c.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
