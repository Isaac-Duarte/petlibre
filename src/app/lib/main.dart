import 'package:flutter/material.dart';

import 'app.dart';
import 'src/rust/frb_generated.dart';
import 'state/app_state.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final app = await AppState.load();
  runApp(WellfedApp(app: app));
}
