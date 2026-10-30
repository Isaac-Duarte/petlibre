import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../src/rust/api/update.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';

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
    '$name runs its other copy of Wellfed now. It can feed, and may be older '
        'than the one it ran.',
  ),
  UpdateEnd_Stopped(stop: UpdateStop.hash || UpdateStop.marker) => (
    "Signature didn't check out",
    "The image may be damaged, so it wasn't installed. $name runs its other "
        'copy of Wellfed now, which can feed and may be older.',
  ),
  UpdateEnd_Stopped() || UpdateEnd_Lost() => (
    'Stopped partway',
    "It didn't finish. $name runs its other copy of Wellfed now, which can "
        'feed and may be older than the one it ran.',
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
        "doesn't start properly, it goes to its other copy of Wellfed by itself.",
  ),
  UpdateEnd_WentBack() => (
    '$name is on its other copy',
    "The new version didn't start properly, so it runs its other copy of "
        'Wellfed. That copy can feed, and may be older than the one it ran.',
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
  FeederRefusal_Update(reason: UpdateRefusal.installing) => (
    'Another update is under way',
    'Let it finish, then look again. Nothing changed.',
  ),
  FeederRefusal_Update(reason: UpdateRefusal.pending) => (
    'An earlier update is still held',
    "One stopped partway, and $name won't take another until it's cancelled. "
        'Nothing is installing now.',
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

/// Where an update ended, with what the feeder runs once it answers. An end only
/// the feeder can settle stays until it has: Done leaves the screen, and coming
/// back shows the same end rather than an offer.
class UpdateEndScreen extends StatelessWidget {
  const UpdateEndScreen({super.key, required this.feeder, required this.end});

  final FeederController feeder;
  final UpdateEnd end;

  @override
  Widget build(BuildContext context) {
    final flow = feeder.update;
    final (title, body) = updateEndWords(feeder.known.name, end);
    final card = feeder.card;
    return StepScaffold(
      title: title,
      body: body,
      content: [
        FactList([
          (
            'Running now',
            Flexible(
              child: Text(
                card != null && feeder.cardIsCurrent
                    ? 'Bank ${card.bank}, build ${_short(card.build)}'
                    : 'Shown once it answers',
                textAlign: TextAlign.end,
              ),
            ),
          ),
        ]),
        if (flow.stillUnknown) ...[
          const SizedBox(height: Space.s3),
          const Notice(
            tone: Tone.neutral,
            glyph: Glyph.info,
            text: "It hasn't settled yet, or didn't answer. Look again soon.",
          ),
        ],
      ],
      actions: [
        if (_heldEarlier)
          FilledButton(
            onPressed: flow.cancelling
                ? null
                : () async {
                    final reply = await flow.cancel();
                    if (!context.mounted) return;
                    if (showProblem(context, reply)) flow.acknowledge();
                  },
            child: const Text('Cancel the earlier update'),
          ),
        if (flow.endUnknown)
          FilledButton(
            onPressed: () => unawaited(flow.lookAgain()),
            child: const Text('Look again'),
          ),
        OutlinedButton(
          onPressed: () {
            flow.acknowledge();
            if (flow.endUnknown) Navigator.pop(context);
          },
          child: const Text('Done'),
        ),
      ],
    );
  }

  /// A stop left its candidate recorded, and only a cancel forgets it.
  bool get _heldEarlier => switch (end) {
    UpdateEnd_Refused(
      refusal: FeederRefusal_Update(reason: UpdateRefusal.pending),
    ) =>
      true,
    _ => false,
  };

  static String _short(String build) =>
      build.length > 12 ? build.substring(0, 12) : build;
}
