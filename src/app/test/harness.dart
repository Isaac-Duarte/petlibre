import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/src/rust/api/store.dart';
import 'package:wellfed/state/feeder.dart';

import 'fakes.dart';

const kitchen = KnownFeeder(
  identity: 'aa',
  name: 'Kitchen',
  address: 'lan:4747',
);

/// One feeder's controller, its session opened.
Future<(FeederController, FakeSession)> oneFeeder(WidgetTester tester) async {
  final book = FakeBook()..feeders = [kitchen];
  final ports = FakeFeederPorts();
  final feeder = FeederController(book, ports, kitchen);
  addTearDown(feeder.dispose);
  await tester.pump();
  ports.session.opened();
  await tester.pump();
  return (feeder, ports.session);
}

/// `tab` on a phone-sized frame, as the boards draw it.
Future<void> showTab(WidgetTester tester, Widget tab) async {
  tester.view.physicalSize = const Size(412, 915);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(themed(Scaffold(body: tab)));
  await settle(tester);
}

/// Tap and let the fake's answer come back.
Future<void> tapIn(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await tester.pump();
  await tester.tap(target);
  await settle(tester);
}
