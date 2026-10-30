import 'dart:async';

import 'package:flutter/foundation.dart';

import '../platform/feeder_ports.dart';
import '../src/rust/api/error.dart';
import '../src/rust/api/feeder.dart';
import '../src/rust/api/model.dart';
import '../src/rust/api/session.dart';
import '../src/rust/api/store.dart';
import 'app_state.dart';

enum Connection {
  /// Looking for the feeder's address on the LAN.
  looking,
  connecting,
  online,

  /// Not reachable; [FeederController.error] says how the last try ended.
  offline,
}

enum FeedPhase {
  idle,

  /// Sent; waiting for the feeder to reserve it.
  sending,

  /// The feeder runs it as [FeedState.feed].
  running,
  ended,
  refused,

  /// No answer came: the feed may have run. It is never sent again, and
  /// no later feed's end is taken for it: the bowl or the history says.
  unanswered,

  /// The session ended before the request went out: nothing was fed.
  notSent,

  /// The session dropped while the feed ran; the feeder finishes it on its
  /// own and its history keeps the result.
  lost,
}

@immutable
class FeedState {
  const FeedState(
    this.phase, {
    this.portions = 0,
    this.feed,
    this.result,
    this.refusal,
    this.cancelUnconfirmed = false,
    this.sentAt,
    this.endedAt,
  });

  static const idle = FeedState(FeedPhase.idle);

  final FeedPhase phase;
  final int portions;
  final int? feed;
  final FeedResult? result;
  final FeedRefusal? refusal;

  /// "Stop feeding" was refused or went unanswered; the feed may still run
  /// to its end.
  final bool cancelUnconfirmed;

  /// When this phone sent the feed, for "We sent the feed at …".
  final DateTime? sentAt;

  /// When its end arrived.
  final DateTime? endedAt;

  bool get active => phase == FeedPhase.sending || phase == FeedPhase.running;
}

/// One known feeder for the Home card: keeps a session open while the app
/// runs, reconnecting with backoff, and runs feeds through it. Every feed
/// goes through the device API like any other client's; this holds no
/// feeding logic beyond showing what the feeder said.
class FeederController extends ChangeNotifier {
  FeederController(this._book, this._ports, this.known) {
    unawaited(_serve());
  }

  static const _locateWait = Duration(seconds: 8);
  static const _retryFirst = Duration(seconds: 3);
  static const _retryMost = Duration(seconds: 30);

  final FeederBook _book;
  final FeederPorts _ports;
  KnownFeeder known;

  Connection connection = Connection.looking;
  Card? card;

  /// When [card] was read, for an offline card's age.
  DateTime? cardAt;
  ClientRole? role;
  ErrorKind? error;
  FeedState feed = FeedState.idle;

  /// The feeder's history as this session has read it, oldest first.
  List<HistoryEntry> history = const [];

  /// [history] holds everything the feeder kept when this session opened.
  bool historyRead = false;

  FeederSession? _session;
  bool _disposed = false;

  /// The card was read in the session now open, not an earlier one.
  bool _fresh = false;

  bool get canFeed =>
      connection == Connection.online &&
      _fresh &&
      !feed.active &&
      role != ClientRole.server &&
      (card?.ready ?? false);

  /// Feed `portions` once. The feeder's answer and the feed's end arrive
  /// through the session.
  void feedNow(int portions) {
    final session = _session;
    if (session == null || feed.active) return;
    _setFeed(
      FeedState(FeedPhase.sending, portions: portions, sentAt: DateTime.now()),
    );
    try {
      session.feed(portions: portions);
    } on BridgeError {
      // The session had already ended; the request never went out.
      _setFeed(
        FeedState(FeedPhase.notSent, portions: portions, sentAt: feed.sentAt),
      );
    }
  }

  void cancelFeed() {
    final running = feed.feed;
    if (running == null) return;
    try {
      _session?.cancelFeed(feed: running);
    } on BridgeError {
      // The session ended; the feed result comes from the history.
    }
  }

  void dismissFeed() => _setFeed(FeedState.idle);

  /// One request to the feeder. A write goes out once and is never sent
  /// again; the reply says how it ended, an unknown end included.
  Future<FeederReply> ask(FeederRequest request) async {
    final session = _session;
    if (session == null) return const FeederReply.notSent();
    return session.ask(request: request);
  }

  Future<void> _serve() async {
    var retry = _retryFirst;
    while (!_disposed) {
      final address = await _address();
      if (_disposed) return;
      if (address == null) {
        error = ErrorKind.connect;
        _set(Connection.offline);
      } else {
        _set(Connection.connecting);
        final opened = await _run(address);
        if (opened) retry = _retryFirst;
      }
      if (_disposed) return;
      await Future<void>.delayed(retry);
      retry = retry * 2 > _retryMost ? _retryMost : retry * 2;
    }
  }

  /// The last address, or NSD's when there is none or it stopped working.
  Future<String?> _address() async {
    if (connection != Connection.offline && known.address != null) {
      return known.address;
    }
    _set(Connection.looking);
    return await _ports.locate(known.identity, _locateWait) ?? known.address;
  }

  /// One session until it closes; whether it opened.
  Future<bool> _run(String address) async {
    final FeederSession session;
    try {
      session = _ports.open(
        known.identity,
        address,
        syncClock: _book.clockDue(known.identity),
      );
    } on BridgeError catch (failed) {
      error = failed.kind;
      _set(Connection.offline);
      return false;
    }
    _session = session;
    _fresh = false;

    var opened = false;
    await for (final update in session.run()) {
      if (_disposed) {
        session.close();
        break;
      }
      switch (update) {
        case SessionUpdate_Opened(role: final r):
          opened = true;
          role = r;
          error = null;
          history = const [];
          historyRead = false;
          _set(Connection.online);
          try {
            await _book.answeredAt(known.identity, address);
            known = _book.feeders.firstWhere(
              (f) => f.identity == known.identity,
              orElse: () => known,
            );
          } on BridgeError {
            // Only a hint for the next start; NSD finds the feeder anyway.
          }
        case SessionUpdate_Status(card: final c):
          card = c;
          cardAt = DateTime.now();
          _fresh = true;
          _notify();
        case SessionUpdate_FeedStarted(:final feed, :final portions):
          _setFeed(
            FeedState(
              FeedPhase.running,
              portions: portions,
              feed: feed,
              sentAt: this.feed.sentAt,
            ),
          );
        case SessionUpdate_FeedRefused(:final refusal):
          _setFeed(
            FeedState(
              FeedPhase.refused,
              portions: feed.portions,
              refusal: refusal,
              sentAt: feed.sentAt,
            ),
          );
        case SessionUpdate_FeedUnanswered():
          _setFeed(
            FeedState(
              FeedPhase.unanswered,
              portions: feed.portions,
              sentAt: feed.sentAt,
            ),
          );
        case SessionUpdate_FeedEnded(:final result):
          // Ours only when it carries our number: an unanswered feed has
          // none, and the next end may be a meal or another phone's.
          if (feed.phase == FeedPhase.running && feed.feed == result.feed) {
            _setFeed(
              FeedState(
                FeedPhase.ended,
                portions: result.requested,
                feed: result.feed,
                result: result,
                sentAt: feed.sentAt,
                endedAt: DateTime.now(),
              ),
            );
          }
        case SessionUpdate_CancelUnconfirmed():
          if (feed.phase == FeedPhase.running) {
            _setFeed(
              FeedState(
                FeedPhase.running,
                portions: feed.portions,
                feed: feed.feed,
                cancelUnconfirmed: true,
                sentAt: feed.sentAt,
              ),
            );
          }
        case SessionUpdate_ClockSynced():
          _book.clockSynced(known.identity);
        case SessionUpdate_ClockNotSynced():
          // Still due: the next session tries again.
          break;
        case SessionUpdate_History(:final entries, :final caughtUp):
          history = [...history, ...entries];
          historyRead = historyRead || caughtUp;
          _notify();
        case SessionUpdate_Log():
          // Heard only by a screen that asked; none does yet.
          break;
        case SessionUpdate_Closed(error: final e):
          error = e?.kind;
      }
    }

    _session = null;
    _fresh = false;
    // The session reports every feed it sent as started, refused or
    // unanswered before it closes, so one still sending never went out.
    if (feed.phase == FeedPhase.sending) {
      _setFeed(
        FeedState(
          FeedPhase.notSent,
          portions: feed.portions,
          sentAt: feed.sentAt,
        ),
      );
    }
    if (feed.phase == FeedPhase.running) {
      _setFeed(
        FeedState(
          FeedPhase.lost,
          portions: feed.portions,
          feed: feed.feed,
          sentAt: feed.sentAt,
        ),
      );
    }
    if (!_disposed) _set(Connection.offline);
    return opened;
  }

  void _setFeed(FeedState next) {
    feed = next;
    _notify();
  }

  void _set(Connection next) {
    connection = next;
    _notify();
  }

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _session?.close();
    super.dispose();
  }
}
