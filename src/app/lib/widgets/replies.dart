import 'package:flutter/material.dart';

import '../src/rust/api/feeder.dart';

/// Why a request came to nothing, or may have: `null` once it worked. An
/// uncertain write says so, and nothing sends it again.
String? replyProblem(FeederReply reply) => switch (reply) {
  FeederReply_Refused(:final refusal) => refusalWords(refusal),
  FeederReply_NotAsked(:final reason) => notAskedWords(reason),
  FeederReply_Unanswered() =>
    "No answer came, so it may or may not have gone through. Wellfed won't "
        'send it again.',
  FeederReply_NotSent() =>
    'The connection dropped before this went out. Nothing changed.',
  FeederReply_Unexpected() => 'The feeder answered something unexpected.',
  _ => null,
};

String refusalWords(FeederRefusal refusal) => switch (refusal) {
  FeederRefusal_Forbidden() => "This phone isn't allowed to do that here.",
  FeederRefusal_Stale() =>
    'This changed just now. Look again and decide again.',
  FeederRefusal_Busy() => "It's busy. Try again in a moment.",
  FeederRefusal_Unavailable() => "This feeder's firmware doesn't do that yet.",
  FeederRefusal_Invalid() => 'The feeder refused that value.',
  FeederRefusal_NotWritable() => "It can't save changes right now.",
  FeederRefusal_Full() => 'Every slot is taken.',
  FeederRefusal_NotFound() => "It doesn't have that any more.",
  FeederRefusal_Muted() => 'Sound is off, or outside its hours.',
  FeederRefusal_Time() => 'The feeder disagreed with this clock.',
  FeederRefusal_Fault() => 'It stopped on a fault and needs a restart.',
  FeederRefusal_Update() => 'The feeder refused the update.',
  FeederRefusal_Unverified() => 'This feeder has not verified that mode.',
  FeederRefusal_Portions(:final asked) =>
    'A feed is 1 to 48 portions, not $asked.',
  FeederRefusal_NoClock() =>
    "The feeder doesn't know the date. Set its time first.",
};

String notAskedWords(NotAsked reason) => switch (reason) {
  NotAsked.credentials =>
    'A network name is 1 to 32 bytes, and its password 8 to 63 plain '
        'characters.',
  NotAsked.emptySlot => "That meal isn't there any more.",
  NotAsked.plansFull => 'A feeder keeps 10 meals, and this one has 10.',
  NotAsked.nothingToChange => 'Nothing changed.',
  NotAsked.badPlan => 'A meal is 1 to 48 portions, with up to 5 calls.',
  NotAsked.badWeight => 'Give the weight in grams, like 9.8.',
  NotAsked.badNtp => 'Give an IPv4 address, like 192.168.1.1.',
  NotAsked.badZone => "The feeder can't follow that time zone.",
  NotAsked.badRecording => 'Use a WAV recording of about 8 seconds at most.',
  NotAsked.badKey => "That isn't a client's key.",
};

/// Say why `reply` came to nothing, if it did; whether it worked.
bool showProblem(BuildContext context, FeederReply reply) {
  final problem = replyProblem(reply);
  if (problem == null) return true;
  ScaffoldMessenger.of(context)
    ..hideCurrentSnackBar()
    ..showSnackBar(SnackBar(content: Text(problem)));
  return false;
}
