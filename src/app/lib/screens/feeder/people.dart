import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/feeder.dart';
import '../../src/rust/api/model.dart';
import '../../state/feeder.dart';
import '../../theme/theme.dart';
import '../../widgets/icons.dart';
import '../../widgets/layout.dart';
import '../../widgets/replies.dart';
import 'invite.dart';

/// How many keys a feeder lists.
const clientsMost = 8;

String roleName(ClientRole role) => switch (role) {
  ClientRole.owner => 'Owner',
  ClientRole.member => 'Member',
  ClientRole.server => 'Server',
  ClientRole.updateSigner => 'Update key',
};

/// Who a feeder lists (11.1). Owner only: the settings tab links here from its
/// owner section.
class PeopleScreen extends StatefulWidget {
  const PeopleScreen({super.key, required this.feeder});

  final FeederController feeder;

  @override
  State<PeopleScreen> createState() => _PeopleScreenState();
}

class _PeopleScreenState extends State<PeopleScreen> {
  bool _asking = false;

  @override
  void initState() {
    super.initState();
    unawaited(widget.feeder.loadClients());
  }

  Future<void> _remove(ClientView client) async {
    final sure = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${client.label}?'),
        content: const Text(
          'Removing someone closes their connection at once. Their history '
          'stays.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (sure != true || !mounted) return;

    setState(() => _asking = true);
    final reply = await widget.feeder.ask(
      FeederRequest.revokeClient(key: client.key),
    );
    if (!mounted) return;
    setState(() => _asking = false);
    showProblem(context, reply);
    await widget.feeder.loadClients();
  }

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final feeder = widget.feeder;
    return Scaffold(
      appBar: AppBar(
        toolbarHeight: 64,
        automaticallyImplyLeading: false,
        titleSpacing: 4,
        leading: IconButton(
          tooltip: 'Back',
          onPressed: () => Navigator.pop(context),
          icon: StrokeIcon(Glyph.back, color: c.text),
        ),
        title: Text(
          'People',
          style: TextStyle(
            fontSize: 20,
            fontWeight: Fonts.semibold,
            color: c.text,
          ),
        ),
      ),
      body: ListenableBuilder(
        listenable: feeder,
        builder: (context, _) => ListView(
          padding: const EdgeInsets.all(Space.s4),
          children: [
            if (feeder.clients.isNotEmpty)
              Panel(
                child: Column(
                  children: [
                    for (final (i, client) in feeder.clients.indexed)
                      _Person(
                        client: client,
                        first: i == 0,
                        onRemove: _asking || client.you
                            ? null
                            : () => unawaited(_remove(client)),
                      ),
                  ],
                ),
              ),
            const SizedBox(height: Space.s4),
            Text(
              'Removing someone closes their connection at once. Their '
              'history stays.',
              style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
            ),
            const SizedBox(height: Space.s4),
            FilledButton(
              onPressed: feeder.connection == Connection.online
                  ? () => unawaited(inviteSomeone(context, feeder))
                  : null,
              child: const Text('Invite someone'),
            ),
          ],
        ),
      ),
    );
  }
}

class _Person extends StatelessWidget {
  const _Person({
    required this.client,
    required this.first,
    required this.onRemove,
  });

  final ClientView client;
  final bool first;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    final c = context.colors;
    final name = client.you ? '${client.label} (this phone)' : client.label;
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s4,
        vertical: Space.s3,
      ),
      decoration: BoxDecoration(
        border: first ? null : Border(top: BorderSide(color: c.line)),
      ),
      child: Row(
        children: [
          CircleAvatar(
            radius: 20,
            backgroundColor: c.sunken,
            child: Text(
              client.label.isEmpty ? '?' : client.label.characters.first,
              style: TextStyle(color: c.text, fontWeight: Fonts.semibold),
            ),
          ),
          const SizedBox(width: Space.s3),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(name, style: TextStyle(fontSize: 15, color: c.text)),
                Text(
                  roleName(client.role),
                  style: TextStyle(fontSize: Sizes.sm, color: c.textMuted),
                ),
              ],
            ),
          ),
          if (!client.you)
            IconButton(
              tooltip: 'Remove ${client.label}',
              onPressed: onRemove,
              icon: StrokeIcon(Glyph.close, size: 20, color: c.textMuted),
            ),
        ],
      ),
    );
  }
}
