import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';

import '../theme/theme.dart';

/// The design's stroke icons, on a 24-unit grid. Drawn from the same
/// paths as the design so the app and the canvas show the same glyphs.
enum Glyph {
  back('<path d="M19 12H5M11 18l-6-6 6-6"/>'),
  close('<path d="M6 6l12 12M18 6L6 18"/>'),
  add('<path d="M12 5v14M5 12h14"/>'),
  minus('<path d="M5 12h14"/>'),
  check('<path d="M5 12.5l4.5 4.5L19 7"/>'),
  chevron('<path d="M9 6l6 6-6 6"/>'),
  info('<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5h.01"/>'),
  clock('<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>'),
  battery(
    '<rect x="2" y="7" width="18" height="10" rx="2"/><path d="M22 11v2"/>',
  ),
  wifi(
    '<path d="M2 8.5a15 15 0 0 1 20 0M5.5 12a10 10 0 0 1 13 0'
    'M9 15.5a5 5 0 0 1 6 0M12 19h.01"/>',
  ),
  wifiOff(
    '<path d="M2 8.5a15 15 0 0 1 20 0M5.5 12a10 10 0 0 1 13 0'
    'M9 15.5a5 5 0 0 1 6 0M12 19h.01M3 3l18 18"/>',
  ),
  bluetooth('<path d="M7 7l10 10-5 5V2l5 5L7 17"/>'),
  key('<circle cx="8" cy="15" r="4"/><path d="M11 12l9-9M16 7l3 3"/>'),
  eye(
    '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/>'
    '<circle cx="12" cy="12" r="3"/>',
  ),
  eyeOff(
    '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/>'
    '<circle cx="12" cy="12" r="3"/><path d="M3 3l18 18"/>',
  ),
  failed('<circle cx="12" cy="12" r="9"/><path d="M9 9l6 6M15 9l-6 6"/>'),
  warning('<path d="M12 3L2 21h20L12 3zM12 10v5M12 18h.01"/>'),
  install('<path d="M12 3v12M7 10l5 5 5-5M4 20h16"/>');

  const Glyph(this.paths);

  final String paths;
}

class StrokeIcon extends StatelessWidget {
  const StrokeIcon(
    this.glyph, {
    super.key,
    this.size = 24,
    this.color,
    this.weight = 1.8,
  });

  final Glyph glyph;
  final double size;
  final Color? color;
  final double weight;

  @override
  Widget build(BuildContext context) => SvgPicture.string(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" '
    'fill="none" stroke="#000" stroke-width="$weight" '
    'stroke-linecap="round" stroke-linejoin="round">${glyph.paths}</svg>',
    width: size,
    height: size,
    colorFilter: ColorFilter.mode(
      color ?? IconTheme.of(context).color ?? context.colors.text,
      BlendMode.srcIn,
    ),
    excludeFromSemantics: true,
  );
}
