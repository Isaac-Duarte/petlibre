import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import 'platform/install_ports.dart';
import 'platform/pairing_ports.dart';
import 'screens/add_feeder.dart';
import 'screens/home.dart';
import 'screens/install_screen.dart';
import 'screens/recover_screen.dart';
import 'screens/setup_screen.dart';
import 'screens/welcome.dart';
import 'state/app_state.dart';
import 'theme/theme.dart';

/// The app's routes. `root` is one route whatever the feeders: it follows
/// `book` by itself, because refreshing the router when a feeder is saved
/// would rebuild the stack under a flow still showing its success screen.
GoRouter wellfedRouter({
  required Listenable book,
  required WidgetBuilder root,
  required Map<String, WidgetBuilder> pages,
}) => GoRouter(
  routes: [
    GoRoute(
      path: '/',
      builder: (context, _) => ListenableBuilder(
        listenable: book,
        builder: (context, _) => root(context),
      ),
    ),
    for (final MapEntry(key: path, value: page) in pages.entries)
      GoRoute(path: path, builder: (context, _) => page(context)),
  ],
);

class WellfedApp extends StatefulWidget {
  const WellfedApp({super.key, required this.app});

  final AppState app;

  @override
  State<WellfedApp> createState() => _WellfedAppState();
}

class _WellfedAppState extends State<WellfedApp> {
  late final _pairing = AppPairingPorts(widget.app.key);

  late final GoRouter _router = wellfedRouter(
    book: widget.app,
    root: (context) => widget.app.feeders.isEmpty
        ? const WelcomeScreen()
        : HomeScreen(
            app: widget.app,
            earlierInstall: () async {
              final ports = AppInstallPorts(widget.app.key);
              return ports.pending(await ports.paths());
            },
          ),
    pages: {
      '/add': (context) => const AddFeederScreen(),
      '/setup': (context) => SetupScreen(app: widget.app, ports: _pairing),
      '/recover': (context) => RecoverScreen(app: widget.app, ports: _pairing),
      '/install': (context) => InstallScreen(
        book: widget.app,
        ports: AppInstallPorts(widget.app.key),
      ),
    },
  );

  @override
  void dispose() {
    _router.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialApp.router(
    title: 'Wellfed',
    debugShowCheckedModeBanner: false,
    theme: wellfedTheme(Brightness.light),
    darkTheme: wellfedTheme(Brightness.dark),
    routerConfig: _router,
  );
}
