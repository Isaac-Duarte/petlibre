import 'package:flutter/material.dart';

import '../../state/feeder.dart';

class DiagnosticsScreen extends StatelessWidget {
  const DiagnosticsScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => Scaffold(appBar: AppBar());
}
