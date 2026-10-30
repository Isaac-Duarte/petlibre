import 'package:flutter/material.dart';

import '../../theme/theme.dart';
import '../../widgets/icons.dart';

/// What the feeder's lights and buttons mean (7.2): a key over the panel
/// photo, in the firmware's own meanings.
class LightsHelp extends StatelessWidget {
  const LightsHelp({super.key});

  /// The photo's crop, on the board's 410-wide frame.
  static const _width = 410.0;
  static const _height = 170.0;
  static const _top = -126.0;

  /// Each mark over the photo, on that frame.
  static const _marks = {
    'A': Offset(25, 44),
    'B': Offset(68, 44),
    '1': Offset(141, 46),
    '2': Offset(181, 46),
    '3': Offset(221, 46),
    '4': Offset(261, 46),
    'C': Offset(325, 50),
    'D': Offset(367, 44),
  };

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Back',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.back, color: c.text),
        ),
        title: Text(
          'Lights and buttons',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListView(
        children: [
          _photo(context),
          Padding(
            padding: const EdgeInsets.all(Space.s4),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: const [
                _Heading('Lights'),
                _Key('1', 'Food.', 'Blinks while it feeds.'),
                _Key(
                  '2',
                  'Lock.',
                  'On: buttons locked. Fast blink: waiting for a pairing '
                      'code.',
                ),
                _Key(
                  '3',
                  'Wi-Fi.',
                  'On: connected. Slow blink: Bluetooth setup. Fast blink: '
                      'installing an update.',
                ),
                _Key(
                  '4',
                  'Warning.',
                  'On: a fault stopped feeding until a restart. Quick blink: '
                      'a press was refused. Slow blink: batteries too low to '
                      'feed.',
                ),
                SizedBox(height: Space.s4),
                _Heading('Buttons'),
                _Key(
                  'A',
                  'Lock.',
                  'Hold 5 s to lock or unlock the buttons. They relock after '
                      '30 s.',
                ),
                _Key('B', 'Bowl.', 'Press for one portion.'),
                _Key(
                  'C',
                  'Mic.',
                  'Hold with Lock for 5 s to start Bluetooth setup.',
                ),
                _Key(
                  'D',
                  'Reset.',
                  'Hold 5 s to erase people, Wi-Fi, meals and settings. '
                      'Feeding history stays.',
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _photo(BuildContext context) => Semantics(
    image: true,
    label: "The feeder's control ring, with its lights and buttons marked",
    child: ExcludeSemantics(
      child: LayoutBuilder(
        builder: (context, constraints) {
          final scale = constraints.maxWidth / _width;
          // The photo is always dark, so its marks use the dark theme's.
          final mark = TextStyle(
            fontSize: 12,
            fontWeight: Fonts.display,
            color: darkTokens.text,
            shadows: const [Shadow(blurRadius: 3, offset: Offset(0, 1))],
          );
          return SizedBox(
            height: _height * scale,
            child: ClipRect(
              child: ColoredBox(
                color: darkTokens.background,
                child: Stack(
                  children: [
                    Positioned(
                      left: 0,
                      top: _top * scale,
                      width: constraints.maxWidth,
                      height: constraints.maxWidth,
                      child: Image.asset('assets/renders/panel.png'),
                    ),
                    for (final MapEntry(key: name, value: at) in _marks.entries)
                      Positioned(
                        left: at.dx * scale,
                        top: at.dy * scale,
                        child: Text(name, style: mark),
                      ),
                  ],
                ),
              ),
            ),
          );
        },
      ),
    ),
  );
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: Space.s2),
    child: Text(
      text,
      style: TextStyle(
        fontSize: Sizes.sm,
        fontWeight: Fonts.semibold,
        color: context.colors.textMuted,
      ),
    ),
  );
}

class _Key extends StatelessWidget {
  const _Key(this.mark, this.name, this.meaning);

  final String mark;
  final String name;
  final String meaning;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 22,
            child: Text(
              mark,
              style: TextStyle(
                fontSize: Sizes.base,
                height: 1.5,
                fontWeight: Fonts.semibold,
                color: c.text,
              ),
            ),
          ),
          Expanded(
            child: Text.rich(
              TextSpan(
                style: TextStyle(
                  fontSize: Sizes.base,
                  height: 1.5,
                  color: c.text,
                ),
                children: [
                  TextSpan(
                    text: '$name ',
                    style: const TextStyle(fontWeight: Fonts.semibold),
                  ),
                  TextSpan(text: meaning),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
