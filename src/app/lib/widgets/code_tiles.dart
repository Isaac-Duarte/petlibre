import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../theme/theme.dart';

/// The symbols printed on the feeder's four buttons, in panel order from
/// the left (positions 1-4), so the code reads like the panel itself.
const _symbols = [
  // 1 Lock
  '<rect x="5" y="11" width="14" height="10" rx="2"/>'
      '<path d="M8 11V7a4 4 0 0 1 8 0v4M12 15v2"/>',
  // 2 Dispense: a bowl with one portion
  '<ellipse cx="12" cy="12" rx="9" ry="7"/>'
      '<path d="M8 9.5l3.5 5M11.5 9.5l-3.5 5M15.5 9v6"/>',
  // 3 Mic
  '<rect x="9" y="3" width="6" height="11" rx="3"/>'
      '<path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>',
  // 4 Reset
  '<path d="M19 9a8 8 0 0 0-14-2M5 3v4h4M5 15a8 8 0 0 0 14 2M19 21v-4h-4"/>',
];

const buttonNames = ['Lock', 'Dispense', 'Mic', 'Reset'];

/// The symbol of the button at panel `position` (1-4).
class PanelSymbol extends StatelessWidget {
  const PanelSymbol(this.position, {super.key, this.size = 28, this.color});

  final int position;
  final double size;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final svg =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" '
        'fill="none" stroke="#000" stroke-width="1.8" '
        'stroke-linecap="round" stroke-linejoin="round">'
        '${_symbols[position - 1]}</svg>';
    return SvgPicture.string(
      svg,
      width: size,
      height: size,
      colorFilter: ColorFilter.mode(
        color ?? context.colors.text,
        BlendMode.srcIn,
      ),
      semanticsLabel: buttonNames[position - 1],
    );
  }
}

/// The six presses to key in, each as the button's symbol.
class CodeTiles extends StatelessWidget {
  const CodeTiles(this.code, {super.key});

  final List<int> code;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Semantics(
      label: 'Press ${code.map((p) => buttonNames[p - 1]).join(', ')}',
      child: ExcludeSemantics(
        child: Row(
          children: [
            for (final (i, position) in code.indexed) ...[
              if (i > 0) const SizedBox(width: Space.s2),
              Expanded(
                child: Container(
                  height: 84,
                  decoration: BoxDecoration(
                    color: c.surface,
                    borderRadius: BorderRadius.circular(Radii.icon),
                    border: Border.all(color: c.line),
                  ),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      PanelSymbol(position, size: 30),
                      const SizedBox(height: 6),
                      Text(
                        buttonNames[position - 1],
                        maxLines: 1,
                        overflow: TextOverflow.fade,
                        softWrap: false,
                        style: TextStyle(
                          fontSize: 11,
                          fontWeight: Fonts.medium,
                          color: c.textMuted,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// The feeder's ring as it looks: two buttons, four lights, two buttons,
/// so the tiles' symbols can be found on it.
class RingLegend extends StatelessWidget {
  const RingLegend({super.key});

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    Widget light() => Container(
      width: 6,
      height: 6,
      decoration: BoxDecoration(color: c.lineStrong, shape: BoxShape.circle),
    );
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 14),
      decoration: BoxDecoration(
        color: c.sunken,
        borderRadius: BorderRadius.circular(Radii.card),
      ),
      child: Column(
        children: [
          Container(
            height: 54,
            padding: const EdgeInsets.symmetric(horizontal: 6),
            decoration: BoxDecoration(
              color: c.surface,
              borderRadius: BorderRadius.circular(16),
              border: Border.all(color: c.line),
            ),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                const PanelSymbol(1, size: 22),
                const PanelSymbol(2, size: 22),
                for (var i = 0; i < 4; i++) light(),
                const PanelSymbol(3, size: 22),
                const PanelSymbol(4, size: 22),
              ],
            ),
          ),
          const SizedBox(height: 10),
          Text(
            "The symbols are the ones printed on the feeder's ring. The "
            'four dots between them are lights, not buttons.',
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: Sizes.xs,
              height: 1.5,
              color: c.textMuted,
            ),
          ),
        ],
      ),
    );
  }
}

/// The panel render, cropped to its buttons, with a ring on each button
/// to hold. Offsets are on the design's 364 px wide render.
class PanelPhoto extends StatelessWidget {
  const PanelPhoto({super.key, required this.held, this.caption});

  /// Panel positions (1-4) to ring, in the order they are held.
  final List<int> held;

  /// One caption under the rings; without it each ring is labelled.
  final String? caption;

  static const _width = 364.0;
  static const _height = 190.0;
  static const _top = -88.0;

  /// Each button's ring on the uncropped render.
  static const _rings = {
    1: Offset(12, 157),
    2: Offset(50, 163),
    3: Offset(270, 164),
    4: Offset(308, 157),
  };

  @override
  Widget build(BuildContext context) {
    // The photo is always dark, so its marks use the dark theme's roles.
    final ring = darkTokens.accent;
    final label = TextStyle(
      color: darkTokens.text,
      fontSize: Sizes.xs,
      fontWeight: Fonts.semibold,
      shadows: const [Shadow(blurRadius: 3)],
    );
    final names = held.map((p) => buttonNames[p - 1]).join(', then ');

    return Semantics(
      image: true,
      label: "The feeder's button panel: hold $names",
      child: ExcludeSemantics(
        child: LayoutBuilder(
          builder: (context, constraints) {
            final scale = constraints.maxWidth / _width;
            Offset at(int position, double dy) {
              final ring = _rings[position]!;
              return Offset(ring.dx * scale, (ring.dy + _top + dy) * scale);
            }

            return ClipRRect(
              borderRadius: BorderRadius.circular(Radii.card),
              child: Container(
                height: _height * scale,
                color: lightTokens.scrim,
                child: Stack(
                  clipBehavior: Clip.hardEdge,
                  children: [
                    Positioned(
                      left: 0,
                      top: _top * scale,
                      width: _width * scale,
                      height: _width * scale,
                      child: Image.asset('assets/renders/panel.png'),
                    ),
                    for (final position in held)
                      Positioned(
                        left: at(position, 0).dx,
                        top: at(position, 0).dy,
                        child: Container(
                          width: 44 * scale,
                          height: 44 * scale,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            border: Border.all(color: ring, width: 3),
                          ),
                        ),
                      ),
                    if (caption != null)
                      Positioned(
                        left: at(held.first, 53).dx,
                        top: at(held.first, 53).dy,
                        child: Text(caption!, style: label),
                      )
                    else
                      for (final position in held)
                        Positioned(
                          left: at(position, 53).dx,
                          top: at(position, 53).dy,
                          child: Text(buttonNames[position - 1], style: label),
                        ),
                  ],
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}
