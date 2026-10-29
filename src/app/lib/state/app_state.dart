import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:path_provider/path_provider.dart';

import '../src/rust/api/keys.dart';
import '../src/rust/api/store.dart';

/// The phone's identity and the feeders it knows. The client secret lives
/// in Keystore-backed secure storage; everything else in the app's own
/// file, which Rust reads and writes.
class AppState extends ChangeNotifier {
  AppState._(this.key, this._store, this._path);

  static const _secretName = 'client-secret';

  final ClientKey key;
  final String _path;
  AppStore _store;

  static Future<AppState> load() async {
    const storage = FlutterSecureStorage();
    final saved = await storage.read(key: _secretName);
    final ClientKey key;
    if (saved == null) {
      key = ClientKey.generate();
      // The store's hex spelling, as for feeder identities: 32 bytes.
      await storage.write(
        key: _secretName,
        value: identityHex(identity: key.secret()),
      );
    } else {
      key = ClientKey.restore(secret: identityBytes(hex: saved));
    }

    final dir = await getApplicationSupportDirectory();
    final path = '${dir.path}/wellfed.json';
    return AppState._(key, await loadStore(path: path), path);
  }

  /// How feeders list this phone; empty until the owner names it.
  String get label => _store.label;

  List<KnownFeeder> get feeders => _store.feeders;

  Future<void> setLabel(String label) =>
      _save(AppStore(label: label, feeders: _store.feeders));

  /// Add `feeder`, or replace the one with its identity.
  Future<void> remember(KnownFeeder feeder) => _save(
    AppStore(
      label: _store.label,
      feeders: [
        for (final known in _store.feeders)
          if (known.identity != feeder.identity) known,
        feeder,
      ],
    ),
  );

  /// Keep where `identity` last answered, so the next start tries it
  /// before NSD has looked.
  Future<void> answeredAt(String identity, String address) async {
    final known = _store.feeders.where((f) => f.identity == identity);
    if (known.isEmpty || known.first.address == address) {
      return;
    }
    await remember(
      KnownFeeder(identity: identity, name: known.first.name, address: address),
    );
  }

  Future<void> _save(AppStore store) async {
    await saveStore(path: _path, store: store);
    _store = store;
    notifyListeners();
  }
}
