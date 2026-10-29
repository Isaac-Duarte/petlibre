import 'package:flutter/material.dart';
import 'package:go_router/go_router.dart';

import 'screens/add_feeder.dart';
import 'screens/home.dart';
import 'screens/recover_screen.dart';
import 'screens/setup_screen.dart';
import 'screens/welcome.dart';
import 'state/app_state.dart';
import 'theme/theme.dart';

class WellfedApp extends StatefulWidget {
  const WellfedApp({super.key, required this.app});

  final AppState app;

  @override
  State<WellfedApp> createState() => _WellfedAppState();
}

class _WellfedAppState extends State<WellfedApp> {
  late final GoRouter _router = GoRouter(
    refreshListenable: widget.app,
    routes: [
      GoRoute(
        path: '/',
        builder: (context, _) => widget.app.feeders.isEmpty
            ? const WelcomeScreen()
            : HomeScreen(app: widget.app),
      ),
      GoRoute(path: '/add', builder: (context, _) => const AddFeederScreen()),
      GoRoute(
        path: '/setup',
        builder: (context, _) => SetupScreen(app: widget.app),
      ),
      GoRoute(
        path: '/recover',
        builder: (context, _) => RecoverScreen(app: widget.app),
      ),
    ],
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
