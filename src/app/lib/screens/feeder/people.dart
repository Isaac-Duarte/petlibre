import 'package:flutter/material.dart';

import '../../state/feeder.dart';

class PeopleScreen extends StatelessWidget {
  const PeopleScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  Widget build(BuildContext context) => Scaffold(appBar: AppBar());
}

const clientsMost = 8;
