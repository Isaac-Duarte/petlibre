import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:path_provider/path_provider.dart';

import '../src/rust/api/keys.dart';
import '../src/rust/api/store.dart';

/// The feeders this phone knows, and what the flows that add one need of
/// the app's file.
abstract interface class FeederBook implements Listenable {
  /// How feeders list this phone; empty until the owner names it.
  String get label;

  List<KnownFeeder> get feeders;

  Future<void> setLabel(String label);

  /// Add `feeder`, or replace the one with its identity. With `paired`, this
  /// phone just became its owner, so its next session gives it this phone's
  /// clock.
  Future<void> remember(KnownFeeder feeder, {bool paired = false});

  /// Keep where `identity` last answered, so the next start tries it before
  /// NSD has looked.
  Future<void> answeredAt(String identity, String address);
}

/// The phone's identity and the feeders it knows. The client secret lives
/// in Keystore-backed secure storage; everything else in the app's own
/// file, which Rust reads and writes.
class AppState extends ChangeNotifier implements FeederBook {
  AppState._(this.key, this._store, this._path);

  static const _secretName = 'client-secret';

  final ClientKey key;
  final String _path;
  AppStore _store;

  /// Feeders that paired since the app started, whose clock this phone has
  /// not set yet. In memory: a feeder with no clock is set whenever a
  /// session opens, whatever this holds.
  final _clockDue = <String>{};

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

  @override
  String get label => _store.label;

  @override
  List<KnownFeeder> get feeders => _store.feeders;

  bool clockDue(String identity) => _clockDue.contains(identity);

  void clockSynced(String identity) => _clockDue.remove(identity);

  @override
  Future<void> setLabel(String label) =>
      _save(AppStore(label: label, feeders: _store.feeders));

  @override
  Future<void> remember(KnownFeeder feeder, {bool paired = false}) {
    if (paired) _clockDue.add(feeder.identity);
    return _save(
      AppStore(
        label: _store.label,
        feeders: [
          for (final known in _store.feeders)
            if (known.identity != feeder.identity) known,
          feeder,
        ],
      ),
    );
  }

  @override
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
