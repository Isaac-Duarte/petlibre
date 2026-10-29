// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'session.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$SessionUpdate {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'SessionUpdate()';
}


}

/// @nodoc
class $SessionUpdateCopyWith<$Res>  {
$SessionUpdateCopyWith(SessionUpdate _, $Res Function(SessionUpdate) __);
}


/// Adds pattern-matching-related methods to [SessionUpdate].
extension SessionUpdatePatterns on SessionUpdate {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( SessionUpdate_Opened value)?  opened,TResult Function( SessionUpdate_Status value)?  status,TResult Function( SessionUpdate_FeedStarted value)?  feedStarted,TResult Function( SessionUpdate_FeedRefused value)?  feedRefused,TResult Function( SessionUpdate_FeedUnanswered value)?  feedUnanswered,TResult Function( SessionUpdate_FeedEnded value)?  feedEnded,TResult Function( SessionUpdate_CancelUnconfirmed value)?  cancelUnconfirmed,TResult Function( SessionUpdate_Closed value)?  closed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that);case SessionUpdate_Status() when status != null:
return status(_that);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered(_that);case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that);case SessionUpdate_Closed() when closed != null:
return closed(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( SessionUpdate_Opened value)  opened,required TResult Function( SessionUpdate_Status value)  status,required TResult Function( SessionUpdate_FeedStarted value)  feedStarted,required TResult Function( SessionUpdate_FeedRefused value)  feedRefused,required TResult Function( SessionUpdate_FeedUnanswered value)  feedUnanswered,required TResult Function( SessionUpdate_FeedEnded value)  feedEnded,required TResult Function( SessionUpdate_CancelUnconfirmed value)  cancelUnconfirmed,required TResult Function( SessionUpdate_Closed value)  closed,}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened():
return opened(_that);case SessionUpdate_Status():
return status(_that);case SessionUpdate_FeedStarted():
return feedStarted(_that);case SessionUpdate_FeedRefused():
return feedRefused(_that);case SessionUpdate_FeedUnanswered():
return feedUnanswered(_that);case SessionUpdate_FeedEnded():
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed():
return cancelUnconfirmed(_that);case SessionUpdate_Closed():
return closed(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( SessionUpdate_Opened value)?  opened,TResult? Function( SessionUpdate_Status value)?  status,TResult? Function( SessionUpdate_FeedStarted value)?  feedStarted,TResult? Function( SessionUpdate_FeedRefused value)?  feedRefused,TResult? Function( SessionUpdate_FeedUnanswered value)?  feedUnanswered,TResult? Function( SessionUpdate_FeedEnded value)?  feedEnded,TResult? Function( SessionUpdate_CancelUnconfirmed value)?  cancelUnconfirmed,TResult? Function( SessionUpdate_Closed value)?  closed,}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that);case SessionUpdate_Status() when status != null:
return status(_that);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered(_that);case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that);case SessionUpdate_Closed() when closed != null:
return closed(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String build,  ClientRole role)?  opened,TResult Function( Card card)?  status,TResult Function( int feed,  int portions)?  feedStarted,TResult Function( FeedRefusal refusal)?  feedRefused,TResult Function()?  feedUnanswered,TResult Function( FeedResult result)?  feedEnded,TResult Function( FeedRefusal? refusal)?  cancelUnconfirmed,TResult Function( BridgeError? error)?  closed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that.build,_that.role);case SessionUpdate_Status() when status != null:
return status(_that.card);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered();case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that.refusal);case SessionUpdate_Closed() when closed != null:
return closed(_that.error);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String build,  ClientRole role)  opened,required TResult Function( Card card)  status,required TResult Function( int feed,  int portions)  feedStarted,required TResult Function( FeedRefusal refusal)  feedRefused,required TResult Function()  feedUnanswered,required TResult Function( FeedResult result)  feedEnded,required TResult Function( FeedRefusal? refusal)  cancelUnconfirmed,required TResult Function( BridgeError? error)  closed,}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened():
return opened(_that.build,_that.role);case SessionUpdate_Status():
return status(_that.card);case SessionUpdate_FeedStarted():
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused():
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered():
return feedUnanswered();case SessionUpdate_FeedEnded():
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed():
return cancelUnconfirmed(_that.refusal);case SessionUpdate_Closed():
return closed(_that.error);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String build,  ClientRole role)?  opened,TResult? Function( Card card)?  status,TResult? Function( int feed,  int portions)?  feedStarted,TResult? Function( FeedRefusal refusal)?  feedRefused,TResult? Function()?  feedUnanswered,TResult? Function( FeedResult result)?  feedEnded,TResult? Function( FeedRefusal? refusal)?  cancelUnconfirmed,TResult? Function( BridgeError? error)?  closed,}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that.build,_that.role);case SessionUpdate_Status() when status != null:
return status(_that.card);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered();case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that.refusal);case SessionUpdate_Closed() when closed != null:
return closed(_that.error);case _:
  return null;

}
}

}

/// @nodoc


class SessionUpdate_Opened extends SessionUpdate {
  const SessionUpdate_Opened({required this.build, required this.role}): super._();
  

 final  String build;
 final  ClientRole role;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_OpenedCopyWith<SessionUpdate_Opened> get copyWith => _$SessionUpdate_OpenedCopyWithImpl<SessionUpdate_Opened>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_Opened&&(identical(other.build, build) || other.build == build)&&(identical(other.role, role) || other.role == role));
}


@override
int get hashCode {
    return Object.hash(runtimeType,build,role);
}

@override
String toString() {
    return 'SessionUpdate.opened(build: $build, role: $role)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_OpenedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_OpenedCopyWith(SessionUpdate_Opened value, $Res Function(SessionUpdate_Opened) _then) = _$SessionUpdate_OpenedCopyWithImpl;
@useResult
$Res call({
 String build, ClientRole role
});




}
/// @nodoc
class _$SessionUpdate_OpenedCopyWithImpl<$Res>
    implements $SessionUpdate_OpenedCopyWith<$Res> {
  _$SessionUpdate_OpenedCopyWithImpl(this._self, this._then);

  final SessionUpdate_Opened _self;
  final $Res Function(SessionUpdate_Opened) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? build = null,Object? role = null,}) {
  return _then(SessionUpdate_Opened(
build: null == build ? _self.build : build // ignore: cast_nullable_to_non_nullable
as String,role: null == role ? _self.role : role // ignore: cast_nullable_to_non_nullable
as ClientRole,
  ));
}


}

/// @nodoc


class SessionUpdate_Status extends SessionUpdate {
  const SessionUpdate_Status({required this.card}): super._();
  

 final  Card card;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_StatusCopyWith<SessionUpdate_Status> get copyWith => _$SessionUpdate_StatusCopyWithImpl<SessionUpdate_Status>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_Status&&(identical(other.card, card) || other.card == card));
}


@override
int get hashCode {
    return Object.hash(runtimeType,card);
}

@override
String toString() {
    return 'SessionUpdate.status(card: $card)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_StatusCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_StatusCopyWith(SessionUpdate_Status value, $Res Function(SessionUpdate_Status) _then) = _$SessionUpdate_StatusCopyWithImpl;
@useResult
$Res call({
 Card card
});




}
/// @nodoc
class _$SessionUpdate_StatusCopyWithImpl<$Res>
    implements $SessionUpdate_StatusCopyWith<$Res> {
  _$SessionUpdate_StatusCopyWithImpl(this._self, this._then);

  final SessionUpdate_Status _self;
  final $Res Function(SessionUpdate_Status) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? card = null,}) {
  return _then(SessionUpdate_Status(
card: null == card ? _self.card : card // ignore: cast_nullable_to_non_nullable
as Card,
  ));
}


}

/// @nodoc


class SessionUpdate_FeedStarted extends SessionUpdate {
  const SessionUpdate_FeedStarted({required this.feed, required this.portions}): super._();
  

 final  int feed;
 final  int portions;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_FeedStartedCopyWith<SessionUpdate_FeedStarted> get copyWith => _$SessionUpdate_FeedStartedCopyWithImpl<SessionUpdate_FeedStarted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_FeedStarted&&(identical(other.feed, feed) || other.feed == feed)&&(identical(other.portions, portions) || other.portions == portions));
}


@override
int get hashCode {
    return Object.hash(runtimeType,feed,portions);
}

@override
String toString() {
    return 'SessionUpdate.feedStarted(feed: $feed, portions: $portions)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_FeedStartedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_FeedStartedCopyWith(SessionUpdate_FeedStarted value, $Res Function(SessionUpdate_FeedStarted) _then) = _$SessionUpdate_FeedStartedCopyWithImpl;
@useResult
$Res call({
 int feed, int portions
});




}
/// @nodoc
class _$SessionUpdate_FeedStartedCopyWithImpl<$Res>
    implements $SessionUpdate_FeedStartedCopyWith<$Res> {
  _$SessionUpdate_FeedStartedCopyWithImpl(this._self, this._then);

  final SessionUpdate_FeedStarted _self;
  final $Res Function(SessionUpdate_FeedStarted) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? feed = null,Object? portions = null,}) {
  return _then(SessionUpdate_FeedStarted(
feed: null == feed ? _self.feed : feed // ignore: cast_nullable_to_non_nullable
as int,portions: null == portions ? _self.portions : portions // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class SessionUpdate_FeedRefused extends SessionUpdate {
  const SessionUpdate_FeedRefused({required this.refusal}): super._();
  

 final  FeedRefusal refusal;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_FeedRefusedCopyWith<SessionUpdate_FeedRefused> get copyWith => _$SessionUpdate_FeedRefusedCopyWithImpl<SessionUpdate_FeedRefused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_FeedRefused&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'SessionUpdate.feedRefused(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_FeedRefusedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_FeedRefusedCopyWith(SessionUpdate_FeedRefused value, $Res Function(SessionUpdate_FeedRefused) _then) = _$SessionUpdate_FeedRefusedCopyWithImpl;
@useResult
$Res call({
 FeedRefusal refusal
});




}
/// @nodoc
class _$SessionUpdate_FeedRefusedCopyWithImpl<$Res>
    implements $SessionUpdate_FeedRefusedCopyWith<$Res> {
  _$SessionUpdate_FeedRefusedCopyWithImpl(this._self, this._then);

  final SessionUpdate_FeedRefused _self;
  final $Res Function(SessionUpdate_FeedRefused) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = null,}) {
  return _then(SessionUpdate_FeedRefused(
refusal: null == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as FeedRefusal,
  ));
}


}

/// @nodoc


class SessionUpdate_FeedUnanswered extends SessionUpdate {
  const SessionUpdate_FeedUnanswered(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_FeedUnanswered);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'SessionUpdate.feedUnanswered()';
}


}




/// @nodoc


class SessionUpdate_FeedEnded extends SessionUpdate {
  const SessionUpdate_FeedEnded({required this.result}): super._();
  

 final  FeedResult result;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_FeedEndedCopyWith<SessionUpdate_FeedEnded> get copyWith => _$SessionUpdate_FeedEndedCopyWithImpl<SessionUpdate_FeedEnded>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_FeedEnded&&(identical(other.result, result) || other.result == result));
}


@override
int get hashCode {
    return Object.hash(runtimeType,result);
}

@override
String toString() {
    return 'SessionUpdate.feedEnded(result: $result)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_FeedEndedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_FeedEndedCopyWith(SessionUpdate_FeedEnded value, $Res Function(SessionUpdate_FeedEnded) _then) = _$SessionUpdate_FeedEndedCopyWithImpl;
@useResult
$Res call({
 FeedResult result
});




}
/// @nodoc
class _$SessionUpdate_FeedEndedCopyWithImpl<$Res>
    implements $SessionUpdate_FeedEndedCopyWith<$Res> {
  _$SessionUpdate_FeedEndedCopyWithImpl(this._self, this._then);

  final SessionUpdate_FeedEnded _self;
  final $Res Function(SessionUpdate_FeedEnded) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? result = null,}) {
  return _then(SessionUpdate_FeedEnded(
result: null == result ? _self.result : result // ignore: cast_nullable_to_non_nullable
as FeedResult,
  ));
}


}

/// @nodoc


class SessionUpdate_CancelUnconfirmed extends SessionUpdate {
  const SessionUpdate_CancelUnconfirmed({this.refusal}): super._();
  

 final  FeedRefusal? refusal;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_CancelUnconfirmedCopyWith<SessionUpdate_CancelUnconfirmed> get copyWith => _$SessionUpdate_CancelUnconfirmedCopyWithImpl<SessionUpdate_CancelUnconfirmed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_CancelUnconfirmed&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'SessionUpdate.cancelUnconfirmed(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_CancelUnconfirmedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_CancelUnconfirmedCopyWith(SessionUpdate_CancelUnconfirmed value, $Res Function(SessionUpdate_CancelUnconfirmed) _then) = _$SessionUpdate_CancelUnconfirmedCopyWithImpl;
@useResult
$Res call({
 FeedRefusal? refusal
});




}
/// @nodoc
class _$SessionUpdate_CancelUnconfirmedCopyWithImpl<$Res>
    implements $SessionUpdate_CancelUnconfirmedCopyWith<$Res> {
  _$SessionUpdate_CancelUnconfirmedCopyWithImpl(this._self, this._then);

  final SessionUpdate_CancelUnconfirmed _self;
  final $Res Function(SessionUpdate_CancelUnconfirmed) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = freezed,}) {
  return _then(SessionUpdate_CancelUnconfirmed(
refusal: freezed == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as FeedRefusal?,
  ));
}


}

/// @nodoc


class SessionUpdate_Closed extends SessionUpdate {
  const SessionUpdate_Closed({this.error}): super._();
  

 final  BridgeError? error;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_ClosedCopyWith<SessionUpdate_Closed> get copyWith => _$SessionUpdate_ClosedCopyWithImpl<SessionUpdate_Closed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_Closed&&(identical(other.error, error) || other.error == error));
}


@override
int get hashCode {
    return Object.hash(runtimeType,error);
}

@override
String toString() {
    return 'SessionUpdate.closed(error: $error)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_ClosedCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_ClosedCopyWith(SessionUpdate_Closed value, $Res Function(SessionUpdate_Closed) _then) = _$SessionUpdate_ClosedCopyWithImpl;
@useResult
$Res call({
 BridgeError? error
});




}
/// @nodoc
class _$SessionUpdate_ClosedCopyWithImpl<$Res>
    implements $SessionUpdate_ClosedCopyWith<$Res> {
  _$SessionUpdate_ClosedCopyWithImpl(this._self, this._then);

  final SessionUpdate_Closed _self;
  final $Res Function(SessionUpdate_Closed) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? error = freezed,}) {
  return _then(SessionUpdate_Closed(
error: freezed == error ? _self.error : error // ignore: cast_nullable_to_non_nullable
as BridgeError?,
  ));
}


}

// dart format on
