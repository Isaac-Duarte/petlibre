import 'dart:typed_data';

import 'package:flutter/material.dart' hide Card;
import 'package:flutter_test/flutter_test.dart';
import 'package:wellfed/platform/join_ports.dart';
import 'package:wellfed/screens/feeder/invite.dart';
import 'package:wellfed/screens/feeder/people.dart';
import 'package:wellfed/screens/join.dart';
import 'package:wellfed/src/rust/api/error.dart';
import 'package:wellfed/src/rust/api/feeder.dart';
import 'package:wellfed/src/rust/api/join.dart';
import 'package:wellfed/src/rust/api/model.dart';
import 'package:wellfed/state/feeder.dart';

import 'fakes.dart';
import 'harness.dart';

ClientView _client(int id, String label, ClientRole role, {bool you = false}) =>
    ClientView(
      key: Uint8List.fromList([id]),
      fingerprint: '0$id',
      label: label,
      role: role,
      you: you,
    );

Future<(FeederController, FakeSession)> _people(WidgetTester tester) async {
  final (feeder, session) = await oneFeeder(tester);
  session.status(fakeCard());
  session.replies.add(
    FeederReply.clients(
      clients: [
        _client(1, "Sam's Pixel", ClientRole.owner, you: true),
        _client(2, "Ana's phone", ClientRole.member),
      ],
    ),
  );
  await showTab(tester, PeopleScreen(feeder: feeder));
  session.requests.clear();
  return (feeder, session);
}

class _FakeJoin implements JoinPorts {
  final joins = <String>[];
  final answers = <Object>[];
  String? found = 'lan:4747';

  @override
  String? inviteFeeder(String text) =>
      text.startsWith('wellfed-invite:') ? 'aa' : null;

  @override
  Future<JoinAnswer> join(String invite, String label, String address) async {
    joins.add(invite);
    final answer = answers.removeAt(0);
    if (answer is BridgeError) throw answer;
    return answer as JoinAnswer;
  }

  @override
  Future<String?> locate(String identity, Duration wait) async => found;
}

void main() {
  group('People', () {
    testWidgets('lists who the feeder lists, and this phone stays', (
      tester,
    ) async {
      await _people(tester);

      expect(find.text("Sam's Pixel (this phone)"), findsOneWidget);
      expect(find.text('Member'), findsOneWidget);
      expect(find.byTooltip("Remove Ana's phone"), findsOneWidget);
      expect(find.byTooltip("Remove Sam's Pixel"), findsNothing);
    });

    testWidgets('removing confirms, then revokes the key once', (tester) async {
      final (_, session) = await _people(tester);

      await tapIn(tester, find.byTooltip("Remove Ana's phone"));
      await tapIn(tester, find.widgetWithText(FilledButton, 'Remove'));

      final revoke = session.requests.first as FeederRequest_RevokeClient;
      expect(revoke.key, [2]);
      expect(session.requests.last, const FeederRequest.listClients());
    });

    testWidgets('a refused or unanswered removal says so', (tester) async {
      final (_, session) = await _people(tester);
      session.replies.add(
        const FeederReply.refused(refusal: FeederRefusal.forbidden()),
      );
      await tapIn(tester, find.byTooltip("Remove Ana's phone"));
      await tapIn(tester, find.widgetWithText(FilledButton, 'Remove'));
      expect(find.textContaining("isn't allowed"), findsOneWidget);

      session.requests.clear();
      session.replies.add(const FeederReply.unanswered());
      await tester.pump(const Duration(seconds: 5));
      await tapIn(tester, find.byTooltip("Remove Ana's phone"));
      await tapIn(tester, find.widgetWithText(FilledButton, 'Remove'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(
        session.requests.whereType<FeederRequest_RevokeClient>(),
        hasLength(1),
      );
    });

    testWidgets('an invite is created once for the role chosen', (
      tester,
    ) async {
      final (_, session) = await _people(tester);
      session.replies.add(
        const FeederReply.invite(
          invite: InviteView(
            text: 'wellfed-invite:1:x',
            seconds: 600,
            role: ClientRole.owner,
          ),
        ),
      );

      await tapIn(tester, find.text('Invite someone'));
      expect(find.text('Invite someone to Kitchen'), findsOneWidget);
      await tapIn(
        tester,
        find.widgetWithText(RadioListTile<ClientRole>, 'Owner'),
      );
      await tester.tap(find.text('Create invite'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pump(const Duration(milliseconds: 500));

      expect(
        session.requests.single,
        const FeederRequest.createInvite(role: ClientRole.owner),
      );
      expect(find.text('Owner invite'), findsOneWidget);
      expect(find.text('9:59 until it expires · works once'), findsOneWidget);
    });

    testWidgets('a create refused or unanswered shows no invite', (
      tester,
    ) async {
      final (_, session) = await _people(tester);
      session.replies
        ..add(const FeederReply.refused(refusal: FeederRefusal.full()))
        ..add(const FeederReply.unanswered());

      await tapIn(tester, find.text('Invite someone'));
      await tapIn(tester, find.text('Create invite'));
      expect(find.textContaining('Every slot'), findsOneWidget);

      await tester.pump(const Duration(seconds: 5));
      await tapIn(tester, find.text('Invite someone'));
      await tapIn(tester, find.text('Create invite'));
      expect(find.textContaining('may or may not have'), findsOneWidget);
      expect(find.text('Member invite'), findsNothing);
    });

    testWidgets('an invite runs out, and shares only its text', (tester) async {
      final shared = <String>[];
      await showTab(
        tester,
        InviteScreen(
          invite: const InviteView(
            text: 'wellfed-invite:1:x',
            seconds: 2,
            role: ClientRole.member,
          ),
          share: (text) async => shared.add(text),
        ),
      );

      await tester.tap(find.text('Share link instead'));
      expect(shared, ['wellfed-invite:1:x']);

      await tester.pump(const Duration(seconds: 3));
      expect(find.textContaining('has expired'), findsOneWidget);
    });
  });

  group('Joining', () {
    Future<(_FakeJoin, FakeBook, void Function(String))> join(
      WidgetTester tester,
    ) async {
      final ports = _FakeJoin();
      final book = FakeBook()..label = 'My phone';
      late void Function(String) seen;
      await showTab(
        tester,
        JoinScreen(
          book: book,
          ports: ports,
          scanner: (found) {
            seen = found;
            return const SizedBox();
          },
        ),
      );
      return (ports, book, seen);
    }

    testWidgets('a code that is no invite stays on the scanner', (
      tester,
    ) async {
      final (_, _, seen) = await join(tester);

      seen('https://example.com');
      await settle(tester);
      expect(find.textContaining("isn't a Wellfed invite"), findsOneWidget);
    });

    testWidgets('an invite joins, and the phone keeps the feeder', (
      tester,
    ) async {
      final (ports, book, seen) = await join(tester);
      ports.answers.add(const JoinAnswer.joined(role: ClientRole.member));

      seen('wellfed-invite:1:x');
      await settle(tester);
      expect(find.text('Found'), findsOneWidget);
      await tester.enterText(find.byType(TextField).first, 'Kitchen');
      await settle(tester);
      await tapIn(tester, find.text('Join Kitchen'));

      expect(ports.joins, ['wellfed-invite:1:x']);
      expect(find.text('You can use Kitchen'), findsOneWidget);
      expect(book.feeders.single.identity, 'aa');
      expect(book.feeders.single.name, 'Kitchen');
    });

    testWidgets('a feeder not found cannot be joined yet', (tester) async {
      final (ports, _, seen) = await join(tester);
      ports.found = null;

      seen('wellfed-invite:1:x');
      await settle(tester);
      expect(find.text('Not found'), findsOneWidget);
      final button = tester.widget<FilledButton>(find.byType(FilledButton));
      expect(button.onPressed, isNull);
    });

    testWidgets('a refusal says so, and the invite is not sent again', (
      tester,
    ) async {
      final (ports, book, seen) = await join(tester);
      ports.answers.add(
        const JoinAnswer.refused(refusal: FeederRefusal.full()),
      );

      seen('wellfed-invite:1:x');
      await settle(tester);
      await tapIn(tester, find.text('Join Feeder'));

      expect(find.textContaining('Every slot'), findsOneWidget);
      expect(book.feeders, isEmpty);
      final button = tester.widget<FilledButton>(find.byType(FilledButton));
      expect(button.onPressed, isNull);
    });

    testWidgets('no answer is uncertain, and the invite is not sent again', (
      tester,
    ) async {
      final (ports, book, seen) = await join(tester);
      ports.answers.add(
        const BridgeError(kind: ErrorKind.timeout, message: 'late'),
      );

      seen('wellfed-invite:1:x');
      await settle(tester);
      await tapIn(tester, find.text('Join Feeder'));

      expect(find.textContaining('may or may not have joined'), findsOneWidget);
      expect(book.feeders, isEmpty);
      final button = tester.widget<FilledButton>(find.byType(FilledButton));
      expect(button.onPressed, isNull);
    });
  });
}
