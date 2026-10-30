import 'package:flutter/material.dart';

import '../../state/feeder.dart';

class UpdatesScreen extends StatelessWidget {
  const UpdatesScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => Scaffold(appBar: AppBar());
}
