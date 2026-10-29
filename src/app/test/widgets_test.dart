import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/screens/home.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/theme/theme.dart';
import 'package:wellfed/widgets/code_tiles.dart';
import 'package:wellfed/widgets/layout.dart';

Widget _themed(Widget child, {Brightness brightness = Brightness.light}) =>
    MaterialApp(
      theme: wellfedTheme(brightness),
      home: Scaffold(body: child),
    );

void main() {
  testWidgets('the code shows each press as its panel button, in order', (
    tester,
  ) async {
    await tester.pumpWidget(_themed(const CodeTiles([1, 2, 3, 4, 2, 1])));

    expect(find.byType(PanelSymbol), findsNWidgets(6));
    final names = tester
        .widgetList<Text>(find.byType(Text))
        .map((text) => text.data)
        .toList();
    expect(names, ['Lock', 'Dispense', 'Mic', 'Reset', 'Dispense', 'Lock']);
    expect(
      find.bySemanticsLabel('Press Lock, Dispense, Mic, Reset, Dispense, Lock'),
      findsOneWidget,
    );
  });

  testWidgets('both themes take their roles from the brand tokens', (
    tester,
  ) async {
    for (final (brightness, tokens) in [
      (Brightness.light, lightTokens),
      (Brightness.dark, darkTokens),
    ]) {
      late ThemeTokens seen;
      await tester.pumpWidget(
        _themed(
          Builder(
            builder: (context) {
              seen = context.colors;
              return const SizedBox();
            },
          ),
          brightness: brightness,
        ),
      );
      // MaterialApp animates from the previous theme.
      await tester.pumpAndSettle();
      expect(seen, same(tokens));
      final theme = wellfedTheme(brightness);
      expect(theme.colorScheme.primary, tokens.accent);
      expect(theme.scaffoldBackgroundColor, tokens.background);
    }
  });

  testWidgets('a step screen keeps its actions below the content', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: wellfedTheme(Brightness.light),
        home: StepScaffold(
          title: 'Title',
          body: 'Body',
          content: const [Text('Content')],
          actions: [FilledButton(onPressed: () {}, child: const Text('Go'))],
        ),
      ),
    );

    final content = tester.getTopLeft(find.text('Content'));
    final action = tester.getTopLeft(find.text('Go'));
    expect(action.dy, greaterThan(content.dy));
  });

  test('every reason a feed cannot start has words', () {
    for (final reason in Blocked.values) {
      expect(blockedWords(reason), isNotEmpty);
    }
  });
}
