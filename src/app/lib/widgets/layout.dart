import 'dart:async';

import 'package:flutter/material.dart';

import '../theme/theme.dart';
import 'icons.dart';

/// What the header's leading button does: step back, or leave the flow.
enum Leading { back, close, none }

/// A step screen: a header (back or close, then a step label or a title),
/// an optional step progress bar, a title, body copy, content, and the
/// actions pinned at the bottom.
class StepScaffold extends StatelessWidget {
  const StepScaffold({
    super.key,
    this.title,
    this.body,
    this.content = const [],
    this.actions = const [],
    this.appBarTitle,
    this.stepLabel,
    this.progress,
    this.leading = Leading.back,
    this.onLeading,
    this.top,
  });

  final String? title;
  final String? body;
  final List<Widget> content;
  final List<Widget> actions;

  /// A screen title in the header (20 px), as on list-like screens.
  final String? appBarTitle;

  /// "Step 1 of 4" in the header, muted.
  final String? stepLabel;

  /// The flow's progress, 0 to 1, drawn under the header.
  final double? progress;
  final Leading leading;

  /// What the leading button does; popping the route by default.
  final VoidCallback? onLeading;

  /// Above the title, such as a status line or an icon.
  final Widget? top;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final canPop = Navigator.canPop(context);
    final showLeading =
        leading != Leading.none && (onLeading != null || canPop);
    final header = appBarTitle ?? stepLabel;

    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: showLeading ? 4 : Space.s5,
        leading: showLeading
            ? IconButton(
                tooltip: leading == Leading.close ? 'Close' : 'Back',
                onPressed: onLeading ?? () => Navigator.pop(context),
                icon: StrokeIcon(
                  leading == Leading.close ? Glyph.close : Glyph.back,
                  color: c.text,
                ),
              )
            : null,
        title: header == null
            ? null
            : Text(
                header,
                style: appBarTitle != null
                    ? null
                    : TextStyle(
                        fontSize: Sizes.base,
                        fontWeight: Fonts.medium,
                        color: c.textMuted,
                      ),
              ),
      ),
      body: SafeArea(
        top: false,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (progress != null)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: Space.s5),
                child: Bar(value: progress!, height: 4),
              ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(
                  Space.s5,
                  20,
                  Space.s5,
                  Space.s5,
                ),
                children: [
                  if (top != null) ...[top!, const SizedBox(height: Space.s4)],
                  if (title != null) Headline(title!),
                  if (body != null) ...[
                    const SizedBox(height: Space.s3),
                    Text(body!, style: bodyStyle(context)),
                  ],
                  if (title != null || body != null) const SizedBox(height: 20),
                  ...content,
                ],
              ),
            ),
            if (actions.isNotEmpty)
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  Space.s5,
                  0,
                  Space.s5,
                  Space.s5,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final (i, action) in actions.indexed) ...[
                      if (i > 0) const SizedBox(height: Space.s3),
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

/// Body copy: 15 px, muted.
TextStyle bodyStyle(BuildContext context) =>
    TextStyle(fontSize: 15, color: context.colors.textMuted, height: 1.5);

/// A screen's headline: 28 px extra bold, tight.
class Headline extends StatelessWidget {
  const Headline(
    this.text, {
    super.key,
    this.size = Sizes.xl,
    this.align = TextAlign.start,
  });

  final String text;
  final double size;
  final TextAlign align;

  @override
  Widget build(BuildContext context) => Text(
    text,
    textAlign: align,
    style: TextStyle(
      fontSize: size,
      fontWeight: Fonts.display,
      letterSpacing: -0.02 * size,
      height: 1.15,
      color: context.colors.text,
    ),
  );
}

/// A rounded track filled to `value` (0 to 1).
class Bar extends StatelessWidget {
  const Bar({super.key, required this.value, this.height = 6, this.color});

  final double value;
  final double height;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return ClipRRect(
      borderRadius: BorderRadius.circular(Radii.pill),
      child: LinearProgressIndicator(
        value: value.clamp(0, 1),
        minHeight: height,
        backgroundColor: c.sunken,
        color: color ?? c.accent,
        borderRadius: BorderRadius.circular(Radii.pill),
      ),
    );
  }
}

/// A small spinner beside muted words, for "Searching nearby…".
class Searching extends StatelessWidget {
  const Searching(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Row(
      children: [
        SizedBox.square(
          dimension: 16,
          child: CircularProgressIndicator(
            strokeWidth: 2,
            color: c.accent,
            backgroundColor: c.sunken,
          ),
        ),
        const SizedBox(width: 10),
        Flexible(
          child: Text(
            text,
            style: TextStyle(fontSize: Sizes.base, color: c.textMuted),
          ),
        ),
      ],
    );
  }
}

/// `text` with each `**…**` run in bold, in the plain text color.
List<TextSpan> emphasized(String text, Color strong) => [
  for (final (i, part) in text.split('**').indexed)
    TextSpan(
      text: part,
      style: i.isOdd
          ? TextStyle(fontWeight: Fonts.semibold, color: strong)
          : null,
    ),
];

/// Numbered instructions, the numbers in the accent color. Each step may
/// mark button names `**Lock**`.
class NumberedSteps extends StatelessWidget {
  const NumberedSteps(this.steps, {super.key});

  final List<String> steps;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final style = TextStyle(fontSize: 15, height: 1.5, color: c.text);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final (i, step) in steps.indexed) ...[
          if (i > 0) const SizedBox(height: 14),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 18,
                child: Text(
                  '${i + 1}',
                  style: style.copyWith(
                    fontWeight: Fonts.display,
                    color: c.accent,
                  ),
                ),
              ),
              const SizedBox(width: Space.s3),
              Expanded(
                child: Text.rich(
                  TextSpan(style: style, children: emphasized(step, c.text)),
                ),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

/// A status: a dot (or a glyph) in a status color, then the words.
class StatusChip extends StatelessWidget {
  const StatusChip(this.text, {super.key, required this.color, this.glyph});

  final String text;
  final Color color;
  final Glyph? glyph;

  @override
  Widget build(BuildContext context) => Row(
    mainAxisSize: MainAxisSize.min,
    children: [
      if (glyph != null)
        StrokeIcon(glyph!, size: 14, color: color, weight: 2)
      else
        Container(
          width: 6,
          height: 6,
          decoration: BoxDecoration(color: color, shape: BoxShape.circle),
        ),
      const SizedBox(width: 6),
      Flexible(
        child: Text(
          text,
          style: TextStyle(
            fontSize: Sizes.sm,
            fontWeight: Fonts.semibold,
            color: color,
          ),
        ),
      ),
    ],
  );
}

/// The food dot: amber when there is food, an amber ring when low, and
/// muted when the hopper has not been read.
class FoodDot extends StatelessWidget {
  const FoodDot({super.key, required this.present});

  final bool? present;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Container(
      width: 8,
      height: 8,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: present == true ? c.food : null,
        border: present == true
            ? null
            : Border.all(
                color: present == false ? c.food : c.lineStrong,
                width: 2,
              ),
      ),
    );
  }
}

enum Tone { warning, fault, neutral }

/// A tinted box: a bold lead sentence, then the rest, and an optional
/// action under it.
class Notice extends StatelessWidget {
  const Notice({
    super.key,
    required this.tone,
    this.lead,
    required this.text,
    this.glyph,
    this.radius = Radii.icon,
  });

  final Tone tone;
  final String? lead;
  final String text;
  final Glyph? glyph;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final background = switch (tone) {
      Tone.warning => c.warningSoft,
      Tone.fault => c.faultSoft,
      Tone.neutral => c.sunken,
    };
    final words = tone == Tone.neutral ? c.textMuted : c.text;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: background,
        borderRadius: BorderRadius.circular(radius),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (glyph != null) ...[
            StrokeIcon(glyph!, size: 18, color: c.accent),
            const SizedBox(width: 10),
          ],
          Expanded(
            child: Text.rich(
              TextSpan(
                style: TextStyle(
                  fontSize: Sizes.base,
                  height: 1.45,
                  color: words,
                ),
                children: [
                  if (lead != null)
                    TextSpan(
                      text: '$lead ',
                      style: TextStyle(
                        fontWeight: Fonts.semibold,
                        color: c.text,
                      ),
                    ),
                  ...emphasized(text, c.text),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A white card with the design's 20 px corners and hairline border.
class Panel extends StatelessWidget {
  const Panel({
    super.key,
    required this.child,
    this.padding = EdgeInsets.zero,
    this.onTap,
    this.highlight = false,
  });

  final Widget child;
  final EdgeInsetsGeometry padding;
  final VoidCallback? onTap;

  /// An accent border, for the suggested choice.
  final bool highlight;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final shape = RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(Radii.card),
      side: highlight
          ? BorderSide(color: c.accent, width: 2)
          : BorderSide(color: c.line),
    );
    return Material(
      color: c.surface,
      shape: shape,
      clipBehavior: Clip.antiAlias,
      child: onTap == null
          ? Padding(padding: padding, child: child)
          : InkWell(
              onTap: onTap,
              child: Padding(padding: padding, child: child),
            ),
    );
  }
}

/// One way in: an icon tile, a title and what it is for.
class RouteCard extends StatelessWidget {
  const RouteCard({
    super.key,
    required this.glyph,
    required this.title,
    required this.body,
    required this.onTap,
  });

  final Glyph glyph;
  final String title;
  final String body;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Panel(
      onTap: onTap,
      padding: const EdgeInsets.all(Space.s4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          IconTile(glyph),
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
                    height: 1.45,
                    color: c.textMuted,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A glyph on a soft accent square.
class IconTile extends StatelessWidget {
  const IconTile(this.glyph, {super.key, this.size = 48, this.muted = false});

  final Glyph glyph;
  final double size;
  final bool muted;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: muted ? c.sunken : c.accentSoft,
        borderRadius: BorderRadius.circular(Radii.icon),
      ),
      alignment: Alignment.center,
      child: StrokeIcon(
        glyph,
        size: size / 2,
        color: muted ? c.textMuted : c.accent,
      ),
    );
  }
}

/// Label-value rows with hairlines between, on a card or sunk into one.
class FactList extends StatelessWidget {
  const FactList(this.rows, {super.key, this.sunken = false});

  final List<(String, Widget)> rows;
  final bool sunken;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Container(
      decoration: BoxDecoration(
        color: sunken ? c.sunken : c.surface,
        borderRadius: BorderRadius.circular(sunken ? Radii.icon : Radii.card),
        border: sunken ? null : Border.all(color: c.line),
      ),
      child: Column(
        children: [
          for (final (i, (label, value)) in rows.indexed)
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
              decoration: BoxDecoration(
                border: i == 0 ? null : Border(top: BorderSide(color: c.line)),
              ),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      label,
                      style: TextStyle(
                        fontSize: Sizes.base,
                        color: c.textMuted,
                      ),
                    ),
                  ),
                  DefaultTextStyle.merge(
                    style: TextStyle(fontSize: Sizes.base, color: c.text),
                    child: value,
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Titled rows on a card: "Other reasons setup stops".
class ReasonList extends StatelessWidget {
  const ReasonList(this.rows, {super.key});

  final List<(String, String)> rows;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Panel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final (i, (title, body)) in rows.indexed)
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
              decoration: BoxDecoration(
                border: i == 0 ? null : Border(top: BorderSide(color: c.line)),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: TextStyle(
                      fontSize: 15,
                      fontWeight: Fonts.semibold,
                      color: c.text,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    body,
                    style: TextStyle(
                      fontSize: Sizes.base,
                      height: 1.45,
                      color: c.textMuted,
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// An uppercase section label.
class SectionLabel extends StatelessWidget {
  const SectionLabel(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text.toUpperCase(),
    style: TextStyle(
      fontSize: Sizes.sm,
      fontWeight: Fonts.semibold,
      letterSpacing: 0.06 * Sizes.sm,
      color: context.colors.textMuted,
    ),
  );
}

/// A form field with its label above and a hint (and counter) below.
class LabeledField extends StatelessWidget {
  const LabeledField({
    super.key,
    required this.label,
    required this.child,
    this.hint,
    this.counter,
  });

  final String label;
  final Widget child;
  final String? hint;
  final String? counter;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final small = TextStyle(fontSize: Sizes.sm, color: c.textMuted);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(label, style: small.copyWith(fontWeight: Fonts.semibold)),
        const SizedBox(height: 6),
        child,
        if (hint != null || counter != null) ...[
          const SizedBox(height: 6),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(child: Text(hint ?? '', style: small)),
              if (counter != null)
                Text(
                  counter!,
                  style: small.copyWith(
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
            ],
          ),
        ],
      ],
    );
  }
}

/// The feeder render for the current theme.
class FeederImage extends StatelessWidget {
  const FeederImage({super.key, required this.size, this.dimmed = false});

  final double size;
  final bool dimmed;

  @override
  Widget build(BuildContext context) => Opacity(
    opacity: dimmed ? 0.5 : 1,
    child: Image.asset(
      context.isDark
          ? 'assets/renders/card-dark-cutout.png'
          : 'assets/renders/card-cutout.png',
      width: size,
      height: size,
      fit: BoxFit.contain,
      excludeFromSemantics: true,
    ),
  );
}

/// What a run waits for, once it has waited [after] with the same `words` and
/// the same `progress`: no screen stays still for long with nothing said.
class WaitWords extends StatefulWidget {
  const WaitWords(this.words, {super.key, this.progress});

  /// Longer than any step that is just working.
  static const after = Duration(seconds: 30);

  /// `null` when the screen already says what it waits for.
  final String? words;

  /// What, when it moves, shows the run is not stuck.
  final Object? progress;

  @override
  State<WaitWords> createState() => _WaitWordsState();
}

class _WaitWordsState extends State<WaitWords> {
  Timer? _timer;
  bool _long = false;

  @override
  void initState() {
    super.initState();
    _watch();
  }

  @override
  void didUpdateWidget(WaitWords old) {
    super.didUpdateWidget(old);
    if (old.words != widget.words || old.progress != widget.progress) _watch();
  }

  void _watch() {
    _long = false;
    _timer?.cancel();
    _timer = Timer(WaitWords.after, () => setState(() => _long = true));
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final words = widget.words;
    if (!_long || words == null) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: Space.s3),
      child: Notice(tone: Tone.neutral, text: words),
    );
  }
}
