import 'package:flutter/material.dart';

import 'tokens.g.dart';

export 'tokens.g.dart';

/// Material's theme from the brand tokens: blue is the only signal color,
/// amber only for food, and the status colors only for status.
ThemeData wellfedTheme(Brightness brightness) {
  final t = brightness == Brightness.dark ? darkTokens : lightTokens;
  final scheme = ColorScheme(
    brightness: brightness,
    primary: t.accent,
    onPrimary: t.accentText,
    primaryContainer: t.accentSoft,
    onPrimaryContainer: t.text,
    secondary: t.accent,
    onSecondary: t.accentText,
    error: t.fault,
    onError: t.accentText,
    errorContainer: t.faultSoft,
    onErrorContainer: t.text,
    surface: t.surface,
    onSurface: t.text,
    onSurfaceVariant: t.textMuted,
    surfaceContainerLowest: t.background,
    surfaceContainerLow: t.background,
    surfaceContainer: t.surface,
    surfaceContainerHigh: t.surface,
    surfaceContainerHighest: t.sunken,
    outline: t.lineStrong,
    outlineVariant: t.line,
    scrim: t.scrim,
  );
  final controlShape = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(Radii.control),
  );
  const buttonSize = Size(64, 48);

  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    brightness: brightness,
    fontFamily: Fonts.family,
    scaffoldBackgroundColor: t.background,
    extensions: [WellfedColors(t)],
    appBarTheme: AppBarTheme(
      backgroundColor: t.background,
      foregroundColor: t.text,
      elevation: 0,
      scrolledUnderElevation: 0,
      titleTextStyle: TextStyle(
        fontFamily: Fonts.family,
        fontSize: Sizes.lg,
        fontWeight: Fonts.semibold,
        color: t.text,
      ),
    ),
    cardTheme: CardThemeData(
      color: t.surface,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(Radii.card),
        side: BorderSide(color: t.line),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: buttonSize,
        shape: controlShape,
        textStyle: const TextStyle(
          fontSize: Sizes.md,
          fontWeight: Fonts.semibold,
        ),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: buttonSize,
        shape: controlShape,
        side: BorderSide(color: t.line),
        foregroundColor: t.text,
        textStyle: const TextStyle(
          fontSize: Sizes.md,
          fontWeight: Fonts.semibold,
        ),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(minimumSize: const Size(48, 48)),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: t.surface,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(Radii.control),
        borderSide: BorderSide(color: t.line),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(Radii.control),
        borderSide: BorderSide(color: t.line),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(Radii.control),
        borderSide: BorderSide(color: t.focus, width: 2),
      ),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: t.surface,
      showDragHandle: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(Radii.card)),
      ),
    ),
    dividerTheme: DividerThemeData(color: t.line, space: 1),
  );
}

/// The roles Material has no slot for (food, ok, warning), reachable as
/// `context.colors`.
class WellfedColors extends ThemeExtension<WellfedColors> {
  const WellfedColors(this.tokens);

  final ThemeTokens tokens;

  @override
  WellfedColors copyWith({ThemeTokens? tokens}) =>
      WellfedColors(tokens ?? this.tokens);

  @override
  WellfedColors lerp(WellfedColors? other, double t) =>
      t < 0.5 || other == null ? this : other;
}

extension WellfedContext on BuildContext {
  ThemeTokens get colors => Theme.of(this).extension<WellfedColors>()!.tokens;

  bool get isDark => Theme.of(this).brightness == Brightness.dark;
}
