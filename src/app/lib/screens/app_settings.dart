import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';

import '../platform/install_ports.dart';
import '../src/rust/api/store.dart';
import '../state/app_state.dart';
import '../theme/theme.dart';
import '../widgets/ask_text.dart';
import '../widgets/icons.dart';
import '../widgets/layout.dart';
import 'name_step.dart';

const sourceUrl = 'https://github.com/Isaac-Duarte/wellfed';
const issuesUrl = '$sourceUrl/issues';

/// The two places in the app's main navigation.
class MainNav extends StatelessWidget {
  const MainNav({super.key, required this.selected});

  /// 0: Feeders, 1: Settings.
  final int selected;

  @override
  Widget build(BuildContext context) => NavigationBar(
    selectedIndex: selected,
    onDestinationSelected: (index) {
      if (index != selected) context.go(index == 0 ? '/' : '/settings');
    },
    destinations: const [
      NavigationDestination(icon: StrokeIcon(Glyph.food), label: 'Feeders'),
      NavigationDestination(
        icon: Icon(Icons.settings_outlined),
        label: 'Settings',
      ),
    ],
  );
}

/// The app's own settings (15.1).
class AppSettingsScreen extends StatefulWidget {
  const AppSettingsScreen({
    super.key,
    required this.book,
    required this.backups,
  });

  final FeederBook book;
  final BackupPorts backups;

  @override
  State<AppSettingsScreen> createState() => _AppSettingsScreenState();
}

class _AppSettingsScreenState extends State<AppSettingsScreen> {
  /// Each feeder with a backup here.
  List<(KnownFeeder, String)> _backups = const [];

  @override
  void initState() {
    super.initState();
    unawaited(_readBackups());
  }

  Future<void> _readBackups() async {
    final kept = <(KnownFeeder, String)>[];
    for (final feeder in widget.book.feeders) {
      if (await widget.backups.kept(feeder.identity) case final path?) {
        kept.add((feeder, path));
      }
    }
    if (mounted) setState(() => _backups = kept);
  }

  Future<void> _rename() async {
    final name = await askText(
      context,
      title: 'Name on feeders',
      label: 'This phone',
      initial: widget.book.label,
    );
    if (name == null || !mounted) return;

    final trimmed = name.trim();
    final bytes = utf8.encode(trimmed).length;
    if (bytes < 1 || bytes > NameStep.maxBytes) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('A phone name is 1 to 16 bytes.')),
      );
      return;
    }
    await widget.book.setLabel(trimmed);
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.book,
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final c = context.colors;
    final book = widget.book;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        titleSpacing: 20,
        automaticallyImplyLeading: false,
        title: Text(
          'Settings',
          style: TextStyle(
            fontSize: 22,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      bottomNavigationBar: const MainNav(selected: 1),
      body: ListView(
        padding: const EdgeInsets.all(Space.s4),
        children: [
          const SectionLabel('Appearance'),
          const SizedBox(height: Space.s2),
          SegmentedButton<Appearance>(
            segments: const [
              ButtonSegment(value: Appearance.system, label: Text('System')),
              ButtonSegment(value: Appearance.light, label: Text('Light')),
              ButtonSegment(value: Appearance.dark, label: Text('Dark')),
            ],
            selected: {book.appearance},
            onSelectionChanged: (chosen) =>
                unawaited(book.setAppearance(chosen.first)),
          ),
          const SizedBox(height: Space.s4),
          const SectionLabel('This phone'),
          const SizedBox(height: Space.s2),
          Panel(
            child: Column(
              children: [
                ListTile(
                  title: const Text('Name on feeders'),
                  subtitle: Text(
                    '${book.label}\nFeeders keep the name they were joined '
                    'with.',
                  ),
                  isThreeLine: true,
                  onTap: () => unawaited(_rename()),
                ),
                ListTile(
                  title: const Text('Firmware backups'),
                  trailing: Text(
                    '${_backups.length}',
                    style: TextStyle(fontSize: 15, color: c.text),
                  ),
                ),
                for (final (feeder, path) in _backups)
                  ListTile(
                    title: Text(feeder.name),
                    trailing: IconButton(
                      tooltip: 'Share ${feeder.name}\'s backup',
                      icon: const Icon(Icons.share_outlined),
                      onPressed: () => unawaited(widget.backups.share(path)),
                    ),
                  ),
                ListTile(
                  title: const Text('About Wellfed'),
                  trailing: StrokeIcon(
                    Glyph.chevron,
                    size: 20,
                    color: c.textMuted,
                  ),
                  onTap: () => unawaited(
                    Navigator.of(context).push(
                      MaterialPageRoute<void>(
                        builder: (_) => const AboutScreen(),
                      ),
                    ),
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

/// What this app is (15.2).
class AboutScreen extends StatelessWidget {
  const AboutScreen({super.key, this.version = appVersion});

  final String Function() version;

  void _copy(BuildContext context, String url) {
    unawaited(Clipboard.setData(ClipboardData(text: url)));
    ScaffoldMessenger.of(context)
        .showSnackBar(const SnackBar(content: Text('Link copied.')));
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final muted = TextStyle(
      fontSize: Sizes.sm,
      height: 1.5,
      color: c.textMuted,
    );
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
          'About',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.s4),
        children: [
          Text(
            'Wellfed',
            style: TextStyle(
              fontSize: 22,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          Text('App ${version()}', style: muted),
          const SizedBox(height: Space.s3),
          Text(
            'Open firmware and an app for the PETLIBRO Granary PLAF103, so it '
            'runs on your own network.',
            style: TextStyle(fontSize: 15, height: 1.5, color: c.text),
          ),
          const SizedBox(height: Space.s4),
          Panel(
            child: Column(
              children: [
                ListTile(
                  title: const Text('Source code'),
                  subtitle: const Text(sourceUrl),
                  onTap: () => _copy(context, sourceUrl),
                ),
                ListTile(
                  title: const Text('Open-source licenses'),
                  onTap: () => showLicensePage(
                    context: context,
                    applicationName: 'Wellfed',
                    applicationVersion: version(),
                  ),
                ),
                ListTile(
                  title: const Text('Report a problem'),
                  subtitle: const Text(issuesUrl),
                  onTap: () => _copy(context, issuesUrl),
                ),
              ],
            ),
          ),
          const SizedBox(height: Space.s4),
          Text(
            'Independent community project. Not affiliated with or endorsed '
            "by PETLIBRO. PETLIBRO and Granary are their owner's trademarks.",
            style: muted,
          ),
        ],
      ),
    );
  }
}
