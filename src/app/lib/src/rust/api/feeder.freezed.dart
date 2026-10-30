// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'feeder.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$FeederRefusal {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal()';
}


}

/// @nodoc
class $FeederRefusalCopyWith<$Res>  {
$FeederRefusalCopyWith(FeederRefusal _, $Res Function(FeederRefusal) __);
}


/// Adds pattern-matching-related methods to [FeederRefusal].
extension FeederRefusalPatterns on FeederRefusal {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( FeederRefusal_Forbidden value)?  forbidden,TResult Function( FeederRefusal_Stale value)?  stale,TResult Function( FeederRefusal_Busy value)?  busy,TResult Function( FeederRefusal_Unavailable value)?  unavailable,TResult Function( FeederRefusal_Invalid value)?  invalid,TResult Function( FeederRefusal_NotWritable value)?  notWritable,TResult Function( FeederRefusal_Full value)?  full,TResult Function( FeederRefusal_NotFound value)?  notFound,TResult Function( FeederRefusal_Muted value)?  muted,TResult Function( FeederRefusal_Time value)?  time,TResult Function( FeederRefusal_Fault value)?  fault,TResult Function( FeederRefusal_Update value)?  update,TResult Function( FeederRefusal_Unverified value)?  unverified,TResult Function( FeederRefusal_Portions value)?  portions,TResult Function( FeederRefusal_NoClock value)?  noClock,required TResult orElse(),}){
final _that = this;
switch (_that) {
case FeederRefusal_Forbidden() when forbidden != null:
return forbidden(_that);case FeederRefusal_Stale() when stale != null:
return stale(_that);case FeederRefusal_Busy() when busy != null:
return busy(_that);case FeederRefusal_Unavailable() when unavailable != null:
return unavailable(_that);case FeederRefusal_Invalid() when invalid != null:
return invalid(_that);case FeederRefusal_NotWritable() when notWritable != null:
return notWritable(_that);case FeederRefusal_Full() when full != null:
return full(_that);case FeederRefusal_NotFound() when notFound != null:
return notFound(_that);case FeederRefusal_Muted() when muted != null:
return muted(_that);case FeederRefusal_Time() when time != null:
return time(_that);case FeederRefusal_Fault() when fault != null:
return fault(_that);case FeederRefusal_Update() when update != null:
return update(_that);case FeederRefusal_Unverified() when unverified != null:
return unverified(_that);case FeederRefusal_Portions() when portions != null:
return portions(_that);case FeederRefusal_NoClock() when noClock != null:
return noClock(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( FeederRefusal_Forbidden value)  forbidden,required TResult Function( FeederRefusal_Stale value)  stale,required TResult Function( FeederRefusal_Busy value)  busy,required TResult Function( FeederRefusal_Unavailable value)  unavailable,required TResult Function( FeederRefusal_Invalid value)  invalid,required TResult Function( FeederRefusal_NotWritable value)  notWritable,required TResult Function( FeederRefusal_Full value)  full,required TResult Function( FeederRefusal_NotFound value)  notFound,required TResult Function( FeederRefusal_Muted value)  muted,required TResult Function( FeederRefusal_Time value)  time,required TResult Function( FeederRefusal_Fault value)  fault,required TResult Function( FeederRefusal_Update value)  update,required TResult Function( FeederRefusal_Unverified value)  unverified,required TResult Function( FeederRefusal_Portions value)  portions,required TResult Function( FeederRefusal_NoClock value)  noClock,}){
final _that = this;
switch (_that) {
case FeederRefusal_Forbidden():
return forbidden(_that);case FeederRefusal_Stale():
return stale(_that);case FeederRefusal_Busy():
return busy(_that);case FeederRefusal_Unavailable():
return unavailable(_that);case FeederRefusal_Invalid():
return invalid(_that);case FeederRefusal_NotWritable():
return notWritable(_that);case FeederRefusal_Full():
return full(_that);case FeederRefusal_NotFound():
return notFound(_that);case FeederRefusal_Muted():
return muted(_that);case FeederRefusal_Time():
return time(_that);case FeederRefusal_Fault():
return fault(_that);case FeederRefusal_Update():
return update(_that);case FeederRefusal_Unverified():
return unverified(_that);case FeederRefusal_Portions():
return portions(_that);case FeederRefusal_NoClock():
return noClock(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( FeederRefusal_Forbidden value)?  forbidden,TResult? Function( FeederRefusal_Stale value)?  stale,TResult? Function( FeederRefusal_Busy value)?  busy,TResult? Function( FeederRefusal_Unavailable value)?  unavailable,TResult? Function( FeederRefusal_Invalid value)?  invalid,TResult? Function( FeederRefusal_NotWritable value)?  notWritable,TResult? Function( FeederRefusal_Full value)?  full,TResult? Function( FeederRefusal_NotFound value)?  notFound,TResult? Function( FeederRefusal_Muted value)?  muted,TResult? Function( FeederRefusal_Time value)?  time,TResult? Function( FeederRefusal_Fault value)?  fault,TResult? Function( FeederRefusal_Update value)?  update,TResult? Function( FeederRefusal_Unverified value)?  unverified,TResult? Function( FeederRefusal_Portions value)?  portions,TResult? Function( FeederRefusal_NoClock value)?  noClock,}){
final _that = this;
switch (_that) {
case FeederRefusal_Forbidden() when forbidden != null:
return forbidden(_that);case FeederRefusal_Stale() when stale != null:
return stale(_that);case FeederRefusal_Busy() when busy != null:
return busy(_that);case FeederRefusal_Unavailable() when unavailable != null:
return unavailable(_that);case FeederRefusal_Invalid() when invalid != null:
return invalid(_that);case FeederRefusal_NotWritable() when notWritable != null:
return notWritable(_that);case FeederRefusal_Full() when full != null:
return full(_that);case FeederRefusal_NotFound() when notFound != null:
return notFound(_that);case FeederRefusal_Muted() when muted != null:
return muted(_that);case FeederRefusal_Time() when time != null:
return time(_that);case FeederRefusal_Fault() when fault != null:
return fault(_that);case FeederRefusal_Update() when update != null:
return update(_that);case FeederRefusal_Unverified() when unverified != null:
return unverified(_that);case FeederRefusal_Portions() when portions != null:
return portions(_that);case FeederRefusal_NoClock() when noClock != null:
return noClock(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  forbidden,TResult Function( int current)?  stale,TResult Function()?  busy,TResult Function()?  unavailable,TResult Function()?  invalid,TResult Function()?  notWritable,TResult Function()?  full,TResult Function()?  notFound,TResult Function()?  muted,TResult Function()?  time,TResult Function()?  fault,TResult Function( UpdateRefusal reason)?  update,TResult Function()?  unverified,TResult Function( int asked)?  portions,TResult Function()?  noClock,required TResult orElse(),}) {final _that = this;
switch (_that) {
case FeederRefusal_Forbidden() when forbidden != null:
return forbidden();case FeederRefusal_Stale() when stale != null:
return stale(_that.current);case FeederRefusal_Busy() when busy != null:
return busy();case FeederRefusal_Unavailable() when unavailable != null:
return unavailable();case FeederRefusal_Invalid() when invalid != null:
return invalid();case FeederRefusal_NotWritable() when notWritable != null:
return notWritable();case FeederRefusal_Full() when full != null:
return full();case FeederRefusal_NotFound() when notFound != null:
return notFound();case FeederRefusal_Muted() when muted != null:
return muted();case FeederRefusal_Time() when time != null:
return time();case FeederRefusal_Fault() when fault != null:
return fault();case FeederRefusal_Update() when update != null:
return update(_that.reason);case FeederRefusal_Unverified() when unverified != null:
return unverified();case FeederRefusal_Portions() when portions != null:
return portions(_that.asked);case FeederRefusal_NoClock() when noClock != null:
return noClock();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  forbidden,required TResult Function( int current)  stale,required TResult Function()  busy,required TResult Function()  unavailable,required TResult Function()  invalid,required TResult Function()  notWritable,required TResult Function()  full,required TResult Function()  notFound,required TResult Function()  muted,required TResult Function()  time,required TResult Function()  fault,required TResult Function( UpdateRefusal reason)  update,required TResult Function()  unverified,required TResult Function( int asked)  portions,required TResult Function()  noClock,}) {final _that = this;
switch (_that) {
case FeederRefusal_Forbidden():
return forbidden();case FeederRefusal_Stale():
return stale(_that.current);case FeederRefusal_Busy():
return busy();case FeederRefusal_Unavailable():
return unavailable();case FeederRefusal_Invalid():
return invalid();case FeederRefusal_NotWritable():
return notWritable();case FeederRefusal_Full():
return full();case FeederRefusal_NotFound():
return notFound();case FeederRefusal_Muted():
return muted();case FeederRefusal_Time():
return time();case FeederRefusal_Fault():
return fault();case FeederRefusal_Update():
return update(_that.reason);case FeederRefusal_Unverified():
return unverified();case FeederRefusal_Portions():
return portions(_that.asked);case FeederRefusal_NoClock():
return noClock();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  forbidden,TResult? Function( int current)?  stale,TResult? Function()?  busy,TResult? Function()?  unavailable,TResult? Function()?  invalid,TResult? Function()?  notWritable,TResult? Function()?  full,TResult? Function()?  notFound,TResult? Function()?  muted,TResult? Function()?  time,TResult? Function()?  fault,TResult? Function( UpdateRefusal reason)?  update,TResult? Function()?  unverified,TResult? Function( int asked)?  portions,TResult? Function()?  noClock,}) {final _that = this;
switch (_that) {
case FeederRefusal_Forbidden() when forbidden != null:
return forbidden();case FeederRefusal_Stale() when stale != null:
return stale(_that.current);case FeederRefusal_Busy() when busy != null:
return busy();case FeederRefusal_Unavailable() when unavailable != null:
return unavailable();case FeederRefusal_Invalid() when invalid != null:
return invalid();case FeederRefusal_NotWritable() when notWritable != null:
return notWritable();case FeederRefusal_Full() when full != null:
return full();case FeederRefusal_NotFound() when notFound != null:
return notFound();case FeederRefusal_Muted() when muted != null:
return muted();case FeederRefusal_Time() when time != null:
return time();case FeederRefusal_Fault() when fault != null:
return fault();case FeederRefusal_Update() when update != null:
return update(_that.reason);case FeederRefusal_Unverified() when unverified != null:
return unverified();case FeederRefusal_Portions() when portions != null:
return portions(_that.asked);case FeederRefusal_NoClock() when noClock != null:
return noClock();case _:
  return null;

}
}

}

/// @nodoc


class FeederRefusal_Forbidden extends FeederRefusal {
  const FeederRefusal_Forbidden(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Forbidden);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.forbidden()';
}


}




/// @nodoc


class FeederRefusal_Stale extends FeederRefusal {
  const FeederRefusal_Stale({required this.current}): super._();
  

 final  int current;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRefusal_StaleCopyWith<FeederRefusal_Stale> get copyWith => _$FeederRefusal_StaleCopyWithImpl<FeederRefusal_Stale>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Stale&&(identical(other.current, current) || other.current == current));
}


@override
int get hashCode {
    return Object.hash(runtimeType,current);
}

@override
String toString() {
    return 'FeederRefusal.stale(current: $current)';
}


}

/// @nodoc
abstract mixin class $FeederRefusal_StaleCopyWith<$Res> implements $FeederRefusalCopyWith<$Res> {
  factory $FeederRefusal_StaleCopyWith(FeederRefusal_Stale value, $Res Function(FeederRefusal_Stale) _then) = _$FeederRefusal_StaleCopyWithImpl;
@useResult
$Res call({
 int current
});




}
/// @nodoc
class _$FeederRefusal_StaleCopyWithImpl<$Res>
    implements $FeederRefusal_StaleCopyWith<$Res> {
  _$FeederRefusal_StaleCopyWithImpl(this._self, this._then);

  final FeederRefusal_Stale _self;
  final $Res Function(FeederRefusal_Stale) _then;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? current = null,}) {
  return _then(FeederRefusal_Stale(
current: null == current ? _self.current : current // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class FeederRefusal_Busy extends FeederRefusal {
  const FeederRefusal_Busy(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Busy);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.busy()';
}


}




/// @nodoc


class FeederRefusal_Unavailable extends FeederRefusal {
  const FeederRefusal_Unavailable(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Unavailable);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.unavailable()';
}


}




/// @nodoc


class FeederRefusal_Invalid extends FeederRefusal {
  const FeederRefusal_Invalid(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Invalid);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.invalid()';
}


}




/// @nodoc


class FeederRefusal_NotWritable extends FeederRefusal {
  const FeederRefusal_NotWritable(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_NotWritable);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.notWritable()';
}


}




/// @nodoc


class FeederRefusal_Full extends FeederRefusal {
  const FeederRefusal_Full(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Full);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.full()';
}


}




/// @nodoc


class FeederRefusal_NotFound extends FeederRefusal {
  const FeederRefusal_NotFound(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_NotFound);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.notFound()';
}


}




/// @nodoc


class FeederRefusal_Muted extends FeederRefusal {
  const FeederRefusal_Muted(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Muted);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.muted()';
}


}




/// @nodoc


class FeederRefusal_Time extends FeederRefusal {
  const FeederRefusal_Time(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Time);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.time()';
}


}




/// @nodoc


class FeederRefusal_Fault extends FeederRefusal {
  const FeederRefusal_Fault(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Fault);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.fault()';
}


}




/// @nodoc


class FeederRefusal_Update extends FeederRefusal {
  const FeederRefusal_Update({required this.reason}): super._();
  

 final  UpdateRefusal reason;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRefusal_UpdateCopyWith<FeederRefusal_Update> get copyWith => _$FeederRefusal_UpdateCopyWithImpl<FeederRefusal_Update>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Update&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'FeederRefusal.update(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $FeederRefusal_UpdateCopyWith<$Res> implements $FeederRefusalCopyWith<$Res> {
  factory $FeederRefusal_UpdateCopyWith(FeederRefusal_Update value, $Res Function(FeederRefusal_Update) _then) = _$FeederRefusal_UpdateCopyWithImpl;
@useResult
$Res call({
 UpdateRefusal reason
});




}
/// @nodoc
class _$FeederRefusal_UpdateCopyWithImpl<$Res>
    implements $FeederRefusal_UpdateCopyWith<$Res> {
  _$FeederRefusal_UpdateCopyWithImpl(this._self, this._then);

  final FeederRefusal_Update _self;
  final $Res Function(FeederRefusal_Update) _then;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(FeederRefusal_Update(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as UpdateRefusal,
  ));
}


}

/// @nodoc


class FeederRefusal_Unverified extends FeederRefusal {
  const FeederRefusal_Unverified(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Unverified);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.unverified()';
}


}




/// @nodoc


class FeederRefusal_Portions extends FeederRefusal {
  const FeederRefusal_Portions({required this.asked}): super._();
  

 final  int asked;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRefusal_PortionsCopyWith<FeederRefusal_Portions> get copyWith => _$FeederRefusal_PortionsCopyWithImpl<FeederRefusal_Portions>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_Portions&&(identical(other.asked, asked) || other.asked == asked));
}


@override
int get hashCode {
    return Object.hash(runtimeType,asked);
}

@override
String toString() {
    return 'FeederRefusal.portions(asked: $asked)';
}


}

/// @nodoc
abstract mixin class $FeederRefusal_PortionsCopyWith<$Res> implements $FeederRefusalCopyWith<$Res> {
  factory $FeederRefusal_PortionsCopyWith(FeederRefusal_Portions value, $Res Function(FeederRefusal_Portions) _then) = _$FeederRefusal_PortionsCopyWithImpl;
@useResult
$Res call({
 int asked
});




}
/// @nodoc
class _$FeederRefusal_PortionsCopyWithImpl<$Res>
    implements $FeederRefusal_PortionsCopyWith<$Res> {
  _$FeederRefusal_PortionsCopyWithImpl(this._self, this._then);

  final FeederRefusal_Portions _self;
  final $Res Function(FeederRefusal_Portions) _then;

/// Create a copy of FeederRefusal
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? asked = null,}) {
  return _then(FeederRefusal_Portions(
asked: null == asked ? _self.asked : asked // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class FeederRefusal_NoClock extends FeederRefusal {
  const FeederRefusal_NoClock(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRefusal_NoClock);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRefusal.noClock()';
}


}




/// @nodoc
mixin _$FeederReply {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederReply()';
}


}

/// @nodoc
class $FeederReplyCopyWith<$Res>  {
$FeederReplyCopyWith(FeederReply _, $Res Function(FeederReply) __);
}


/// Adds pattern-matching-related methods to [FeederReply].
extension FeederReplyPatterns on FeederReply {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( FeederReply_Done value)?  done,TResult Function( FeederReply_Settings value)?  settings,TResult Function( FeederReply_Time value)?  time,TResult Function( FeederReply_Plans value)?  plans,TResult Function( FeederReply_Clients value)?  clients,TResult Function( FeederReply_Invite value)?  invite,TResult Function( FeederReply_Calibration value)?  calibration,TResult Function( FeederReply_Call value)?  call,TResult Function( FeederReply_Stats value)?  stats,TResult Function( FeederReply_Trace value)?  trace,TResult Function( FeederReply_Update value)?  update,TResult Function( FeederReply_Refused value)?  refused,TResult Function( FeederReply_NotAsked value)?  notAsked,TResult Function( FeederReply_Unanswered value)?  unanswered,TResult Function( FeederReply_NotSent value)?  notSent,TResult Function( FeederReply_Unexpected value)?  unexpected,required TResult orElse(),}){
final _that = this;
switch (_that) {
case FeederReply_Done() when done != null:
return done(_that);case FeederReply_Settings() when settings != null:
return settings(_that);case FeederReply_Time() when time != null:
return time(_that);case FeederReply_Plans() when plans != null:
return plans(_that);case FeederReply_Clients() when clients != null:
return clients(_that);case FeederReply_Invite() when invite != null:
return invite(_that);case FeederReply_Calibration() when calibration != null:
return calibration(_that);case FeederReply_Call() when call != null:
return call(_that);case FeederReply_Stats() when stats != null:
return stats(_that);case FeederReply_Trace() when trace != null:
return trace(_that);case FeederReply_Update() when update != null:
return update(_that);case FeederReply_Refused() when refused != null:
return refused(_that);case FeederReply_NotAsked() when notAsked != null:
return notAsked(_that);case FeederReply_Unanswered() when unanswered != null:
return unanswered(_that);case FeederReply_NotSent() when notSent != null:
return notSent(_that);case FeederReply_Unexpected() when unexpected != null:
return unexpected(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( FeederReply_Done value)  done,required TResult Function( FeederReply_Settings value)  settings,required TResult Function( FeederReply_Time value)  time,required TResult Function( FeederReply_Plans value)  plans,required TResult Function( FeederReply_Clients value)  clients,required TResult Function( FeederReply_Invite value)  invite,required TResult Function( FeederReply_Calibration value)  calibration,required TResult Function( FeederReply_Call value)  call,required TResult Function( FeederReply_Stats value)  stats,required TResult Function( FeederReply_Trace value)  trace,required TResult Function( FeederReply_Update value)  update,required TResult Function( FeederReply_Refused value)  refused,required TResult Function( FeederReply_NotAsked value)  notAsked,required TResult Function( FeederReply_Unanswered value)  unanswered,required TResult Function( FeederReply_NotSent value)  notSent,required TResult Function( FeederReply_Unexpected value)  unexpected,}){
final _that = this;
switch (_that) {
case FeederReply_Done():
return done(_that);case FeederReply_Settings():
return settings(_that);case FeederReply_Time():
return time(_that);case FeederReply_Plans():
return plans(_that);case FeederReply_Clients():
return clients(_that);case FeederReply_Invite():
return invite(_that);case FeederReply_Calibration():
return calibration(_that);case FeederReply_Call():
return call(_that);case FeederReply_Stats():
return stats(_that);case FeederReply_Trace():
return trace(_that);case FeederReply_Update():
return update(_that);case FeederReply_Refused():
return refused(_that);case FeederReply_NotAsked():
return notAsked(_that);case FeederReply_Unanswered():
return unanswered(_that);case FeederReply_NotSent():
return notSent(_that);case FeederReply_Unexpected():
return unexpected(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( FeederReply_Done value)?  done,TResult? Function( FeederReply_Settings value)?  settings,TResult? Function( FeederReply_Time value)?  time,TResult? Function( FeederReply_Plans value)?  plans,TResult? Function( FeederReply_Clients value)?  clients,TResult? Function( FeederReply_Invite value)?  invite,TResult? Function( FeederReply_Calibration value)?  calibration,TResult? Function( FeederReply_Call value)?  call,TResult? Function( FeederReply_Stats value)?  stats,TResult? Function( FeederReply_Trace value)?  trace,TResult? Function( FeederReply_Update value)?  update,TResult? Function( FeederReply_Refused value)?  refused,TResult? Function( FeederReply_NotAsked value)?  notAsked,TResult? Function( FeederReply_Unanswered value)?  unanswered,TResult? Function( FeederReply_NotSent value)?  notSent,TResult? Function( FeederReply_Unexpected value)?  unexpected,}){
final _that = this;
switch (_that) {
case FeederReply_Done() when done != null:
return done(_that);case FeederReply_Settings() when settings != null:
return settings(_that);case FeederReply_Time() when time != null:
return time(_that);case FeederReply_Plans() when plans != null:
return plans(_that);case FeederReply_Clients() when clients != null:
return clients(_that);case FeederReply_Invite() when invite != null:
return invite(_that);case FeederReply_Calibration() when calibration != null:
return calibration(_that);case FeederReply_Call() when call != null:
return call(_that);case FeederReply_Stats() when stats != null:
return stats(_that);case FeederReply_Trace() when trace != null:
return trace(_that);case FeederReply_Update() when update != null:
return update(_that);case FeederReply_Refused() when refused != null:
return refused(_that);case FeederReply_NotAsked() when notAsked != null:
return notAsked(_that);case FeederReply_Unanswered() when unanswered != null:
return unanswered(_that);case FeederReply_NotSent() when notSent != null:
return notSent(_that);case FeederReply_Unexpected() when unexpected != null:
return unexpected(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( int? generation)?  done,TResult Function( SettingsView settings)?  settings,TResult Function( TimeInfo time)?  time,TResult Function( PlansView plans)?  plans,TResult Function( List<ClientView> clients)?  clients,TResult Function( InviteView invite)?  invite,TResult Function( CalibrationInfo calibration)?  calibration,TResult Function( CallInfo call)?  call,TResult Function( Stats stats)?  stats,TResult Function( TraceView trace)?  trace,TResult Function( UpdateInfo update)?  update,TResult Function( FeederRefusal refusal)?  refused,TResult Function( NotAsked reason)?  notAsked,TResult Function()?  unanswered,TResult Function()?  notSent,TResult Function()?  unexpected,required TResult orElse(),}) {final _that = this;
switch (_that) {
case FeederReply_Done() when done != null:
return done(_that.generation);case FeederReply_Settings() when settings != null:
return settings(_that.settings);case FeederReply_Time() when time != null:
return time(_that.time);case FeederReply_Plans() when plans != null:
return plans(_that.plans);case FeederReply_Clients() when clients != null:
return clients(_that.clients);case FeederReply_Invite() when invite != null:
return invite(_that.invite);case FeederReply_Calibration() when calibration != null:
return calibration(_that.calibration);case FeederReply_Call() when call != null:
return call(_that.call);case FeederReply_Stats() when stats != null:
return stats(_that.stats);case FeederReply_Trace() when trace != null:
return trace(_that.trace);case FeederReply_Update() when update != null:
return update(_that.update);case FeederReply_Refused() when refused != null:
return refused(_that.refusal);case FeederReply_NotAsked() when notAsked != null:
return notAsked(_that.reason);case FeederReply_Unanswered() when unanswered != null:
return unanswered();case FeederReply_NotSent() when notSent != null:
return notSent();case FeederReply_Unexpected() when unexpected != null:
return unexpected();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( int? generation)  done,required TResult Function( SettingsView settings)  settings,required TResult Function( TimeInfo time)  time,required TResult Function( PlansView plans)  plans,required TResult Function( List<ClientView> clients)  clients,required TResult Function( InviteView invite)  invite,required TResult Function( CalibrationInfo calibration)  calibration,required TResult Function( CallInfo call)  call,required TResult Function( Stats stats)  stats,required TResult Function( TraceView trace)  trace,required TResult Function( UpdateInfo update)  update,required TResult Function( FeederRefusal refusal)  refused,required TResult Function( NotAsked reason)  notAsked,required TResult Function()  unanswered,required TResult Function()  notSent,required TResult Function()  unexpected,}) {final _that = this;
switch (_that) {
case FeederReply_Done():
return done(_that.generation);case FeederReply_Settings():
return settings(_that.settings);case FeederReply_Time():
return time(_that.time);case FeederReply_Plans():
return plans(_that.plans);case FeederReply_Clients():
return clients(_that.clients);case FeederReply_Invite():
return invite(_that.invite);case FeederReply_Calibration():
return calibration(_that.calibration);case FeederReply_Call():
return call(_that.call);case FeederReply_Stats():
return stats(_that.stats);case FeederReply_Trace():
return trace(_that.trace);case FeederReply_Update():
return update(_that.update);case FeederReply_Refused():
return refused(_that.refusal);case FeederReply_NotAsked():
return notAsked(_that.reason);case FeederReply_Unanswered():
return unanswered();case FeederReply_NotSent():
return notSent();case FeederReply_Unexpected():
return unexpected();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( int? generation)?  done,TResult? Function( SettingsView settings)?  settings,TResult? Function( TimeInfo time)?  time,TResult? Function( PlansView plans)?  plans,TResult? Function( List<ClientView> clients)?  clients,TResult? Function( InviteView invite)?  invite,TResult? Function( CalibrationInfo calibration)?  calibration,TResult? Function( CallInfo call)?  call,TResult? Function( Stats stats)?  stats,TResult? Function( TraceView trace)?  trace,TResult? Function( UpdateInfo update)?  update,TResult? Function( FeederRefusal refusal)?  refused,TResult? Function( NotAsked reason)?  notAsked,TResult? Function()?  unanswered,TResult? Function()?  notSent,TResult? Function()?  unexpected,}) {final _that = this;
switch (_that) {
case FeederReply_Done() when done != null:
return done(_that.generation);case FeederReply_Settings() when settings != null:
return settings(_that.settings);case FeederReply_Time() when time != null:
return time(_that.time);case FeederReply_Plans() when plans != null:
return plans(_that.plans);case FeederReply_Clients() when clients != null:
return clients(_that.clients);case FeederReply_Invite() when invite != null:
return invite(_that.invite);case FeederReply_Calibration() when calibration != null:
return calibration(_that.calibration);case FeederReply_Call() when call != null:
return call(_that.call);case FeederReply_Stats() when stats != null:
return stats(_that.stats);case FeederReply_Trace() when trace != null:
return trace(_that.trace);case FeederReply_Update() when update != null:
return update(_that.update);case FeederReply_Refused() when refused != null:
return refused(_that.refusal);case FeederReply_NotAsked() when notAsked != null:
return notAsked(_that.reason);case FeederReply_Unanswered() when unanswered != null:
return unanswered();case FeederReply_NotSent() when notSent != null:
return notSent();case FeederReply_Unexpected() when unexpected != null:
return unexpected();case _:
  return null;

}
}

}

/// @nodoc


class FeederReply_Done extends FeederReply {
  const FeederReply_Done({this.generation}): super._();
  

 final  int? generation;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_DoneCopyWith<FeederReply_Done> get copyWith => _$FeederReply_DoneCopyWithImpl<FeederReply_Done>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Done&&(identical(other.generation, generation) || other.generation == generation));
}


@override
int get hashCode {
    return Object.hash(runtimeType,generation);
}

@override
String toString() {
    return 'FeederReply.done(generation: $generation)';
}


}

/// @nodoc
abstract mixin class $FeederReply_DoneCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_DoneCopyWith(FeederReply_Done value, $Res Function(FeederReply_Done) _then) = _$FeederReply_DoneCopyWithImpl;
@useResult
$Res call({
 int? generation
});




}
/// @nodoc
class _$FeederReply_DoneCopyWithImpl<$Res>
    implements $FeederReply_DoneCopyWith<$Res> {
  _$FeederReply_DoneCopyWithImpl(this._self, this._then);

  final FeederReply_Done _self;
  final $Res Function(FeederReply_Done) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? generation = freezed,}) {
  return _then(FeederReply_Done(
generation: freezed == generation ? _self.generation : generation // ignore: cast_nullable_to_non_nullable
as int?,
  ));
}


}

/// @nodoc


class FeederReply_Settings extends FeederReply {
  const FeederReply_Settings({required this.settings}): super._();
  

 final  SettingsView settings;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_SettingsCopyWith<FeederReply_Settings> get copyWith => _$FeederReply_SettingsCopyWithImpl<FeederReply_Settings>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Settings&&(identical(other.settings, settings) || other.settings == settings));
}


@override
int get hashCode {
    return Object.hash(runtimeType,settings);
}

@override
String toString() {
    return 'FeederReply.settings(settings: $settings)';
}


}

/// @nodoc
abstract mixin class $FeederReply_SettingsCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_SettingsCopyWith(FeederReply_Settings value, $Res Function(FeederReply_Settings) _then) = _$FeederReply_SettingsCopyWithImpl;
@useResult
$Res call({
 SettingsView settings
});




}
/// @nodoc
class _$FeederReply_SettingsCopyWithImpl<$Res>
    implements $FeederReply_SettingsCopyWith<$Res> {
  _$FeederReply_SettingsCopyWithImpl(this._self, this._then);

  final FeederReply_Settings _self;
  final $Res Function(FeederReply_Settings) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? settings = null,}) {
  return _then(FeederReply_Settings(
settings: null == settings ? _self.settings : settings // ignore: cast_nullable_to_non_nullable
as SettingsView,
  ));
}


}

/// @nodoc


class FeederReply_Time extends FeederReply {
  const FeederReply_Time({required this.time}): super._();
  

 final  TimeInfo time;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_TimeCopyWith<FeederReply_Time> get copyWith => _$FeederReply_TimeCopyWithImpl<FeederReply_Time>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Time&&(identical(other.time, time) || other.time == time));
}


@override
int get hashCode {
    return Object.hash(runtimeType,time);
}

@override
String toString() {
    return 'FeederReply.time(time: $time)';
}


}

/// @nodoc
abstract mixin class $FeederReply_TimeCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_TimeCopyWith(FeederReply_Time value, $Res Function(FeederReply_Time) _then) = _$FeederReply_TimeCopyWithImpl;
@useResult
$Res call({
 TimeInfo time
});




}
/// @nodoc
class _$FeederReply_TimeCopyWithImpl<$Res>
    implements $FeederReply_TimeCopyWith<$Res> {
  _$FeederReply_TimeCopyWithImpl(this._self, this._then);

  final FeederReply_Time _self;
  final $Res Function(FeederReply_Time) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? time = null,}) {
  return _then(FeederReply_Time(
time: null == time ? _self.time : time // ignore: cast_nullable_to_non_nullable
as TimeInfo,
  ));
}


}

/// @nodoc


class FeederReply_Plans extends FeederReply {
  const FeederReply_Plans({required this.plans}): super._();
  

 final  PlansView plans;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_PlansCopyWith<FeederReply_Plans> get copyWith => _$FeederReply_PlansCopyWithImpl<FeederReply_Plans>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Plans&&(identical(other.plans, plans) || other.plans == plans));
}


@override
int get hashCode {
    return Object.hash(runtimeType,plans);
}

@override
String toString() {
    return 'FeederReply.plans(plans: $plans)';
}


}

/// @nodoc
abstract mixin class $FeederReply_PlansCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_PlansCopyWith(FeederReply_Plans value, $Res Function(FeederReply_Plans) _then) = _$FeederReply_PlansCopyWithImpl;
@useResult
$Res call({
 PlansView plans
});




}
/// @nodoc
class _$FeederReply_PlansCopyWithImpl<$Res>
    implements $FeederReply_PlansCopyWith<$Res> {
  _$FeederReply_PlansCopyWithImpl(this._self, this._then);

  final FeederReply_Plans _self;
  final $Res Function(FeederReply_Plans) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? plans = null,}) {
  return _then(FeederReply_Plans(
plans: null == plans ? _self.plans : plans // ignore: cast_nullable_to_non_nullable
as PlansView,
  ));
}


}

/// @nodoc


class FeederReply_Clients extends FeederReply {
  const FeederReply_Clients({required  List<ClientView> clients}): _clients = clients,super._();
  

 final  List<ClientView> _clients;
 List<ClientView> get clients {
  if (_clients is EqualUnmodifiableListView) return _clients;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_clients);
}


/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_ClientsCopyWith<FeederReply_Clients> get copyWith => _$FeederReply_ClientsCopyWithImpl<FeederReply_Clients>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Clients&&const DeepCollectionEquality().equals(other.clients, _clients));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_clients));
}

@override
String toString() {
    return 'FeederReply.clients(clients: $clients)';
}


}

/// @nodoc
abstract mixin class $FeederReply_ClientsCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_ClientsCopyWith(FeederReply_Clients value, $Res Function(FeederReply_Clients) _then) = _$FeederReply_ClientsCopyWithImpl;
@useResult
$Res call({
 List<ClientView> clients
});




}
/// @nodoc
class _$FeederReply_ClientsCopyWithImpl<$Res>
    implements $FeederReply_ClientsCopyWith<$Res> {
  _$FeederReply_ClientsCopyWithImpl(this._self, this._then);

  final FeederReply_Clients _self;
  final $Res Function(FeederReply_Clients) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? clients = null,}) {
  return _then(FeederReply_Clients(
clients: null == clients ? _self._clients : clients // ignore: cast_nullable_to_non_nullable
as List<ClientView>,
  ));
}


}

/// @nodoc


class FeederReply_Invite extends FeederReply {
  const FeederReply_Invite({required this.invite}): super._();
  

 final  InviteView invite;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_InviteCopyWith<FeederReply_Invite> get copyWith => _$FeederReply_InviteCopyWithImpl<FeederReply_Invite>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Invite&&(identical(other.invite, invite) || other.invite == invite));
}


@override
int get hashCode {
    return Object.hash(runtimeType,invite);
}

@override
String toString() {
    return 'FeederReply.invite(invite: $invite)';
}


}

/// @nodoc
abstract mixin class $FeederReply_InviteCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_InviteCopyWith(FeederReply_Invite value, $Res Function(FeederReply_Invite) _then) = _$FeederReply_InviteCopyWithImpl;
@useResult
$Res call({
 InviteView invite
});




}
/// @nodoc
class _$FeederReply_InviteCopyWithImpl<$Res>
    implements $FeederReply_InviteCopyWith<$Res> {
  _$FeederReply_InviteCopyWithImpl(this._self, this._then);

  final FeederReply_Invite _self;
  final $Res Function(FeederReply_Invite) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? invite = null,}) {
  return _then(FeederReply_Invite(
invite: null == invite ? _self.invite : invite // ignore: cast_nullable_to_non_nullable
as InviteView,
  ));
}


}

/// @nodoc


class FeederReply_Calibration extends FeederReply {
  const FeederReply_Calibration({required this.calibration}): super._();
  

 final  CalibrationInfo calibration;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_CalibrationCopyWith<FeederReply_Calibration> get copyWith => _$FeederReply_CalibrationCopyWithImpl<FeederReply_Calibration>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Calibration&&(identical(other.calibration, calibration) || other.calibration == calibration));
}


@override
int get hashCode {
    return Object.hash(runtimeType,calibration);
}

@override
String toString() {
    return 'FeederReply.calibration(calibration: $calibration)';
}


}

/// @nodoc
abstract mixin class $FeederReply_CalibrationCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_CalibrationCopyWith(FeederReply_Calibration value, $Res Function(FeederReply_Calibration) _then) = _$FeederReply_CalibrationCopyWithImpl;
@useResult
$Res call({
 CalibrationInfo calibration
});




}
/// @nodoc
class _$FeederReply_CalibrationCopyWithImpl<$Res>
    implements $FeederReply_CalibrationCopyWith<$Res> {
  _$FeederReply_CalibrationCopyWithImpl(this._self, this._then);

  final FeederReply_Calibration _self;
  final $Res Function(FeederReply_Calibration) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? calibration = null,}) {
  return _then(FeederReply_Calibration(
calibration: null == calibration ? _self.calibration : calibration // ignore: cast_nullable_to_non_nullable
as CalibrationInfo,
  ));
}


}

/// @nodoc


class FeederReply_Call extends FeederReply {
  const FeederReply_Call({required this.call}): super._();
  

 final  CallInfo call;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_CallCopyWith<FeederReply_Call> get copyWith => _$FeederReply_CallCopyWithImpl<FeederReply_Call>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Call&&(identical(other.call, call) || other.call == call));
}


@override
int get hashCode {
    return Object.hash(runtimeType,call);
}

@override
String toString() {
    return 'FeederReply.call(call: $call)';
}


}

/// @nodoc
abstract mixin class $FeederReply_CallCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_CallCopyWith(FeederReply_Call value, $Res Function(FeederReply_Call) _then) = _$FeederReply_CallCopyWithImpl;
@useResult
$Res call({
 CallInfo call
});




}
/// @nodoc
class _$FeederReply_CallCopyWithImpl<$Res>
    implements $FeederReply_CallCopyWith<$Res> {
  _$FeederReply_CallCopyWithImpl(this._self, this._then);

  final FeederReply_Call _self;
  final $Res Function(FeederReply_Call) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? call = null,}) {
  return _then(FeederReply_Call(
call: null == call ? _self.call : call // ignore: cast_nullable_to_non_nullable
as CallInfo,
  ));
}


}

/// @nodoc


class FeederReply_Stats extends FeederReply {
  const FeederReply_Stats({required this.stats}): super._();
  

 final  Stats stats;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_StatsCopyWith<FeederReply_Stats> get copyWith => _$FeederReply_StatsCopyWithImpl<FeederReply_Stats>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Stats&&(identical(other.stats, stats) || other.stats == stats));
}


@override
int get hashCode {
    return Object.hash(runtimeType,stats);
}

@override
String toString() {
    return 'FeederReply.stats(stats: $stats)';
}


}

/// @nodoc
abstract mixin class $FeederReply_StatsCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_StatsCopyWith(FeederReply_Stats value, $Res Function(FeederReply_Stats) _then) = _$FeederReply_StatsCopyWithImpl;
@useResult
$Res call({
 Stats stats
});


$StatsCopyWith<$Res> get stats;

}
/// @nodoc
class _$FeederReply_StatsCopyWithImpl<$Res>
    implements $FeederReply_StatsCopyWith<$Res> {
  _$FeederReply_StatsCopyWithImpl(this._self, this._then);

  final FeederReply_Stats _self;
  final $Res Function(FeederReply_Stats) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? stats = null,}) {
  return _then(FeederReply_Stats(
stats: null == stats ? _self.stats : stats // ignore: cast_nullable_to_non_nullable
as Stats,
  ));
}

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$StatsCopyWith<$Res> get stats {
  
  return $StatsCopyWith<$Res>(_self.stats, (value) {
    return _then(_self.copyWith(stats: value));
  });
}
}

/// @nodoc


class FeederReply_Trace extends FeederReply {
  const FeederReply_Trace({required this.trace}): super._();
  

 final  TraceView trace;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_TraceCopyWith<FeederReply_Trace> get copyWith => _$FeederReply_TraceCopyWithImpl<FeederReply_Trace>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Trace&&(identical(other.trace, trace) || other.trace == trace));
}


@override
int get hashCode {
    return Object.hash(runtimeType,trace);
}

@override
String toString() {
    return 'FeederReply.trace(trace: $trace)';
}


}

/// @nodoc
abstract mixin class $FeederReply_TraceCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_TraceCopyWith(FeederReply_Trace value, $Res Function(FeederReply_Trace) _then) = _$FeederReply_TraceCopyWithImpl;
@useResult
$Res call({
 TraceView trace
});




}
/// @nodoc
class _$FeederReply_TraceCopyWithImpl<$Res>
    implements $FeederReply_TraceCopyWith<$Res> {
  _$FeederReply_TraceCopyWithImpl(this._self, this._then);

  final FeederReply_Trace _self;
  final $Res Function(FeederReply_Trace) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? trace = null,}) {
  return _then(FeederReply_Trace(
trace: null == trace ? _self.trace : trace // ignore: cast_nullable_to_non_nullable
as TraceView,
  ));
}


}

/// @nodoc


class FeederReply_Update extends FeederReply {
  const FeederReply_Update({required this.update}): super._();
  

 final  UpdateInfo update;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_UpdateCopyWith<FeederReply_Update> get copyWith => _$FeederReply_UpdateCopyWithImpl<FeederReply_Update>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Update&&(identical(other.update, update) || other.update == update));
}


@override
int get hashCode {
    return Object.hash(runtimeType,update);
}

@override
String toString() {
    return 'FeederReply.update(update: $update)';
}


}

/// @nodoc
abstract mixin class $FeederReply_UpdateCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_UpdateCopyWith(FeederReply_Update value, $Res Function(FeederReply_Update) _then) = _$FeederReply_UpdateCopyWithImpl;
@useResult
$Res call({
 UpdateInfo update
});




}
/// @nodoc
class _$FeederReply_UpdateCopyWithImpl<$Res>
    implements $FeederReply_UpdateCopyWith<$Res> {
  _$FeederReply_UpdateCopyWithImpl(this._self, this._then);

  final FeederReply_Update _self;
  final $Res Function(FeederReply_Update) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? update = null,}) {
  return _then(FeederReply_Update(
update: null == update ? _self.update : update // ignore: cast_nullable_to_non_nullable
as UpdateInfo,
  ));
}


}

/// @nodoc


class FeederReply_Refused extends FeederReply {
  const FeederReply_Refused({required this.refusal}): super._();
  

 final  FeederRefusal refusal;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_RefusedCopyWith<FeederReply_Refused> get copyWith => _$FeederReply_RefusedCopyWithImpl<FeederReply_Refused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Refused&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'FeederReply.refused(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $FeederReply_RefusedCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_RefusedCopyWith(FeederReply_Refused value, $Res Function(FeederReply_Refused) _then) = _$FeederReply_RefusedCopyWithImpl;
@useResult
$Res call({
 FeederRefusal refusal
});


$FeederRefusalCopyWith<$Res> get refusal;

}
/// @nodoc
class _$FeederReply_RefusedCopyWithImpl<$Res>
    implements $FeederReply_RefusedCopyWith<$Res> {
  _$FeederReply_RefusedCopyWithImpl(this._self, this._then);

  final FeederReply_Refused _self;
  final $Res Function(FeederReply_Refused) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = null,}) {
  return _then(FeederReply_Refused(
refusal: null == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as FeederRefusal,
  ));
}

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FeederRefusalCopyWith<$Res> get refusal {
  
  return $FeederRefusalCopyWith<$Res>(_self.refusal, (value) {
    return _then(_self.copyWith(refusal: value));
  });
}
}

/// @nodoc


class FeederReply_NotAsked extends FeederReply {
  const FeederReply_NotAsked({required this.reason}): super._();
  

 final  NotAsked reason;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederReply_NotAskedCopyWith<FeederReply_NotAsked> get copyWith => _$FeederReply_NotAskedCopyWithImpl<FeederReply_NotAsked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_NotAsked&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'FeederReply.notAsked(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $FeederReply_NotAskedCopyWith<$Res> implements $FeederReplyCopyWith<$Res> {
  factory $FeederReply_NotAskedCopyWith(FeederReply_NotAsked value, $Res Function(FeederReply_NotAsked) _then) = _$FeederReply_NotAskedCopyWithImpl;
@useResult
$Res call({
 NotAsked reason
});




}
/// @nodoc
class _$FeederReply_NotAskedCopyWithImpl<$Res>
    implements $FeederReply_NotAskedCopyWith<$Res> {
  _$FeederReply_NotAskedCopyWithImpl(this._self, this._then);

  final FeederReply_NotAsked _self;
  final $Res Function(FeederReply_NotAsked) _then;

/// Create a copy of FeederReply
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(FeederReply_NotAsked(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as NotAsked,
  ));
}


}

/// @nodoc


class FeederReply_Unanswered extends FeederReply {
  const FeederReply_Unanswered(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Unanswered);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederReply.unanswered()';
}


}




/// @nodoc


class FeederReply_NotSent extends FeederReply {
  const FeederReply_NotSent(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_NotSent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederReply.notSent()';
}


}




/// @nodoc


class FeederReply_Unexpected extends FeederReply {
  const FeederReply_Unexpected(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederReply_Unexpected);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederReply.unexpected()';
}


}




/// @nodoc
mixin _$FeederRequest {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest()';
}


}

/// @nodoc
class $FeederRequestCopyWith<$Res>  {
$FeederRequestCopyWith(FeederRequest _, $Res Function(FeederRequest) __);
}


/// Adds pattern-matching-related methods to [FeederRequest].
extension FeederRequestPatterns on FeederRequest {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( FeederRequest_GetSettings value)?  getSettings,TResult Function( FeederRequest_PutSettings value)?  putSettings,TResult Function( FeederRequest_PutWifi value)?  putWifi,TResult Function( FeederRequest_ForgetWifi value)?  forgetWifi,TResult Function( FeederRequest_SetLock value)?  setLock,TResult Function( FeederRequest_Play value)?  play,TResult Function( FeederRequest_SyncClock value)?  syncClock,TResult Function( FeederRequest_GetTime value)?  getTime,TResult Function( FeederRequest_PutTime value)?  putTime,TResult Function( FeederRequest_CreateInvite value)?  createInvite,TResult Function( FeederRequest_ListClients value)?  listClients,TResult Function( FeederRequest_RevokeClient value)?  revokeClient,TResult Function( FeederRequest_Reboot value)?  reboot,TResult Function( FeederRequest_ResetDesiccant value)?  resetDesiccant,TResult Function( FeederRequest_Stats value)?  stats,TResult Function( FeederRequest_ReadTrace value)?  readTrace,TResult Function( FeederRequest_GetCalibration value)?  getCalibration,TResult Function( FeederRequest_PutCalibration value)?  putCalibration,TResult Function( FeederRequest_GetPlans value)?  getPlans,TResult Function( FeederRequest_EditPlans value)?  editPlans,TResult Function( FeederRequest_GetCall value)?  getCall,TResult Function( FeederRequest_UploadCall value)?  uploadCall,TResult Function( FeederRequest_DeleteCall value)?  deleteCall,TResult Function( FeederRequest_UpdateState value)?  updateState,TResult Function( FeederRequest_CancelUpdate value)?  cancelUpdate,TResult Function( FeederRequest_TailLog value)?  tailLog,required TResult orElse(),}){
final _that = this;
switch (_that) {
case FeederRequest_GetSettings() when getSettings != null:
return getSettings(_that);case FeederRequest_PutSettings() when putSettings != null:
return putSettings(_that);case FeederRequest_PutWifi() when putWifi != null:
return putWifi(_that);case FeederRequest_ForgetWifi() when forgetWifi != null:
return forgetWifi(_that);case FeederRequest_SetLock() when setLock != null:
return setLock(_that);case FeederRequest_Play() when play != null:
return play(_that);case FeederRequest_SyncClock() when syncClock != null:
return syncClock(_that);case FeederRequest_GetTime() when getTime != null:
return getTime(_that);case FeederRequest_PutTime() when putTime != null:
return putTime(_that);case FeederRequest_CreateInvite() when createInvite != null:
return createInvite(_that);case FeederRequest_ListClients() when listClients != null:
return listClients(_that);case FeederRequest_RevokeClient() when revokeClient != null:
return revokeClient(_that);case FeederRequest_Reboot() when reboot != null:
return reboot(_that);case FeederRequest_ResetDesiccant() when resetDesiccant != null:
return resetDesiccant(_that);case FeederRequest_Stats() when stats != null:
return stats(_that);case FeederRequest_ReadTrace() when readTrace != null:
return readTrace(_that);case FeederRequest_GetCalibration() when getCalibration != null:
return getCalibration(_that);case FeederRequest_PutCalibration() when putCalibration != null:
return putCalibration(_that);case FeederRequest_GetPlans() when getPlans != null:
return getPlans(_that);case FeederRequest_EditPlans() when editPlans != null:
return editPlans(_that);case FeederRequest_GetCall() when getCall != null:
return getCall(_that);case FeederRequest_UploadCall() when uploadCall != null:
return uploadCall(_that);case FeederRequest_DeleteCall() when deleteCall != null:
return deleteCall(_that);case FeederRequest_UpdateState() when updateState != null:
return updateState(_that);case FeederRequest_CancelUpdate() when cancelUpdate != null:
return cancelUpdate(_that);case FeederRequest_TailLog() when tailLog != null:
return tailLog(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( FeederRequest_GetSettings value)  getSettings,required TResult Function( FeederRequest_PutSettings value)  putSettings,required TResult Function( FeederRequest_PutWifi value)  putWifi,required TResult Function( FeederRequest_ForgetWifi value)  forgetWifi,required TResult Function( FeederRequest_SetLock value)  setLock,required TResult Function( FeederRequest_Play value)  play,required TResult Function( FeederRequest_SyncClock value)  syncClock,required TResult Function( FeederRequest_GetTime value)  getTime,required TResult Function( FeederRequest_PutTime value)  putTime,required TResult Function( FeederRequest_CreateInvite value)  createInvite,required TResult Function( FeederRequest_ListClients value)  listClients,required TResult Function( FeederRequest_RevokeClient value)  revokeClient,required TResult Function( FeederRequest_Reboot value)  reboot,required TResult Function( FeederRequest_ResetDesiccant value)  resetDesiccant,required TResult Function( FeederRequest_Stats value)  stats,required TResult Function( FeederRequest_ReadTrace value)  readTrace,required TResult Function( FeederRequest_GetCalibration value)  getCalibration,required TResult Function( FeederRequest_PutCalibration value)  putCalibration,required TResult Function( FeederRequest_GetPlans value)  getPlans,required TResult Function( FeederRequest_EditPlans value)  editPlans,required TResult Function( FeederRequest_GetCall value)  getCall,required TResult Function( FeederRequest_UploadCall value)  uploadCall,required TResult Function( FeederRequest_DeleteCall value)  deleteCall,required TResult Function( FeederRequest_UpdateState value)  updateState,required TResult Function( FeederRequest_CancelUpdate value)  cancelUpdate,required TResult Function( FeederRequest_TailLog value)  tailLog,}){
final _that = this;
switch (_that) {
case FeederRequest_GetSettings():
return getSettings(_that);case FeederRequest_PutSettings():
return putSettings(_that);case FeederRequest_PutWifi():
return putWifi(_that);case FeederRequest_ForgetWifi():
return forgetWifi(_that);case FeederRequest_SetLock():
return setLock(_that);case FeederRequest_Play():
return play(_that);case FeederRequest_SyncClock():
return syncClock(_that);case FeederRequest_GetTime():
return getTime(_that);case FeederRequest_PutTime():
return putTime(_that);case FeederRequest_CreateInvite():
return createInvite(_that);case FeederRequest_ListClients():
return listClients(_that);case FeederRequest_RevokeClient():
return revokeClient(_that);case FeederRequest_Reboot():
return reboot(_that);case FeederRequest_ResetDesiccant():
return resetDesiccant(_that);case FeederRequest_Stats():
return stats(_that);case FeederRequest_ReadTrace():
return readTrace(_that);case FeederRequest_GetCalibration():
return getCalibration(_that);case FeederRequest_PutCalibration():
return putCalibration(_that);case FeederRequest_GetPlans():
return getPlans(_that);case FeederRequest_EditPlans():
return editPlans(_that);case FeederRequest_GetCall():
return getCall(_that);case FeederRequest_UploadCall():
return uploadCall(_that);case FeederRequest_DeleteCall():
return deleteCall(_that);case FeederRequest_UpdateState():
return updateState(_that);case FeederRequest_CancelUpdate():
return cancelUpdate(_that);case FeederRequest_TailLog():
return tailLog(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( FeederRequest_GetSettings value)?  getSettings,TResult? Function( FeederRequest_PutSettings value)?  putSettings,TResult? Function( FeederRequest_PutWifi value)?  putWifi,TResult? Function( FeederRequest_ForgetWifi value)?  forgetWifi,TResult? Function( FeederRequest_SetLock value)?  setLock,TResult? Function( FeederRequest_Play value)?  play,TResult? Function( FeederRequest_SyncClock value)?  syncClock,TResult? Function( FeederRequest_GetTime value)?  getTime,TResult? Function( FeederRequest_PutTime value)?  putTime,TResult? Function( FeederRequest_CreateInvite value)?  createInvite,TResult? Function( FeederRequest_ListClients value)?  listClients,TResult? Function( FeederRequest_RevokeClient value)?  revokeClient,TResult? Function( FeederRequest_Reboot value)?  reboot,TResult? Function( FeederRequest_ResetDesiccant value)?  resetDesiccant,TResult? Function( FeederRequest_Stats value)?  stats,TResult? Function( FeederRequest_ReadTrace value)?  readTrace,TResult? Function( FeederRequest_GetCalibration value)?  getCalibration,TResult? Function( FeederRequest_PutCalibration value)?  putCalibration,TResult? Function( FeederRequest_GetPlans value)?  getPlans,TResult? Function( FeederRequest_EditPlans value)?  editPlans,TResult? Function( FeederRequest_GetCall value)?  getCall,TResult? Function( FeederRequest_UploadCall value)?  uploadCall,TResult? Function( FeederRequest_DeleteCall value)?  deleteCall,TResult? Function( FeederRequest_UpdateState value)?  updateState,TResult? Function( FeederRequest_CancelUpdate value)?  cancelUpdate,TResult? Function( FeederRequest_TailLog value)?  tailLog,}){
final _that = this;
switch (_that) {
case FeederRequest_GetSettings() when getSettings != null:
return getSettings(_that);case FeederRequest_PutSettings() when putSettings != null:
return putSettings(_that);case FeederRequest_PutWifi() when putWifi != null:
return putWifi(_that);case FeederRequest_ForgetWifi() when forgetWifi != null:
return forgetWifi(_that);case FeederRequest_SetLock() when setLock != null:
return setLock(_that);case FeederRequest_Play() when play != null:
return play(_that);case FeederRequest_SyncClock() when syncClock != null:
return syncClock(_that);case FeederRequest_GetTime() when getTime != null:
return getTime(_that);case FeederRequest_PutTime() when putTime != null:
return putTime(_that);case FeederRequest_CreateInvite() when createInvite != null:
return createInvite(_that);case FeederRequest_ListClients() when listClients != null:
return listClients(_that);case FeederRequest_RevokeClient() when revokeClient != null:
return revokeClient(_that);case FeederRequest_Reboot() when reboot != null:
return reboot(_that);case FeederRequest_ResetDesiccant() when resetDesiccant != null:
return resetDesiccant(_that);case FeederRequest_Stats() when stats != null:
return stats(_that);case FeederRequest_ReadTrace() when readTrace != null:
return readTrace(_that);case FeederRequest_GetCalibration() when getCalibration != null:
return getCalibration(_that);case FeederRequest_PutCalibration() when putCalibration != null:
return putCalibration(_that);case FeederRequest_GetPlans() when getPlans != null:
return getPlans(_that);case FeederRequest_EditPlans() when editPlans != null:
return editPlans(_that);case FeederRequest_GetCall() when getCall != null:
return getCall(_that);case FeederRequest_UploadCall() when uploadCall != null:
return uploadCall(_that);case FeederRequest_DeleteCall() when deleteCall != null:
return deleteCall(_that);case FeederRequest_UpdateState() when updateState != null:
return updateState(_that);case FeederRequest_CancelUpdate() when cancelUpdate != null:
return cancelUpdate(_that);case FeederRequest_TailLog() when tailLog != null:
return tailLog(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  getSettings,TResult Function( int base,  SettingsChanges changes)?  putSettings,TResult Function( int base,  String ssid,  String passphrase)?  putWifi,TResult Function( int base)?  forgetWifi,TResult Function( bool locked)?  setLock,TResult Function( Tune tune)?  play,TResult Function()?  syncClock,TResult Function()?  getTime,TResult Function( int base,  ZoneChange? zone,  String? ntp)?  putTime,TResult Function( ClientRole role)?  createInvite,TResult Function()?  listClients,TResult Function( Uint8List key)?  revokeClient,TResult Function()?  reboot,TResult Function()?  resetDesiccant,TResult Function( StatsSection section)?  stats,TResult Function( int feed)?  readTrace,TResult Function()?  getCalibration,TResult Function( int base,  PortionChange portion,  Bowl bowl)?  putCalibration,TResult Function()?  getPlans,TResult Function( PlansView plans,  PlanEdit edit)?  editPlans,TResult Function()?  getCall,TResult Function( Uint8List wav)?  uploadCall,TResult Function()?  deleteCall,TResult Function()?  updateState,TResult Function()?  cancelUpdate,TResult Function( int? after)?  tailLog,required TResult orElse(),}) {final _that = this;
switch (_that) {
case FeederRequest_GetSettings() when getSettings != null:
return getSettings();case FeederRequest_PutSettings() when putSettings != null:
return putSettings(_that.base,_that.changes);case FeederRequest_PutWifi() when putWifi != null:
return putWifi(_that.base,_that.ssid,_that.passphrase);case FeederRequest_ForgetWifi() when forgetWifi != null:
return forgetWifi(_that.base);case FeederRequest_SetLock() when setLock != null:
return setLock(_that.locked);case FeederRequest_Play() when play != null:
return play(_that.tune);case FeederRequest_SyncClock() when syncClock != null:
return syncClock();case FeederRequest_GetTime() when getTime != null:
return getTime();case FeederRequest_PutTime() when putTime != null:
return putTime(_that.base,_that.zone,_that.ntp);case FeederRequest_CreateInvite() when createInvite != null:
return createInvite(_that.role);case FeederRequest_ListClients() when listClients != null:
return listClients();case FeederRequest_RevokeClient() when revokeClient != null:
return revokeClient(_that.key);case FeederRequest_Reboot() when reboot != null:
return reboot();case FeederRequest_ResetDesiccant() when resetDesiccant != null:
return resetDesiccant();case FeederRequest_Stats() when stats != null:
return stats(_that.section);case FeederRequest_ReadTrace() when readTrace != null:
return readTrace(_that.feed);case FeederRequest_GetCalibration() when getCalibration != null:
return getCalibration();case FeederRequest_PutCalibration() when putCalibration != null:
return putCalibration(_that.base,_that.portion,_that.bowl);case FeederRequest_GetPlans() when getPlans != null:
return getPlans();case FeederRequest_EditPlans() when editPlans != null:
return editPlans(_that.plans,_that.edit);case FeederRequest_GetCall() when getCall != null:
return getCall();case FeederRequest_UploadCall() when uploadCall != null:
return uploadCall(_that.wav);case FeederRequest_DeleteCall() when deleteCall != null:
return deleteCall();case FeederRequest_UpdateState() when updateState != null:
return updateState();case FeederRequest_CancelUpdate() when cancelUpdate != null:
return cancelUpdate();case FeederRequest_TailLog() when tailLog != null:
return tailLog(_that.after);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  getSettings,required TResult Function( int base,  SettingsChanges changes)  putSettings,required TResult Function( int base,  String ssid,  String passphrase)  putWifi,required TResult Function( int base)  forgetWifi,required TResult Function( bool locked)  setLock,required TResult Function( Tune tune)  play,required TResult Function()  syncClock,required TResult Function()  getTime,required TResult Function( int base,  ZoneChange? zone,  String? ntp)  putTime,required TResult Function( ClientRole role)  createInvite,required TResult Function()  listClients,required TResult Function( Uint8List key)  revokeClient,required TResult Function()  reboot,required TResult Function()  resetDesiccant,required TResult Function( StatsSection section)  stats,required TResult Function( int feed)  readTrace,required TResult Function()  getCalibration,required TResult Function( int base,  PortionChange portion,  Bowl bowl)  putCalibration,required TResult Function()  getPlans,required TResult Function( PlansView plans,  PlanEdit edit)  editPlans,required TResult Function()  getCall,required TResult Function( Uint8List wav)  uploadCall,required TResult Function()  deleteCall,required TResult Function()  updateState,required TResult Function()  cancelUpdate,required TResult Function( int? after)  tailLog,}) {final _that = this;
switch (_that) {
case FeederRequest_GetSettings():
return getSettings();case FeederRequest_PutSettings():
return putSettings(_that.base,_that.changes);case FeederRequest_PutWifi():
return putWifi(_that.base,_that.ssid,_that.passphrase);case FeederRequest_ForgetWifi():
return forgetWifi(_that.base);case FeederRequest_SetLock():
return setLock(_that.locked);case FeederRequest_Play():
return play(_that.tune);case FeederRequest_SyncClock():
return syncClock();case FeederRequest_GetTime():
return getTime();case FeederRequest_PutTime():
return putTime(_that.base,_that.zone,_that.ntp);case FeederRequest_CreateInvite():
return createInvite(_that.role);case FeederRequest_ListClients():
return listClients();case FeederRequest_RevokeClient():
return revokeClient(_that.key);case FeederRequest_Reboot():
return reboot();case FeederRequest_ResetDesiccant():
return resetDesiccant();case FeederRequest_Stats():
return stats(_that.section);case FeederRequest_ReadTrace():
return readTrace(_that.feed);case FeederRequest_GetCalibration():
return getCalibration();case FeederRequest_PutCalibration():
return putCalibration(_that.base,_that.portion,_that.bowl);case FeederRequest_GetPlans():
return getPlans();case FeederRequest_EditPlans():
return editPlans(_that.plans,_that.edit);case FeederRequest_GetCall():
return getCall();case FeederRequest_UploadCall():
return uploadCall(_that.wav);case FeederRequest_DeleteCall():
return deleteCall();case FeederRequest_UpdateState():
return updateState();case FeederRequest_CancelUpdate():
return cancelUpdate();case FeederRequest_TailLog():
return tailLog(_that.after);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  getSettings,TResult? Function( int base,  SettingsChanges changes)?  putSettings,TResult? Function( int base,  String ssid,  String passphrase)?  putWifi,TResult? Function( int base)?  forgetWifi,TResult? Function( bool locked)?  setLock,TResult? Function( Tune tune)?  play,TResult? Function()?  syncClock,TResult? Function()?  getTime,TResult? Function( int base,  ZoneChange? zone,  String? ntp)?  putTime,TResult? Function( ClientRole role)?  createInvite,TResult? Function()?  listClients,TResult? Function( Uint8List key)?  revokeClient,TResult? Function()?  reboot,TResult? Function()?  resetDesiccant,TResult? Function( StatsSection section)?  stats,TResult? Function( int feed)?  readTrace,TResult? Function()?  getCalibration,TResult? Function( int base,  PortionChange portion,  Bowl bowl)?  putCalibration,TResult? Function()?  getPlans,TResult? Function( PlansView plans,  PlanEdit edit)?  editPlans,TResult? Function()?  getCall,TResult? Function( Uint8List wav)?  uploadCall,TResult? Function()?  deleteCall,TResult? Function()?  updateState,TResult? Function()?  cancelUpdate,TResult? Function( int? after)?  tailLog,}) {final _that = this;
switch (_that) {
case FeederRequest_GetSettings() when getSettings != null:
return getSettings();case FeederRequest_PutSettings() when putSettings != null:
return putSettings(_that.base,_that.changes);case FeederRequest_PutWifi() when putWifi != null:
return putWifi(_that.base,_that.ssid,_that.passphrase);case FeederRequest_ForgetWifi() when forgetWifi != null:
return forgetWifi(_that.base);case FeederRequest_SetLock() when setLock != null:
return setLock(_that.locked);case FeederRequest_Play() when play != null:
return play(_that.tune);case FeederRequest_SyncClock() when syncClock != null:
return syncClock();case FeederRequest_GetTime() when getTime != null:
return getTime();case FeederRequest_PutTime() when putTime != null:
return putTime(_that.base,_that.zone,_that.ntp);case FeederRequest_CreateInvite() when createInvite != null:
return createInvite(_that.role);case FeederRequest_ListClients() when listClients != null:
return listClients();case FeederRequest_RevokeClient() when revokeClient != null:
return revokeClient(_that.key);case FeederRequest_Reboot() when reboot != null:
return reboot();case FeederRequest_ResetDesiccant() when resetDesiccant != null:
return resetDesiccant();case FeederRequest_Stats() when stats != null:
return stats(_that.section);case FeederRequest_ReadTrace() when readTrace != null:
return readTrace(_that.feed);case FeederRequest_GetCalibration() when getCalibration != null:
return getCalibration();case FeederRequest_PutCalibration() when putCalibration != null:
return putCalibration(_that.base,_that.portion,_that.bowl);case FeederRequest_GetPlans() when getPlans != null:
return getPlans();case FeederRequest_EditPlans() when editPlans != null:
return editPlans(_that.plans,_that.edit);case FeederRequest_GetCall() when getCall != null:
return getCall();case FeederRequest_UploadCall() when uploadCall != null:
return uploadCall(_that.wav);case FeederRequest_DeleteCall() when deleteCall != null:
return deleteCall();case FeederRequest_UpdateState() when updateState != null:
return updateState();case FeederRequest_CancelUpdate() when cancelUpdate != null:
return cancelUpdate();case FeederRequest_TailLog() when tailLog != null:
return tailLog(_that.after);case _:
  return null;

}
}

}

/// @nodoc


class FeederRequest_GetSettings extends FeederRequest {
  const FeederRequest_GetSettings(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_GetSettings);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.getSettings()';
}


}




/// @nodoc


class FeederRequest_PutSettings extends FeederRequest {
  const FeederRequest_PutSettings({required this.base, required this.changes}): super._();
  

 final  int base;
 final  SettingsChanges changes;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_PutSettingsCopyWith<FeederRequest_PutSettings> get copyWith => _$FeederRequest_PutSettingsCopyWithImpl<FeederRequest_PutSettings>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_PutSettings&&(identical(other.base, base) || other.base == base)&&(identical(other.changes, changes) || other.changes == changes));
}


@override
int get hashCode {
    return Object.hash(runtimeType,base,changes);
}

@override
String toString() {
    return 'FeederRequest.putSettings(base: $base, changes: $changes)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_PutSettingsCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_PutSettingsCopyWith(FeederRequest_PutSettings value, $Res Function(FeederRequest_PutSettings) _then) = _$FeederRequest_PutSettingsCopyWithImpl;
@useResult
$Res call({
 int base, SettingsChanges changes
});




}
/// @nodoc
class _$FeederRequest_PutSettingsCopyWithImpl<$Res>
    implements $FeederRequest_PutSettingsCopyWith<$Res> {
  _$FeederRequest_PutSettingsCopyWithImpl(this._self, this._then);

  final FeederRequest_PutSettings _self;
  final $Res Function(FeederRequest_PutSettings) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? base = null,Object? changes = null,}) {
  return _then(FeederRequest_PutSettings(
base: null == base ? _self.base : base // ignore: cast_nullable_to_non_nullable
as int,changes: null == changes ? _self.changes : changes // ignore: cast_nullable_to_non_nullable
as SettingsChanges,
  ));
}


}

/// @nodoc


class FeederRequest_PutWifi extends FeederRequest {
  const FeederRequest_PutWifi({required this.base, required this.ssid, required this.passphrase}): super._();
  

 final  int base;
 final  String ssid;
 final  String passphrase;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_PutWifiCopyWith<FeederRequest_PutWifi> get copyWith => _$FeederRequest_PutWifiCopyWithImpl<FeederRequest_PutWifi>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_PutWifi&&(identical(other.base, base) || other.base == base)&&(identical(other.ssid, ssid) || other.ssid == ssid)&&(identical(other.passphrase, passphrase) || other.passphrase == passphrase));
}


@override
int get hashCode {
    return Object.hash(runtimeType,base,ssid,passphrase);
}

@override
String toString() {
    return 'FeederRequest.putWifi(base: $base, ssid: $ssid, passphrase: $passphrase)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_PutWifiCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_PutWifiCopyWith(FeederRequest_PutWifi value, $Res Function(FeederRequest_PutWifi) _then) = _$FeederRequest_PutWifiCopyWithImpl;
@useResult
$Res call({
 int base, String ssid, String passphrase
});




}
/// @nodoc
class _$FeederRequest_PutWifiCopyWithImpl<$Res>
    implements $FeederRequest_PutWifiCopyWith<$Res> {
  _$FeederRequest_PutWifiCopyWithImpl(this._self, this._then);

  final FeederRequest_PutWifi _self;
  final $Res Function(FeederRequest_PutWifi) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? base = null,Object? ssid = null,Object? passphrase = null,}) {
  return _then(FeederRequest_PutWifi(
base: null == base ? _self.base : base // ignore: cast_nullable_to_non_nullable
as int,ssid: null == ssid ? _self.ssid : ssid // ignore: cast_nullable_to_non_nullable
as String,passphrase: null == passphrase ? _self.passphrase : passphrase // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class FeederRequest_ForgetWifi extends FeederRequest {
  const FeederRequest_ForgetWifi({required this.base}): super._();
  

 final  int base;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_ForgetWifiCopyWith<FeederRequest_ForgetWifi> get copyWith => _$FeederRequest_ForgetWifiCopyWithImpl<FeederRequest_ForgetWifi>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_ForgetWifi&&(identical(other.base, base) || other.base == base));
}


@override
int get hashCode {
    return Object.hash(runtimeType,base);
}

@override
String toString() {
    return 'FeederRequest.forgetWifi(base: $base)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_ForgetWifiCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_ForgetWifiCopyWith(FeederRequest_ForgetWifi value, $Res Function(FeederRequest_ForgetWifi) _then) = _$FeederRequest_ForgetWifiCopyWithImpl;
@useResult
$Res call({
 int base
});




}
/// @nodoc
class _$FeederRequest_ForgetWifiCopyWithImpl<$Res>
    implements $FeederRequest_ForgetWifiCopyWith<$Res> {
  _$FeederRequest_ForgetWifiCopyWithImpl(this._self, this._then);

  final FeederRequest_ForgetWifi _self;
  final $Res Function(FeederRequest_ForgetWifi) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? base = null,}) {
  return _then(FeederRequest_ForgetWifi(
base: null == base ? _self.base : base // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class FeederRequest_SetLock extends FeederRequest {
  const FeederRequest_SetLock({required this.locked}): super._();
  

 final  bool locked;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_SetLockCopyWith<FeederRequest_SetLock> get copyWith => _$FeederRequest_SetLockCopyWithImpl<FeederRequest_SetLock>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_SetLock&&(identical(other.locked, locked) || other.locked == locked));
}


@override
int get hashCode {
    return Object.hash(runtimeType,locked);
}

@override
String toString() {
    return 'FeederRequest.setLock(locked: $locked)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_SetLockCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_SetLockCopyWith(FeederRequest_SetLock value, $Res Function(FeederRequest_SetLock) _then) = _$FeederRequest_SetLockCopyWithImpl;
@useResult
$Res call({
 bool locked
});




}
/// @nodoc
class _$FeederRequest_SetLockCopyWithImpl<$Res>
    implements $FeederRequest_SetLockCopyWith<$Res> {
  _$FeederRequest_SetLockCopyWithImpl(this._self, this._then);

  final FeederRequest_SetLock _self;
  final $Res Function(FeederRequest_SetLock) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? locked = null,}) {
  return _then(FeederRequest_SetLock(
locked: null == locked ? _self.locked : locked // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class FeederRequest_Play extends FeederRequest {
  const FeederRequest_Play({required this.tune}): super._();
  

 final  Tune tune;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_PlayCopyWith<FeederRequest_Play> get copyWith => _$FeederRequest_PlayCopyWithImpl<FeederRequest_Play>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_Play&&(identical(other.tune, tune) || other.tune == tune));
}


@override
int get hashCode {
    return Object.hash(runtimeType,tune);
}

@override
String toString() {
    return 'FeederRequest.play(tune: $tune)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_PlayCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_PlayCopyWith(FeederRequest_Play value, $Res Function(FeederRequest_Play) _then) = _$FeederRequest_PlayCopyWithImpl;
@useResult
$Res call({
 Tune tune
});




}
/// @nodoc
class _$FeederRequest_PlayCopyWithImpl<$Res>
    implements $FeederRequest_PlayCopyWith<$Res> {
  _$FeederRequest_PlayCopyWithImpl(this._self, this._then);

  final FeederRequest_Play _self;
  final $Res Function(FeederRequest_Play) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? tune = null,}) {
  return _then(FeederRequest_Play(
tune: null == tune ? _self.tune : tune // ignore: cast_nullable_to_non_nullable
as Tune,
  ));
}


}

/// @nodoc


class FeederRequest_SyncClock extends FeederRequest {
  const FeederRequest_SyncClock(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_SyncClock);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.syncClock()';
}


}




/// @nodoc


class FeederRequest_GetTime extends FeederRequest {
  const FeederRequest_GetTime(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_GetTime);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.getTime()';
}


}




/// @nodoc


class FeederRequest_PutTime extends FeederRequest {
  const FeederRequest_PutTime({required this.base, this.zone, this.ntp}): super._();
  

 final  int base;
 final  ZoneChange? zone;
/// As `wellfed time --ntp` takes it: `off`, `gateway`, or an address.
 final  String? ntp;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_PutTimeCopyWith<FeederRequest_PutTime> get copyWith => _$FeederRequest_PutTimeCopyWithImpl<FeederRequest_PutTime>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_PutTime&&(identical(other.base, base) || other.base == base)&&(identical(other.zone, zone) || other.zone == zone)&&(identical(other.ntp, ntp) || other.ntp == ntp));
}


@override
int get hashCode {
    return Object.hash(runtimeType,base,zone,ntp);
}

@override
String toString() {
    return 'FeederRequest.putTime(base: $base, zone: $zone, ntp: $ntp)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_PutTimeCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_PutTimeCopyWith(FeederRequest_PutTime value, $Res Function(FeederRequest_PutTime) _then) = _$FeederRequest_PutTimeCopyWithImpl;
@useResult
$Res call({
 int base, ZoneChange? zone, String? ntp
});


$ZoneChangeCopyWith<$Res>? get zone;

}
/// @nodoc
class _$FeederRequest_PutTimeCopyWithImpl<$Res>
    implements $FeederRequest_PutTimeCopyWith<$Res> {
  _$FeederRequest_PutTimeCopyWithImpl(this._self, this._then);

  final FeederRequest_PutTime _self;
  final $Res Function(FeederRequest_PutTime) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? base = null,Object? zone = freezed,Object? ntp = freezed,}) {
  return _then(FeederRequest_PutTime(
base: null == base ? _self.base : base // ignore: cast_nullable_to_non_nullable
as int,zone: freezed == zone ? _self.zone : zone // ignore: cast_nullable_to_non_nullable
as ZoneChange?,ntp: freezed == ntp ? _self.ntp : ntp // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$ZoneChangeCopyWith<$Res>? get zone {
    if (_self.zone == null) {
    return null;
  }

  return $ZoneChangeCopyWith<$Res>(_self.zone!, (value) {
    return _then(_self.copyWith(zone: value));
  });
}
}

/// @nodoc


class FeederRequest_CreateInvite extends FeederRequest {
  const FeederRequest_CreateInvite({required this.role}): super._();
  

 final  ClientRole role;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_CreateInviteCopyWith<FeederRequest_CreateInvite> get copyWith => _$FeederRequest_CreateInviteCopyWithImpl<FeederRequest_CreateInvite>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_CreateInvite&&(identical(other.role, role) || other.role == role));
}


@override
int get hashCode {
    return Object.hash(runtimeType,role);
}

@override
String toString() {
    return 'FeederRequest.createInvite(role: $role)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_CreateInviteCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_CreateInviteCopyWith(FeederRequest_CreateInvite value, $Res Function(FeederRequest_CreateInvite) _then) = _$FeederRequest_CreateInviteCopyWithImpl;
@useResult
$Res call({
 ClientRole role
});




}
/// @nodoc
class _$FeederRequest_CreateInviteCopyWithImpl<$Res>
    implements $FeederRequest_CreateInviteCopyWith<$Res> {
  _$FeederRequest_CreateInviteCopyWithImpl(this._self, this._then);

  final FeederRequest_CreateInvite _self;
  final $Res Function(FeederRequest_CreateInvite) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? role = null,}) {
  return _then(FeederRequest_CreateInvite(
role: null == role ? _self.role : role // ignore: cast_nullable_to_non_nullable
as ClientRole,
  ));
}


}

/// @nodoc


class FeederRequest_ListClients extends FeederRequest {
  const FeederRequest_ListClients(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_ListClients);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.listClients()';
}


}




/// @nodoc


class FeederRequest_RevokeClient extends FeederRequest {
  const FeederRequest_RevokeClient({required this.key}): super._();
  

 final  Uint8List key;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_RevokeClientCopyWith<FeederRequest_RevokeClient> get copyWith => _$FeederRequest_RevokeClientCopyWithImpl<FeederRequest_RevokeClient>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_RevokeClient&&const DeepCollectionEquality().equals(other.key, key));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(key));
}

@override
String toString() {
    return 'FeederRequest.revokeClient(key: $key)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_RevokeClientCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_RevokeClientCopyWith(FeederRequest_RevokeClient value, $Res Function(FeederRequest_RevokeClient) _then) = _$FeederRequest_RevokeClientCopyWithImpl;
@useResult
$Res call({
 Uint8List key
});




}
/// @nodoc
class _$FeederRequest_RevokeClientCopyWithImpl<$Res>
    implements $FeederRequest_RevokeClientCopyWith<$Res> {
  _$FeederRequest_RevokeClientCopyWithImpl(this._self, this._then);

  final FeederRequest_RevokeClient _self;
  final $Res Function(FeederRequest_RevokeClient) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? key = null,}) {
  return _then(FeederRequest_RevokeClient(
key: null == key ? _self.key : key // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class FeederRequest_Reboot extends FeederRequest {
  const FeederRequest_Reboot(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_Reboot);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.reboot()';
}


}




/// @nodoc


class FeederRequest_ResetDesiccant extends FeederRequest {
  const FeederRequest_ResetDesiccant(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_ResetDesiccant);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.resetDesiccant()';
}


}




/// @nodoc


class FeederRequest_Stats extends FeederRequest {
  const FeederRequest_Stats({required this.section}): super._();
  

 final  StatsSection section;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_StatsCopyWith<FeederRequest_Stats> get copyWith => _$FeederRequest_StatsCopyWithImpl<FeederRequest_Stats>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_Stats&&(identical(other.section, section) || other.section == section));
}


@override
int get hashCode {
    return Object.hash(runtimeType,section);
}

@override
String toString() {
    return 'FeederRequest.stats(section: $section)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_StatsCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_StatsCopyWith(FeederRequest_Stats value, $Res Function(FeederRequest_Stats) _then) = _$FeederRequest_StatsCopyWithImpl;
@useResult
$Res call({
 StatsSection section
});




}
/// @nodoc
class _$FeederRequest_StatsCopyWithImpl<$Res>
    implements $FeederRequest_StatsCopyWith<$Res> {
  _$FeederRequest_StatsCopyWithImpl(this._self, this._then);

  final FeederRequest_Stats _self;
  final $Res Function(FeederRequest_Stats) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? section = null,}) {
  return _then(FeederRequest_Stats(
section: null == section ? _self.section : section // ignore: cast_nullable_to_non_nullable
as StatsSection,
  ));
}


}

/// @nodoc


class FeederRequest_ReadTrace extends FeederRequest {
  const FeederRequest_ReadTrace({required this.feed}): super._();
  

 final  int feed;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_ReadTraceCopyWith<FeederRequest_ReadTrace> get copyWith => _$FeederRequest_ReadTraceCopyWithImpl<FeederRequest_ReadTrace>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_ReadTrace&&(identical(other.feed, feed) || other.feed == feed));
}


@override
int get hashCode {
    return Object.hash(runtimeType,feed);
}

@override
String toString() {
    return 'FeederRequest.readTrace(feed: $feed)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_ReadTraceCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_ReadTraceCopyWith(FeederRequest_ReadTrace value, $Res Function(FeederRequest_ReadTrace) _then) = _$FeederRequest_ReadTraceCopyWithImpl;
@useResult
$Res call({
 int feed
});




}
/// @nodoc
class _$FeederRequest_ReadTraceCopyWithImpl<$Res>
    implements $FeederRequest_ReadTraceCopyWith<$Res> {
  _$FeederRequest_ReadTraceCopyWithImpl(this._self, this._then);

  final FeederRequest_ReadTrace _self;
  final $Res Function(FeederRequest_ReadTrace) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? feed = null,}) {
  return _then(FeederRequest_ReadTrace(
feed: null == feed ? _self.feed : feed // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class FeederRequest_GetCalibration extends FeederRequest {
  const FeederRequest_GetCalibration(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_GetCalibration);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.getCalibration()';
}


}




/// @nodoc


class FeederRequest_PutCalibration extends FeederRequest {
  const FeederRequest_PutCalibration({required this.base, required this.portion, required this.bowl}): super._();
  

 final  int base;
 final  PortionChange portion;
 final  Bowl bowl;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_PutCalibrationCopyWith<FeederRequest_PutCalibration> get copyWith => _$FeederRequest_PutCalibrationCopyWithImpl<FeederRequest_PutCalibration>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_PutCalibration&&(identical(other.base, base) || other.base == base)&&(identical(other.portion, portion) || other.portion == portion)&&(identical(other.bowl, bowl) || other.bowl == bowl));
}


@override
int get hashCode {
    return Object.hash(runtimeType,base,portion,bowl);
}

@override
String toString() {
    return 'FeederRequest.putCalibration(base: $base, portion: $portion, bowl: $bowl)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_PutCalibrationCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_PutCalibrationCopyWith(FeederRequest_PutCalibration value, $Res Function(FeederRequest_PutCalibration) _then) = _$FeederRequest_PutCalibrationCopyWithImpl;
@useResult
$Res call({
 int base, PortionChange portion, Bowl bowl
});


$PortionChangeCopyWith<$Res> get portion;

}
/// @nodoc
class _$FeederRequest_PutCalibrationCopyWithImpl<$Res>
    implements $FeederRequest_PutCalibrationCopyWith<$Res> {
  _$FeederRequest_PutCalibrationCopyWithImpl(this._self, this._then);

  final FeederRequest_PutCalibration _self;
  final $Res Function(FeederRequest_PutCalibration) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? base = null,Object? portion = null,Object? bowl = null,}) {
  return _then(FeederRequest_PutCalibration(
base: null == base ? _self.base : base // ignore: cast_nullable_to_non_nullable
as int,portion: null == portion ? _self.portion : portion // ignore: cast_nullable_to_non_nullable
as PortionChange,bowl: null == bowl ? _self.bowl : bowl // ignore: cast_nullable_to_non_nullable
as Bowl,
  ));
}

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$PortionChangeCopyWith<$Res> get portion {
  
  return $PortionChangeCopyWith<$Res>(_self.portion, (value) {
    return _then(_self.copyWith(portion: value));
  });
}
}

/// @nodoc


class FeederRequest_GetPlans extends FeederRequest {
  const FeederRequest_GetPlans(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_GetPlans);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.getPlans()';
}


}




/// @nodoc


class FeederRequest_EditPlans extends FeederRequest {
  const FeederRequest_EditPlans({required this.plans, required this.edit}): super._();
  

 final  PlansView plans;
 final  PlanEdit edit;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_EditPlansCopyWith<FeederRequest_EditPlans> get copyWith => _$FeederRequest_EditPlansCopyWithImpl<FeederRequest_EditPlans>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_EditPlans&&(identical(other.plans, plans) || other.plans == plans)&&(identical(other.edit, edit) || other.edit == edit));
}


@override
int get hashCode {
    return Object.hash(runtimeType,plans,edit);
}

@override
String toString() {
    return 'FeederRequest.editPlans(plans: $plans, edit: $edit)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_EditPlansCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_EditPlansCopyWith(FeederRequest_EditPlans value, $Res Function(FeederRequest_EditPlans) _then) = _$FeederRequest_EditPlansCopyWithImpl;
@useResult
$Res call({
 PlansView plans, PlanEdit edit
});


$PlanEditCopyWith<$Res> get edit;

}
/// @nodoc
class _$FeederRequest_EditPlansCopyWithImpl<$Res>
    implements $FeederRequest_EditPlansCopyWith<$Res> {
  _$FeederRequest_EditPlansCopyWithImpl(this._self, this._then);

  final FeederRequest_EditPlans _self;
  final $Res Function(FeederRequest_EditPlans) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? plans = null,Object? edit = null,}) {
  return _then(FeederRequest_EditPlans(
plans: null == plans ? _self.plans : plans // ignore: cast_nullable_to_non_nullable
as PlansView,edit: null == edit ? _self.edit : edit // ignore: cast_nullable_to_non_nullable
as PlanEdit,
  ));
}

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$PlanEditCopyWith<$Res> get edit {
  
  return $PlanEditCopyWith<$Res>(_self.edit, (value) {
    return _then(_self.copyWith(edit: value));
  });
}
}

/// @nodoc


class FeederRequest_GetCall extends FeederRequest {
  const FeederRequest_GetCall(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_GetCall);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.getCall()';
}


}




/// @nodoc


class FeederRequest_UploadCall extends FeederRequest {
  const FeederRequest_UploadCall({required this.wav}): super._();
  

 final  Uint8List wav;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_UploadCallCopyWith<FeederRequest_UploadCall> get copyWith => _$FeederRequest_UploadCallCopyWithImpl<FeederRequest_UploadCall>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_UploadCall&&const DeepCollectionEquality().equals(other.wav, wav));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(wav));
}

@override
String toString() {
    return 'FeederRequest.uploadCall(wav: $wav)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_UploadCallCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_UploadCallCopyWith(FeederRequest_UploadCall value, $Res Function(FeederRequest_UploadCall) _then) = _$FeederRequest_UploadCallCopyWithImpl;
@useResult
$Res call({
 Uint8List wav
});




}
/// @nodoc
class _$FeederRequest_UploadCallCopyWithImpl<$Res>
    implements $FeederRequest_UploadCallCopyWith<$Res> {
  _$FeederRequest_UploadCallCopyWithImpl(this._self, this._then);

  final FeederRequest_UploadCall _self;
  final $Res Function(FeederRequest_UploadCall) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? wav = null,}) {
  return _then(FeederRequest_UploadCall(
wav: null == wav ? _self.wav : wav // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class FeederRequest_DeleteCall extends FeederRequest {
  const FeederRequest_DeleteCall(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_DeleteCall);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.deleteCall()';
}


}




/// @nodoc


class FeederRequest_UpdateState extends FeederRequest {
  const FeederRequest_UpdateState(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_UpdateState);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.updateState()';
}


}




/// @nodoc


class FeederRequest_CancelUpdate extends FeederRequest {
  const FeederRequest_CancelUpdate(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_CancelUpdate);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FeederRequest.cancelUpdate()';
}


}




/// @nodoc


class FeederRequest_TailLog extends FeederRequest {
  const FeederRequest_TailLog({this.after}): super._();
  

 final  int? after;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FeederRequest_TailLogCopyWith<FeederRequest_TailLog> get copyWith => _$FeederRequest_TailLogCopyWithImpl<FeederRequest_TailLog>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FeederRequest_TailLog&&(identical(other.after, after) || other.after == after));
}


@override
int get hashCode {
    return Object.hash(runtimeType,after);
}

@override
String toString() {
    return 'FeederRequest.tailLog(after: $after)';
}


}

/// @nodoc
abstract mixin class $FeederRequest_TailLogCopyWith<$Res> implements $FeederRequestCopyWith<$Res> {
  factory $FeederRequest_TailLogCopyWith(FeederRequest_TailLog value, $Res Function(FeederRequest_TailLog) _then) = _$FeederRequest_TailLogCopyWithImpl;
@useResult
$Res call({
 int? after
});




}
/// @nodoc
class _$FeederRequest_TailLogCopyWithImpl<$Res>
    implements $FeederRequest_TailLogCopyWith<$Res> {
  _$FeederRequest_TailLogCopyWithImpl(this._self, this._then);

  final FeederRequest_TailLog _self;
  final $Res Function(FeederRequest_TailLog) _then;

/// Create a copy of FeederRequest
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? after = freezed,}) {
  return _then(FeederRequest_TailLog(
after: freezed == after ? _self.after : after // ignore: cast_nullable_to_non_nullable
as int?,
  ));
}


}

/// @nodoc
mixin _$NtpView {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is NtpView);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'NtpView()';
}


}

/// @nodoc
class $NtpViewCopyWith<$Res>  {
$NtpViewCopyWith(NtpView _, $Res Function(NtpView) __);
}


/// Adds pattern-matching-related methods to [NtpView].
extension NtpViewPatterns on NtpView {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( NtpView_Off value)?  off,TResult Function( NtpView_Gateway value)?  gateway,TResult Function( NtpView_Address value)?  address,required TResult orElse(),}){
final _that = this;
switch (_that) {
case NtpView_Off() when off != null:
return off(_that);case NtpView_Gateway() when gateway != null:
return gateway(_that);case NtpView_Address() when address != null:
return address(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( NtpView_Off value)  off,required TResult Function( NtpView_Gateway value)  gateway,required TResult Function( NtpView_Address value)  address,}){
final _that = this;
switch (_that) {
case NtpView_Off():
return off(_that);case NtpView_Gateway():
return gateway(_that);case NtpView_Address():
return address(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( NtpView_Off value)?  off,TResult? Function( NtpView_Gateway value)?  gateway,TResult? Function( NtpView_Address value)?  address,}){
final _that = this;
switch (_that) {
case NtpView_Off() when off != null:
return off(_that);case NtpView_Gateway() when gateway != null:
return gateway(_that);case NtpView_Address() when address != null:
return address(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  off,TResult Function( int port)?  gateway,TResult Function( String ip,  int port)?  address,required TResult orElse(),}) {final _that = this;
switch (_that) {
case NtpView_Off() when off != null:
return off();case NtpView_Gateway() when gateway != null:
return gateway(_that.port);case NtpView_Address() when address != null:
return address(_that.ip,_that.port);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  off,required TResult Function( int port)  gateway,required TResult Function( String ip,  int port)  address,}) {final _that = this;
switch (_that) {
case NtpView_Off():
return off();case NtpView_Gateway():
return gateway(_that.port);case NtpView_Address():
return address(_that.ip,_that.port);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  off,TResult? Function( int port)?  gateway,TResult? Function( String ip,  int port)?  address,}) {final _that = this;
switch (_that) {
case NtpView_Off() when off != null:
return off();case NtpView_Gateway() when gateway != null:
return gateway(_that.port);case NtpView_Address() when address != null:
return address(_that.ip,_that.port);case _:
  return null;

}
}

}

/// @nodoc


class NtpView_Off extends NtpView {
  const NtpView_Off(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is NtpView_Off);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'NtpView.off()';
}


}




/// @nodoc


class NtpView_Gateway extends NtpView {
  const NtpView_Gateway({required this.port}): super._();
  

 final  int port;

/// Create a copy of NtpView
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$NtpView_GatewayCopyWith<NtpView_Gateway> get copyWith => _$NtpView_GatewayCopyWithImpl<NtpView_Gateway>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is NtpView_Gateway&&(identical(other.port, port) || other.port == port));
}


@override
int get hashCode {
    return Object.hash(runtimeType,port);
}

@override
String toString() {
    return 'NtpView.gateway(port: $port)';
}


}

/// @nodoc
abstract mixin class $NtpView_GatewayCopyWith<$Res> implements $NtpViewCopyWith<$Res> {
  factory $NtpView_GatewayCopyWith(NtpView_Gateway value, $Res Function(NtpView_Gateway) _then) = _$NtpView_GatewayCopyWithImpl;
@useResult
$Res call({
 int port
});




}
/// @nodoc
class _$NtpView_GatewayCopyWithImpl<$Res>
    implements $NtpView_GatewayCopyWith<$Res> {
  _$NtpView_GatewayCopyWithImpl(this._self, this._then);

  final NtpView_Gateway _self;
  final $Res Function(NtpView_Gateway) _then;

/// Create a copy of NtpView
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? port = null,}) {
  return _then(NtpView_Gateway(
port: null == port ? _self.port : port // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class NtpView_Address extends NtpView {
  const NtpView_Address({required this.ip, required this.port}): super._();
  

 final  String ip;
 final  int port;

/// Create a copy of NtpView
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$NtpView_AddressCopyWith<NtpView_Address> get copyWith => _$NtpView_AddressCopyWithImpl<NtpView_Address>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is NtpView_Address&&(identical(other.ip, ip) || other.ip == ip)&&(identical(other.port, port) || other.port == port));
}


@override
int get hashCode {
    return Object.hash(runtimeType,ip,port);
}

@override
String toString() {
    return 'NtpView.address(ip: $ip, port: $port)';
}


}

/// @nodoc
abstract mixin class $NtpView_AddressCopyWith<$Res> implements $NtpViewCopyWith<$Res> {
  factory $NtpView_AddressCopyWith(NtpView_Address value, $Res Function(NtpView_Address) _then) = _$NtpView_AddressCopyWithImpl;
@useResult
$Res call({
 String ip, int port
});




}
/// @nodoc
class _$NtpView_AddressCopyWithImpl<$Res>
    implements $NtpView_AddressCopyWith<$Res> {
  _$NtpView_AddressCopyWithImpl(this._self, this._then);

  final NtpView_Address _self;
  final $Res Function(NtpView_Address) _then;

/// Create a copy of NtpView
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? ip = null,Object? port = null,}) {
  return _then(NtpView_Address(
ip: null == ip ? _self.ip : ip // ignore: cast_nullable_to_non_nullable
as String,port: null == port ? _self.port : port // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc
mixin _$PlanEdit {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PlanEdit);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'PlanEdit()';
}


}

/// @nodoc
class $PlanEditCopyWith<$Res>  {
$PlanEditCopyWith(PlanEdit _, $Res Function(PlanEdit) __);
}


/// Adds pattern-matching-related methods to [PlanEdit].
extension PlanEditPatterns on PlanEdit {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( PlanEdit_Add value)?  add,TResult Function( PlanEdit_Change value)?  change,TResult Function( PlanEdit_Enabled value)?  enabled,TResult Function( PlanEdit_Delete value)?  delete,required TResult orElse(),}){
final _that = this;
switch (_that) {
case PlanEdit_Add() when add != null:
return add(_that);case PlanEdit_Change() when change != null:
return change(_that);case PlanEdit_Enabled() when enabled != null:
return enabled(_that);case PlanEdit_Delete() when delete != null:
return delete(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( PlanEdit_Add value)  add,required TResult Function( PlanEdit_Change value)  change,required TResult Function( PlanEdit_Enabled value)  enabled,required TResult Function( PlanEdit_Delete value)  delete,}){
final _that = this;
switch (_that) {
case PlanEdit_Add():
return add(_that);case PlanEdit_Change():
return change(_that);case PlanEdit_Enabled():
return enabled(_that);case PlanEdit_Delete():
return delete(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( PlanEdit_Add value)?  add,TResult? Function( PlanEdit_Change value)?  change,TResult? Function( PlanEdit_Enabled value)?  enabled,TResult? Function( PlanEdit_Delete value)?  delete,}){
final _that = this;
switch (_that) {
case PlanEdit_Add() when add != null:
return add(_that);case PlanEdit_Change() when change != null:
return change(_that);case PlanEdit_Enabled() when enabled != null:
return enabled(_that);case PlanEdit_Delete() when delete != null:
return delete(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( PlanView plan)?  add,TResult Function( int slot,  int? minute,  int? portions,  List<Weekday>? days,  int? call,  bool? enabled)?  change,TResult Function( int slot,  bool enabled)?  enabled,TResult Function( int slot)?  delete,required TResult orElse(),}) {final _that = this;
switch (_that) {
case PlanEdit_Add() when add != null:
return add(_that.plan);case PlanEdit_Change() when change != null:
return change(_that.slot,_that.minute,_that.portions,_that.days,_that.call,_that.enabled);case PlanEdit_Enabled() when enabled != null:
return enabled(_that.slot,_that.enabled);case PlanEdit_Delete() when delete != null:
return delete(_that.slot);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( PlanView plan)  add,required TResult Function( int slot,  int? minute,  int? portions,  List<Weekday>? days,  int? call,  bool? enabled)  change,required TResult Function( int slot,  bool enabled)  enabled,required TResult Function( int slot)  delete,}) {final _that = this;
switch (_that) {
case PlanEdit_Add():
return add(_that.plan);case PlanEdit_Change():
return change(_that.slot,_that.minute,_that.portions,_that.days,_that.call,_that.enabled);case PlanEdit_Enabled():
return enabled(_that.slot,_that.enabled);case PlanEdit_Delete():
return delete(_that.slot);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( PlanView plan)?  add,TResult? Function( int slot,  int? minute,  int? portions,  List<Weekday>? days,  int? call,  bool? enabled)?  change,TResult? Function( int slot,  bool enabled)?  enabled,TResult? Function( int slot)?  delete,}) {final _that = this;
switch (_that) {
case PlanEdit_Add() when add != null:
return add(_that.plan);case PlanEdit_Change() when change != null:
return change(_that.slot,_that.minute,_that.portions,_that.days,_that.call,_that.enabled);case PlanEdit_Enabled() when enabled != null:
return enabled(_that.slot,_that.enabled);case PlanEdit_Delete() when delete != null:
return delete(_that.slot);case _:
  return null;

}
}

}

/// @nodoc


class PlanEdit_Add extends PlanEdit {
  const PlanEdit_Add({required this.plan}): super._();
  

 final  PlanView plan;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PlanEdit_AddCopyWith<PlanEdit_Add> get copyWith => _$PlanEdit_AddCopyWithImpl<PlanEdit_Add>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PlanEdit_Add&&(identical(other.plan, plan) || other.plan == plan));
}


@override
int get hashCode {
    return Object.hash(runtimeType,plan);
}

@override
String toString() {
    return 'PlanEdit.add(plan: $plan)';
}


}

/// @nodoc
abstract mixin class $PlanEdit_AddCopyWith<$Res> implements $PlanEditCopyWith<$Res> {
  factory $PlanEdit_AddCopyWith(PlanEdit_Add value, $Res Function(PlanEdit_Add) _then) = _$PlanEdit_AddCopyWithImpl;
@useResult
$Res call({
 PlanView plan
});




}
/// @nodoc
class _$PlanEdit_AddCopyWithImpl<$Res>
    implements $PlanEdit_AddCopyWith<$Res> {
  _$PlanEdit_AddCopyWithImpl(this._self, this._then);

  final PlanEdit_Add _self;
  final $Res Function(PlanEdit_Add) _then;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? plan = null,}) {
  return _then(PlanEdit_Add(
plan: null == plan ? _self.plan : plan // ignore: cast_nullable_to_non_nullable
as PlanView,
  ));
}


}

/// @nodoc


class PlanEdit_Change extends PlanEdit {
  const PlanEdit_Change({required this.slot, this.minute, this.portions,  List<Weekday>? days, this.call, this.enabled}): _days = days,super._();
  

 final  int slot;
 final  int? minute;
 final  int? portions;
 final  List<Weekday>? _days;
 List<Weekday>? get days {
  final value = _days;
  if (value == null) return null;
  if (_days is EqualUnmodifiableListView) return _days;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(value);
}

 final  int? call;
 final  bool? enabled;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PlanEdit_ChangeCopyWith<PlanEdit_Change> get copyWith => _$PlanEdit_ChangeCopyWithImpl<PlanEdit_Change>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PlanEdit_Change&&(identical(other.slot, slot) || other.slot == slot)&&(identical(other.minute, minute) || other.minute == minute)&&(identical(other.portions, portions) || other.portions == portions)&&const DeepCollectionEquality().equals(other.days, _days)&&(identical(other.call, call) || other.call == call)&&(identical(other.enabled, enabled) || other.enabled == enabled));
}


@override
int get hashCode {
    return Object.hash(runtimeType,slot,minute,portions,const DeepCollectionEquality().hash(_days),call,enabled);
}

@override
String toString() {
    return 'PlanEdit.change(slot: $slot, minute: $minute, portions: $portions, days: $days, call: $call, enabled: $enabled)';
}


}

/// @nodoc
abstract mixin class $PlanEdit_ChangeCopyWith<$Res> implements $PlanEditCopyWith<$Res> {
  factory $PlanEdit_ChangeCopyWith(PlanEdit_Change value, $Res Function(PlanEdit_Change) _then) = _$PlanEdit_ChangeCopyWithImpl;
@useResult
$Res call({
 int slot, int? minute, int? portions, List<Weekday>? days, int? call, bool? enabled
});




}
/// @nodoc
class _$PlanEdit_ChangeCopyWithImpl<$Res>
    implements $PlanEdit_ChangeCopyWith<$Res> {
  _$PlanEdit_ChangeCopyWithImpl(this._self, this._then);

  final PlanEdit_Change _self;
  final $Res Function(PlanEdit_Change) _then;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? slot = null,Object? minute = freezed,Object? portions = freezed,Object? days = freezed,Object? call = freezed,Object? enabled = freezed,}) {
  return _then(PlanEdit_Change(
slot: null == slot ? _self.slot : slot // ignore: cast_nullable_to_non_nullable
as int,minute: freezed == minute ? _self.minute : minute // ignore: cast_nullable_to_non_nullable
as int?,portions: freezed == portions ? _self.portions : portions // ignore: cast_nullable_to_non_nullable
as int?,days: freezed == days ? _self._days : days // ignore: cast_nullable_to_non_nullable
as List<Weekday>?,call: freezed == call ? _self.call : call // ignore: cast_nullable_to_non_nullable
as int?,enabled: freezed == enabled ? _self.enabled : enabled // ignore: cast_nullable_to_non_nullable
as bool?,
  ));
}


}

/// @nodoc


class PlanEdit_Enabled extends PlanEdit {
  const PlanEdit_Enabled({required this.slot, required this.enabled}): super._();
  

 final  int slot;
 final  bool enabled;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PlanEdit_EnabledCopyWith<PlanEdit_Enabled> get copyWith => _$PlanEdit_EnabledCopyWithImpl<PlanEdit_Enabled>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PlanEdit_Enabled&&(identical(other.slot, slot) || other.slot == slot)&&(identical(other.enabled, enabled) || other.enabled == enabled));
}


@override
int get hashCode {
    return Object.hash(runtimeType,slot,enabled);
}

@override
String toString() {
    return 'PlanEdit.enabled(slot: $slot, enabled: $enabled)';
}


}

/// @nodoc
abstract mixin class $PlanEdit_EnabledCopyWith<$Res> implements $PlanEditCopyWith<$Res> {
  factory $PlanEdit_EnabledCopyWith(PlanEdit_Enabled value, $Res Function(PlanEdit_Enabled) _then) = _$PlanEdit_EnabledCopyWithImpl;
@useResult
$Res call({
 int slot, bool enabled
});




}
/// @nodoc
class _$PlanEdit_EnabledCopyWithImpl<$Res>
    implements $PlanEdit_EnabledCopyWith<$Res> {
  _$PlanEdit_EnabledCopyWithImpl(this._self, this._then);

  final PlanEdit_Enabled _self;
  final $Res Function(PlanEdit_Enabled) _then;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? slot = null,Object? enabled = null,}) {
  return _then(PlanEdit_Enabled(
slot: null == slot ? _self.slot : slot // ignore: cast_nullable_to_non_nullable
as int,enabled: null == enabled ? _self.enabled : enabled // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class PlanEdit_Delete extends PlanEdit {
  const PlanEdit_Delete({required this.slot}): super._();
  

 final  int slot;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PlanEdit_DeleteCopyWith<PlanEdit_Delete> get copyWith => _$PlanEdit_DeleteCopyWithImpl<PlanEdit_Delete>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PlanEdit_Delete&&(identical(other.slot, slot) || other.slot == slot));
}


@override
int get hashCode {
    return Object.hash(runtimeType,slot);
}

@override
String toString() {
    return 'PlanEdit.delete(slot: $slot)';
}


}

/// @nodoc
abstract mixin class $PlanEdit_DeleteCopyWith<$Res> implements $PlanEditCopyWith<$Res> {
  factory $PlanEdit_DeleteCopyWith(PlanEdit_Delete value, $Res Function(PlanEdit_Delete) _then) = _$PlanEdit_DeleteCopyWithImpl;
@useResult
$Res call({
 int slot
});




}
/// @nodoc
class _$PlanEdit_DeleteCopyWithImpl<$Res>
    implements $PlanEdit_DeleteCopyWith<$Res> {
  _$PlanEdit_DeleteCopyWithImpl(this._self, this._then);

  final PlanEdit_Delete _self;
  final $Res Function(PlanEdit_Delete) _then;

/// Create a copy of PlanEdit
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? slot = null,}) {
  return _then(PlanEdit_Delete(
slot: null == slot ? _self.slot : slot // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc
mixin _$PortionChange {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PortionChange);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'PortionChange()';
}


}

/// @nodoc
class $PortionChangeCopyWith<$Res>  {
$PortionChangeCopyWith(PortionChange _, $Res Function(PortionChange) __);
}


/// Adds pattern-matching-related methods to [PortionChange].
extension PortionChangePatterns on PortionChange {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( PortionChange_Clear value)?  clear,TResult Function( PortionChange_Milligrams value)?  milligrams,TResult Function( PortionChange_Weighed value)?  weighed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case PortionChange_Clear() when clear != null:
return clear(_that);case PortionChange_Milligrams() when milligrams != null:
return milligrams(_that);case PortionChange_Weighed() when weighed != null:
return weighed(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( PortionChange_Clear value)  clear,required TResult Function( PortionChange_Milligrams value)  milligrams,required TResult Function( PortionChange_Weighed value)  weighed,}){
final _that = this;
switch (_that) {
case PortionChange_Clear():
return clear(_that);case PortionChange_Milligrams():
return milligrams(_that);case PortionChange_Weighed():
return weighed(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( PortionChange_Clear value)?  clear,TResult? Function( PortionChange_Milligrams value)?  milligrams,TResult? Function( PortionChange_Weighed value)?  weighed,}){
final _that = this;
switch (_that) {
case PortionChange_Clear() when clear != null:
return clear(_that);case PortionChange_Milligrams() when milligrams != null:
return milligrams(_that);case PortionChange_Weighed() when weighed != null:
return weighed(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  clear,TResult Function( int mg)?  milligrams,TResult Function( String grams,  int portions)?  weighed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case PortionChange_Clear() when clear != null:
return clear();case PortionChange_Milligrams() when milligrams != null:
return milligrams(_that.mg);case PortionChange_Weighed() when weighed != null:
return weighed(_that.grams,_that.portions);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  clear,required TResult Function( int mg)  milligrams,required TResult Function( String grams,  int portions)  weighed,}) {final _that = this;
switch (_that) {
case PortionChange_Clear():
return clear();case PortionChange_Milligrams():
return milligrams(_that.mg);case PortionChange_Weighed():
return weighed(_that.grams,_that.portions);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  clear,TResult? Function( int mg)?  milligrams,TResult? Function( String grams,  int portions)?  weighed,}) {final _that = this;
switch (_that) {
case PortionChange_Clear() when clear != null:
return clear();case PortionChange_Milligrams() when milligrams != null:
return milligrams(_that.mg);case PortionChange_Weighed() when weighed != null:
return weighed(_that.grams,_that.portions);case _:
  return null;

}
}

}

/// @nodoc


class PortionChange_Clear extends PortionChange {
  const PortionChange_Clear(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PortionChange_Clear);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'PortionChange.clear()';
}


}




/// @nodoc


class PortionChange_Milligrams extends PortionChange {
  const PortionChange_Milligrams({required this.mg}): super._();
  

 final  int mg;

/// Create a copy of PortionChange
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PortionChange_MilligramsCopyWith<PortionChange_Milligrams> get copyWith => _$PortionChange_MilligramsCopyWithImpl<PortionChange_Milligrams>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PortionChange_Milligrams&&(identical(other.mg, mg) || other.mg == mg));
}


@override
int get hashCode {
    return Object.hash(runtimeType,mg);
}

@override
String toString() {
    return 'PortionChange.milligrams(mg: $mg)';
}


}

/// @nodoc
abstract mixin class $PortionChange_MilligramsCopyWith<$Res> implements $PortionChangeCopyWith<$Res> {
  factory $PortionChange_MilligramsCopyWith(PortionChange_Milligrams value, $Res Function(PortionChange_Milligrams) _then) = _$PortionChange_MilligramsCopyWithImpl;
@useResult
$Res call({
 int mg
});




}
/// @nodoc
class _$PortionChange_MilligramsCopyWithImpl<$Res>
    implements $PortionChange_MilligramsCopyWith<$Res> {
  _$PortionChange_MilligramsCopyWithImpl(this._self, this._then);

  final PortionChange_Milligrams _self;
  final $Res Function(PortionChange_Milligrams) _then;

/// Create a copy of PortionChange
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? mg = null,}) {
  return _then(PortionChange_Milligrams(
mg: null == mg ? _self.mg : mg // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class PortionChange_Weighed extends PortionChange {
  const PortionChange_Weighed({required this.grams, required this.portions}): super._();
  

 final  String grams;
 final  int portions;

/// Create a copy of PortionChange
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$PortionChange_WeighedCopyWith<PortionChange_Weighed> get copyWith => _$PortionChange_WeighedCopyWithImpl<PortionChange_Weighed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is PortionChange_Weighed&&(identical(other.grams, grams) || other.grams == grams)&&(identical(other.portions, portions) || other.portions == portions));
}


@override
int get hashCode {
    return Object.hash(runtimeType,grams,portions);
}

@override
String toString() {
    return 'PortionChange.weighed(grams: $grams, portions: $portions)';
}


}

/// @nodoc
abstract mixin class $PortionChange_WeighedCopyWith<$Res> implements $PortionChangeCopyWith<$Res> {
  factory $PortionChange_WeighedCopyWith(PortionChange_Weighed value, $Res Function(PortionChange_Weighed) _then) = _$PortionChange_WeighedCopyWithImpl;
@useResult
$Res call({
 String grams, int portions
});




}
/// @nodoc
class _$PortionChange_WeighedCopyWithImpl<$Res>
    implements $PortionChange_WeighedCopyWith<$Res> {
  _$PortionChange_WeighedCopyWithImpl(this._self, this._then);

  final PortionChange_Weighed _self;
  final $Res Function(PortionChange_Weighed) _then;

/// Create a copy of PortionChange
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? grams = null,Object? portions = null,}) {
  return _then(PortionChange_Weighed(
grams: null == grams ? _self.grams : grams // ignore: cast_nullable_to_non_nullable
as String,portions: null == portions ? _self.portions : portions // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc
mixin _$ZoneChange {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ZoneChange);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ZoneChange()';
}


}

/// @nodoc
class $ZoneChangeCopyWith<$Res>  {
$ZoneChangeCopyWith(ZoneChange _, $Res Function(ZoneChange) __);
}


/// Adds pattern-matching-related methods to [ZoneChange].
extension ZoneChangePatterns on ZoneChange {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ZoneChange_Phone value)?  phone,TResult Function( ZoneChange_Named value)?  named,TResult Function( ZoneChange_Clear value)?  clear,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ZoneChange_Phone() when phone != null:
return phone(_that);case ZoneChange_Named() when named != null:
return named(_that);case ZoneChange_Clear() when clear != null:
return clear(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ZoneChange_Phone value)  phone,required TResult Function( ZoneChange_Named value)  named,required TResult Function( ZoneChange_Clear value)  clear,}){
final _that = this;
switch (_that) {
case ZoneChange_Phone():
return phone(_that);case ZoneChange_Named():
return named(_that);case ZoneChange_Clear():
return clear(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ZoneChange_Phone value)?  phone,TResult? Function( ZoneChange_Named value)?  named,TResult? Function( ZoneChange_Clear value)?  clear,}){
final _that = this;
switch (_that) {
case ZoneChange_Phone() when phone != null:
return phone(_that);case ZoneChange_Named() when named != null:
return named(_that);case ZoneChange_Clear() when clear != null:
return clear(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  phone,TResult Function( String name)?  named,TResult Function()?  clear,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ZoneChange_Phone() when phone != null:
return phone();case ZoneChange_Named() when named != null:
return named(_that.name);case ZoneChange_Clear() when clear != null:
return clear();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  phone,required TResult Function( String name)  named,required TResult Function()  clear,}) {final _that = this;
switch (_that) {
case ZoneChange_Phone():
return phone();case ZoneChange_Named():
return named(_that.name);case ZoneChange_Clear():
return clear();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  phone,TResult? Function( String name)?  named,TResult? Function()?  clear,}) {final _that = this;
switch (_that) {
case ZoneChange_Phone() when phone != null:
return phone();case ZoneChange_Named() when named != null:
return named(_that.name);case ZoneChange_Clear() when clear != null:
return clear();case _:
  return null;

}
}

}

/// @nodoc


class ZoneChange_Phone extends ZoneChange {
  const ZoneChange_Phone(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ZoneChange_Phone);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ZoneChange.phone()';
}


}




/// @nodoc


class ZoneChange_Named extends ZoneChange {
  const ZoneChange_Named({required this.name}): super._();
  

 final  String name;

/// Create a copy of ZoneChange
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ZoneChange_NamedCopyWith<ZoneChange_Named> get copyWith => _$ZoneChange_NamedCopyWithImpl<ZoneChange_Named>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ZoneChange_Named&&(identical(other.name, name) || other.name == name));
}


@override
int get hashCode {
    return Object.hash(runtimeType,name);
}

@override
String toString() {
    return 'ZoneChange.named(name: $name)';
}


}

/// @nodoc
abstract mixin class $ZoneChange_NamedCopyWith<$Res> implements $ZoneChangeCopyWith<$Res> {
  factory $ZoneChange_NamedCopyWith(ZoneChange_Named value, $Res Function(ZoneChange_Named) _then) = _$ZoneChange_NamedCopyWithImpl;
@useResult
$Res call({
 String name
});




}
/// @nodoc
class _$ZoneChange_NamedCopyWithImpl<$Res>
    implements $ZoneChange_NamedCopyWith<$Res> {
  _$ZoneChange_NamedCopyWithImpl(this._self, this._then);

  final ZoneChange_Named _self;
  final $Res Function(ZoneChange_Named) _then;

/// Create a copy of ZoneChange
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? name = null,}) {
  return _then(ZoneChange_Named(
name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ZoneChange_Clear extends ZoneChange {
  const ZoneChange_Clear(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ZoneChange_Clear);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ZoneChange.clear()';
}


}




// dart format on
