import 'package:flutter/material.dart';

import '../src/rust/api/session.dart';

/// A moment as this phone shows the time of day.
String clockText(BuildContext context, DateTime at) =>
    _text(context, TimeOfDay.fromDateTime(at));

/// A moment in milliseconds since the epoch, by this phone's clock.
String clockTextMs(BuildContext context, int ms) =>
    clockText(context, DateTime.fromMillisecondsSinceEpoch(ms));

/// A minute of the feeder's local day, as a plan keeps it.
String minuteText(BuildContext context, int minute) =>
    _text(context, TimeOfDay(hour: minute ~/ 60 % 24, minute: minute % 60));

/// `in 26 min`, `in 2 h 5 min`, or `now` once it is due.
String untilText(DateTime now, int atMs) {
  final minutes = (atMs - now.millisecondsSinceEpoch) ~/ 60000;
  if (minutes < 1) return 'now';
  if (minutes < 60) return 'in $minutes min';
  final rest = minutes % 60;
  return rest == 0
      ? 'in ${minutes ~/ 60} h'
      : 'in ${minutes ~/ 60} h $rest min';
}

String portionsText(int n) => n == 1 ? '1 portion' : '$n portions';

String _text(BuildContext context, TimeOfDay time) =>
    MaterialLocalizations.of(context).formatTimeOfDay(
      time,
      alwaysUse24HourFormat: MediaQuery.alwaysUse24HourFormatOf(context),
    );

/// `2 of 2`, or a range for an uncertain feed, never one number.
String deliveredText(FeedResult fed) => fed.uncertain
    ? '${fed.confirmed} to ${fed.atMost} of ${fed.requested}'
    : '${fed.confirmed} of ${fed.requested}';
