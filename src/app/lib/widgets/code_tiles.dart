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
                child: AspectRatio(
                  aspectRatio: 0.82,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: c.surface,
                      borderRadius: BorderRadius.circular(Radii.control),
                      border: Border.all(color: c.line),
                    ),
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        PanelSymbol(position, size: 30),
                        const SizedBox(height: Space.s1),
                        Text(
                          buttonNames[position - 1],
                          style: TextStyle(fontSize: 10, color: c.textMuted),
                        ),
                      ],
                    ),
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
