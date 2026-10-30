import 'package:flutter/foundation.dart';

import '../platform/feeder_ports.dart';
import 'app_state.dart';
import 'feeder.dart';

/// One controller per feeder this phone knows, kept while the app runs, so
/// every screen of a feeder shows the one session.
class FeederFleet extends ChangeNotifier {
  FeederFleet(this._book, this._ports) {
    _book.addListener(_sync);
    _sync();
  }

  final FeederBook _book;
  final FeederPorts _ports;
  final _feeders = <String, FeederController>{};

  Iterable<FeederController> get all => _feeders.values;

  FeederController? operator [](String identity) => _feeders[identity];

  void _sync() {
    final known = {for (final f in _book.feeders) f.identity: f};
    for (final gone
        in _feeders.keys.where((id) => !known.containsKey(id)).toList()) {
      _feeders.remove(gone)?.dispose();
    }
    for (final feeder in known.values) {
      _feeders.putIfAbsent(
        feeder.identity,
        () => FeederController(_book, _ports, feeder),
      );
    }
    notifyListeners();
  }

  @override
  void dispose() {
    _book.removeListener(_sync);
    for (final controller in _feeders.values) {
      controller.dispose();
    }
    super.dispose();
  }
}
