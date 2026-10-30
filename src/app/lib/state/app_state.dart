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
  /// clock. With `keepName`, a feeder already known keeps the name the owner
  /// gave it.
  Future<void> remember(
    KnownFeeder feeder, {
    bool paired = false,
    bool keepName = false,
  });

  /// Keep where `identity` last answered, so the next start tries it before
  /// NSD has looked.
  Future<void> answeredAt(String identity, String address);

  /// Whether this phone owes `identity` its clock, having just paired.
  bool clockDue(String identity);

  void clockSynced(String identity);
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

  Future<void> _writing = Future.value();

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

  @override
  bool clockDue(String identity) => _clockDue.contains(identity);

  @override
  void clockSynced(String identity) => _clockDue.remove(identity);

  @override
  Future<void> setLabel(String label) =>
      _update((store) => AppStore(label: label, feeders: store.feeders));

  @override
  Future<void> remember(
    KnownFeeder feeder, {
    bool paired = false,
    bool keepName = false,
  }) {
    if (paired) _clockDue.add(feeder.identity);
    return _update((store) {
      final known = store.feeders.where((f) => f.identity == feeder.identity);
      return AppStore(
        label: store.label,
        feeders: [
          for (final other in store.feeders)
            if (other.identity != feeder.identity) other,
          KnownFeeder(
            identity: feeder.identity,
            name: keepName && known.isNotEmpty ? known.first.name : feeder.name,
            address: feeder.address,
          ),
        ],
      );
    });
  }

  @override
  Future<void> answeredAt(String identity, String address) async {
    final known = _store.feeders.where((f) => f.identity == identity);
    if (known.isEmpty || known.first.address == address) {
      return;
    }
    await remember(
      KnownFeeder(identity: identity, name: known.first.name, address: address),
      keepName: true,
    );
  }

  /// Change the file from what it holds now, not from this copy of it: an
  /// install's run saves a feeder there from Rust, before Dart hears of it.
  /// One at a time, so two changes do not read the same file.
  Future<void> _update(AppStore Function(AppStore) change) {
    final done = _writing.then((_) async {
      final store = change(await loadStore(path: _path));
      await saveStore(path: _path, store: store);
      _store = store;
      notifyListeners();
    });
    // A failed write is its caller's to hear; the next one still goes.
    _writing = done.catchError((Object _) {});
    return done;
  }
}
