import 'dart:async';

import 'package:flutter/material.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:share_plus/share_plus.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/replies.dart';

/// Choose a role (11.2), create one invite, and show it. An unanswered create
/// is not sent again: at worst an unseen invite runs out by itself.
Future<void> inviteSomeone(
  BuildContext context,
  FeederController feeder,
) async {
  final role = await showModalBottomSheet<ClientRole>(
    context: context,
    isScrollControlled: true,
    builder: (_) => _RoleSheet(name: feeder.known.name),
  );
  if (role == null || !context.mounted) return;

  final reply = await feeder.ask(FeederRequest.createInvite(role: role));
  if (!context.mounted) return;
  if (reply is FeederReply_Invite) {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => InviteScreen(invite: reply.invite),
      ),
    );
  } else {
    showProblem(context, reply);
  }
}

class _RoleSheet extends StatefulWidget {
  const _RoleSheet({required this.name});

  final String name;

  @override
  State<_RoleSheet> createState() => _RoleSheetState();
}

class _RoleSheetState extends State<_RoleSheet> {
  ClientRole _role = ClientRole.member;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(Space.s5),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Invite someone to ${widget.name}',
              style: TextStyle(
                fontSize: 22,
                fontWeight: Fonts.semibold,
                color: c.text,
              ),
            ),
            const SizedBox(height: Space.s3),
            RadioGroup<ClientRole>(
              groupValue: _role,
              onChanged: (role) =>
                  setState(() => _role = role ?? ClientRole.member),
              child: const Column(
                children: [
                  RadioListTile<ClientRole>(
                    value: ClientRole.member,
                    contentPadding: EdgeInsets.zero,
                    title: Text('Member'),
                    subtitle: Text(
                      'Feeds, edits meals and settings, locks the buttons.',
                    ),
                  ),
                  RadioListTile<ClientRole>(
                    value: ClientRole.owner,
                    contentPadding: EdgeInsets.zero,
                    title: Text('Owner'),
                    subtitle: Text(
                      'Everything, including Wi-Fi, updates and removing '
                      'people.',
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: Space.s2),
            Text(
              "They'll need to be on the same Wi-Fi as ${widget.name} to join.",
              style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
            ),
            const SizedBox(height: Space.s4),
            FilledButton(
              onPressed: () => Navigator.pop(context, _role),
              child: const Text('Create invite'),
            ),
          ],
        ),
      ),
    );
  }
}

/// One invite, shown once (11.3). It holds a secret, so it leaves this screen
/// only when the owner shares it, and it is never kept.
class InviteScreen extends StatefulWidget {
  const InviteScreen({super.key, required this.invite, this.share = _share});

  final InviteView invite;
  final Future<void> Function(String text) share;

  static Future<void> _share(String text) async {
    await SharePlus.instance.share(ShareParams(text: text));
  }

  @override
  State<InviteScreen> createState() => _InviteScreenState();
}

class _InviteScreenState extends State<InviteScreen> {
  late int _left = widget.invite.seconds;
  late final Timer _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (_left <= 1) _tick.cancel();
      setState(() => _left -= 1);
    });
  }

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final invite = widget.invite;
    final clock = '${_left ~/ 60}:${(_left % 60).toString().padLeft(2, '0')}';
    final expired = _left <= 0;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Close',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.close, color: c.text),
        ),
        title: Text(
          invite.role == ClientRole.owner ? 'Owner invite' : 'Member invite',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.s5),
        children: [
          Text(
            'On their phone: Wellfed › Add a feeder › Join with an invite, '
            'then scan this.',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 15, height: 1.5, color: c.textMuted),
          ),
          const SizedBox(height: Space.s4),
          if (!expired)
            Center(
              child: Container(
                // A code stays readable in dark mode only on white.
                color: Colors.white,
                padding: const EdgeInsets.all(Space.s3),
                child: QrImageView(data: invite.text, size: 240),
              ),
            ),
          const SizedBox(height: Space.s3),
          Text(
            expired
                ? 'This invite has expired. Close this and create a new one.'
                : '$clock until it expires · works once',
            textAlign: TextAlign.center,
            style: TextStyle(
              fontSize: 15,
              fontWeight: Fonts.semibold,
              color: c.text,
            ),
          ),
          const SizedBox(height: Space.s4),
          Text(
            "Shown once. Close this and you'll need a new invite.",
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
          ),
          const SizedBox(height: Space.s4),
          OutlinedButton(
            onPressed: expired
                ? null
                : () => unawaited(widget.share(invite.text)),
            child: const Text('Share link instead'),
          ),
        ],
      ),
    );
  }
}
