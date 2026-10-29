import 'package:flutter/material.dart';

import '../theme/theme.dart';

/// A step screen: a title, body copy, content, and the actions pinned at
/// the bottom.
class StepScaffold extends StatelessWidget {
  const StepScaffold({
    super.key,
    required this.title,
    this.body,
    this.content = const [],
    this.actions = const [],
    this.appBarTitle,
  });

  final String title;
  final String? body;
  final List<Widget> content;
  final List<Widget> actions;
  final String? appBarTitle;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Scaffold(
      appBar: AppBar(title: appBarTitle == null ? null : Text(appBarTitle!)),
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(
                  Space.s4,
                  Space.s2,
                  Space.s4,
                  Space.s4,
                ),
                children: [
                  Text(
                    title,
                    style: TextStyle(
                      fontSize: Sizes.xl,
                      fontWeight: Fonts.semibold,
                      color: c.text,
                      height: 1.2,
                    ),
                  ),
                  if (body != null) ...[
                    const SizedBox(height: Space.s3),
                    Text(
                      body!,
                      style: TextStyle(
                        fontSize: Sizes.md,
                        color: c.textMuted,
                        height: 1.45,
                      ),
                    ),
                  ],
                  const SizedBox(height: Space.s5),
                  ...content,
                ],
              ),
            ),
            if (actions.isNotEmpty)
              Padding(
                padding: const EdgeInsets.all(Space.s4),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final (i, action) in actions.indexed) ...[
                      if (i > 0) const SizedBox(height: Space.s2),
                      action,
                    ],
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// A status line: a dot in a status color, then the words.
class StatusLine extends StatelessWidget {
  const StatusLine(this.text, {super.key, required this.color});

  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) => Row(
    children: [
      Container(
        width: 8,
        height: 8,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      ),
      const SizedBox(width: Space.s2),
      Flexible(
        child: Text(
          text,
          style: TextStyle(
            fontSize: Sizes.base,
            fontWeight: Fonts.medium,
            color: color,
          ),
        ),
      ),
    ],
  );
}
