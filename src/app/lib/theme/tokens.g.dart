// Generated from assets/brand/tokens.json by
// src/app/rust/tests/tokens.rs; do not edit.

import 'package:flutter/widgets.dart';

/// A theme's color roles.
class ThemeTokens {
  const ThemeTokens({
    required this.accent,
    required this.accentSoft,
    required this.accentText,
    required this.background,
    required this.fault,
    required this.faultSoft,
    required this.focus,
    required this.food,
    required this.info,
    required this.infoSoft,
    required this.line,
    required this.lineStrong,
    required this.ok,
    required this.okSoft,
    required this.scrim,
    required this.sunken,
    required this.surface,
    required this.text,
    required this.textMuted,
    required this.warning,
    required this.warningSoft,
  });

  final Color accent;
  final Color accentSoft;
  final Color accentText;
  final Color background;
  final Color fault;
  final Color faultSoft;
  final Color focus;
  final Color food;
  final Color info;
  final Color infoSoft;
  final Color line;
  final Color lineStrong;
  final Color ok;
  final Color okSoft;
  final Color scrim;
  final Color sunken;
  final Color surface;
  final Color text;
  final Color textMuted;
  final Color warning;
  final Color warningSoft;
}

const ThemeTokens lightTokens = ThemeTokens(
  accent: Color(0xFF2563EB),
  accentSoft: Color(0xFFEDF2FF),
  accentText: Color(0xFFFFFFFF),
  background: Color(0xFFF4F6FA),
  fault: Color(0xFFB91C1C),
  faultSoft: Color(0xFFFDE4E4),
  focus: Color(0xFF2563EB),
  food: Color(0xFFE0A13A),
  info: Color(0xFF2563EB),
  infoSoft: Color(0xFFEDF2FF),
  line: Color(0xFFDCE2EC),
  lineStrong: Color(0xFF8390A6),
  ok: Color(0xFF15803D),
  okSoft: Color(0xFFE7F7EC),
  scrim: Color(0xFF0B1424),
  sunken: Color(0xFFE9EDF4),
  surface: Color(0xFFFFFFFF),
  text: Color(0xFF0B1424),
  textMuted: Color(0xFF5A6780),
  warning: Color(0xFF9A5B00),
  warningSoft: Color(0xFFFCF0D6),
);

const ThemeTokens darkTokens = ThemeTokens(
  accent: Color(0xFF5A96F7),
  accentSoft: Color(0xFF1D2A40),
  accentText: Color(0xFF121417),
  background: Color(0xFF121417),
  fault: Color(0xFFF87171),
  faultSoft: Color(0xFF3A2022),
  focus: Color(0xFF5A96F7),
  food: Color(0xFFE0A13A),
  info: Color(0xFF7FB0FF),
  infoSoft: Color(0xFF1D2A40),
  line: Color(0xFF2E3238),
  lineStrong: Color(0xFF676E79),
  ok: Color(0xFF4ADE80),
  okSoft: Color(0xFF18301F),
  scrim: Color(0xFF0B1424),
  sunken: Color(0xFF17191D),
  surface: Color(0xFF1C1F24),
  text: Color(0xFFECEEF1),
  textMuted: Color(0xFFA6ACB5),
  warning: Color(0xFFFBBF24),
  warningSoft: Color(0xFF352C17),
);

abstract final class Fonts {
  static const family = 'Inter';
  static const mono = 'JetBrains Mono';
  static const body = FontWeight.w400;
  static const display = FontWeight.w800;
  static const medium = FontWeight.w500;
  static const semibold = FontWeight.w600;
}

abstract final class Sizes {
  static const double base = 14;
  static const double lg = 20;
  static const double md = 16;
  static const double sm = 13;
  static const double xl = 28;
  static const double xs = 12;
  static const double xxl = 40;
}

abstract final class Space {
  static const double s1 = 4;
  static const double s2 = 8;
  static const double s3 = 12;
  static const double s4 = 16;
  static const double s5 = 24;
  static const double s6 = 32;
  static const double s7 = 48;
}

abstract final class Radii {
  static const double card = 20;
  static const double control = 12;
  static const double icon = 14;
  static const double pill = 999;
}

abstract final class Motion {
  static const base = Duration(milliseconds: 200);
  static const fast = Duration(milliseconds: 120);
}
