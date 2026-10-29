import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:go_router/go_router.dart';

import '../theme/theme.dart';
import '../widgets/layout.dart';

class WelcomeScreen extends StatelessWidget {
  const WelcomeScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(28, Space.s7, 28, 28),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  SvgPicture.asset(
                    context.isDark
                        ? 'assets/brand/mark.svg'
                        : 'assets/brand/mark-on-light.svg',
                    width: 36,
                    height: 36,
                    excludeFromSemantics: true,
                  ),
                  const SizedBox(width: 10),
                  Text(
                    'Wellfed',
                    style: TextStyle(
                      fontSize: Sizes.lg,
                      fontWeight: Fonts.display,
                      letterSpacing: -0.4,
                      color: c.text,
                    ),
                  ),
                ],
              ),
              const Expanded(child: Center(child: FeederImage(size: 300))),
              const Headline(
                "Your pet's dinner shouldn't need the internet.",
                size: 36,
              ),
              const SizedBox(height: Space.s3),
              Text(
                'Wellfed talks to your feeder over your own Wi-Fi. No '
                'account, no vendor cloud.',
                style: TextStyle(
                  fontSize: Sizes.md,
                  height: 1.5,
                  color: c.textMuted,
                ),
              ),
              const SizedBox(height: Space.s5),
              FilledButton(
                onPressed: () => context.push('/add'),
                child: const Text('Add a feeder'),
              ),
              const SizedBox(height: Space.s3),
              Text(
                'Independent community project. Not affiliated with or '
                'endorsed by PETLIBRO.',
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: Sizes.xs,
                  height: 1.5,
                  color: c.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
