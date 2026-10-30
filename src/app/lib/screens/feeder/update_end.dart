import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../src/rust/api/update.dart';
import '../../widgets/layout.dart';

/// What an update's end comes to: a title and what it means (12.3).
(String, String) updateEndWords(String name, UpdateEnd end) => switch (end) {
  UpdateEnd_Confirmed() => (
    'Updated',
    '$name runs the new version and has kept it.',
  ),
  UpdateEnd_AlreadyRunning() => (
    'Nothing to do',
    '$name already runs this version. Nothing was sent.',
  ),
  UpdateEnd_Refused(:final refusal) => _refused(name, refusal),
  UpdateEnd_Stopped(stop: UpdateStop.cancelled) => (
    'Update cancelled',
    '$name still runs its current version.',
  ),
  UpdateEnd_Stopped(stop: UpdateStop.hash || UpdateStop.marker) => (
    "Signature didn't check out",
    'The image may be damaged. Nothing was installed.',
  ),
  UpdateEnd_Stopped() || UpdateEnd_Lost() => (
    'Stopped partway',
    "It didn't finish. $name still runs its current version.",
  ),
  UpdateEnd_Uncertain() => (
    '$name stopped answering',
    "The update went out, so it may or may not have installed. Wellfed won't "
        'send it again. Look at the firmware once the feeder is back.',
  ),
  UpdateEnd_NotOffered() => (
    "Couldn't reach $name",
    'Nothing was sent, and nothing changed.',
  ),
  UpdateEnd_Unconfirmed() => (
    'Not confirmed yet',
    "$name took the update, but hasn't confirmed the new version. If it "
        "doesn't start properly, it goes back by itself.",
  ),
  UpdateEnd_WentBack() => (
    '$name went back',
    "The new version didn't start properly, so it returned to the one it ran "
        'before.',
  ),
  UpdateEnd_Failed(:final detail) => (
    "This phone couldn't send it",
    'Nothing was installed. $detail',
  ),
};

(String, String) _refused(
  String name,
  FeederRefusal refusal,
) => switch (refusal) {
  FeederRefusal_Update(reason: UpdateRefusal.battery) => (
    'Plug $name in first',
    "It's running on batteries, and it won't install an update unless it's on "
        'mains power. Nothing changed.',
  ),
  FeederRefusal_Update(reason: UpdateRefusal.downgrade) => (
    'Older than what it runs',
    "$name won't go back to an earlier version.",
  ),
  FeederRefusal_Update(
    reason: UpdateRefusal.manifest ||
        UpdateRefusal.key ||
        UpdateRefusal.revoked ||
        UpdateRefusal.signature ||
        UpdateRefusal.statement ||
        UpdateRefusal.target,
  ) =>
    (
      "Signature didn't check out",
      'The image may be damaged, or it was not signed for this feeder. Nothing '
          'was installed.',
    ),
  FeederRefusal_Update(reason: UpdateRefusal.same) => (
    'Nothing to do',
    '$name already runs this version.',
  ),
  FeederRefusal_Update(reason: UpdateRefusal.failed) => (
    'This version failed on $name',
    'It was tried before and did not start properly, so it will not try again.',
  ),
  FeederRefusal_Update(reason: UpdateRefusal.attempt) => (
    'Restart $name first',
    'An install already reset it since it was last powered up. Nothing changed.',
  ),
  FeederRefusal_Update(
    reason: UpdateRefusal.installing || UpdateRefusal.pending,
  ) =>
    (
      'Another update is under way',
      'Let it finish, then look again. Nothing changed.',
    ),
  FeederRefusal_Update() => (
    '$name said no',
    "It can't install an update right now. Nothing changed.",
  ),
  FeederRefusal_Busy() => ("It's feeding", 'Try again when the feed finishes.'),
  FeederRefusal_Forbidden() => (
    'Owner only',
    'Only an owner can update $name. Nothing changed.',
  ),
  _ => ('$name said no', 'Nothing changed.'),
};

/// Where an update ended, and only one that never reached the feeder offers
/// another try.
class UpdateEndScreen extends StatelessWidget {
  const UpdateEndScreen({
    super.key,
    required this.name,
    required this.end,
    this.onTryAgain,
  });

  final String name;
  final UpdateEnd end;
  final VoidCallback? onTryAgain;

  @override
  Widget build(BuildContext context) {
    final (title, body) = updateEndWords(name, end);
    return StepScaffold(
      title: title,
      body: body,
      actions: [
        if (onTryAgain case final tryAgain?)
          FilledButton(onPressed: tryAgain, child: const Text('Try again')),
        OutlinedButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Done'),
        ),
      ],
    );
  }
}
