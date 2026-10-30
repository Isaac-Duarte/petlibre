// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'restore.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$BackupCheck {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BackupCheck);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'BackupCheck()';
}


}

/// @nodoc
class $BackupCheckCopyWith<$Res>  {
$BackupCheckCopyWith(BackupCheck _, $Res Function(BackupCheck) __);
}


/// Adds pattern-matching-related methods to [BackupCheck].
extension BackupCheckPatterns on BackupCheck {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( BackupCheck_Ready value)?  ready,TResult Function( BackupCheck_Missing value)?  missing,TResult Function( BackupCheck_NotStock value)?  notStock,required TResult orElse(),}){
final _that = this;
switch (_that) {
case BackupCheck_Ready() when ready != null:
return ready(_that);case BackupCheck_Missing() when missing != null:
return missing(_that);case BackupCheck_NotStock() when notStock != null:
return notStock(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( BackupCheck_Ready value)  ready,required TResult Function( BackupCheck_Missing value)  missing,required TResult Function( BackupCheck_NotStock value)  notStock,}){
final _that = this;
switch (_that) {
case BackupCheck_Ready():
return ready(_that);case BackupCheck_Missing():
return missing(_that);case BackupCheck_NotStock():
return notStock(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( BackupCheck_Ready value)?  ready,TResult? Function( BackupCheck_Missing value)?  missing,TResult? Function( BackupCheck_NotStock value)?  notStock,}){
final _that = this;
switch (_that) {
case BackupCheck_Ready() when ready != null:
return ready(_that);case BackupCheck_Missing() when missing != null:
return missing(_that);case BackupCheck_NotStock() when notStock != null:
return notStock(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String version)?  ready,TResult Function()?  missing,TResult Function( String detail)?  notStock,required TResult orElse(),}) {final _that = this;
switch (_that) {
case BackupCheck_Ready() when ready != null:
return ready(_that.version);case BackupCheck_Missing() when missing != null:
return missing();case BackupCheck_NotStock() when notStock != null:
return notStock(_that.detail);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String version)  ready,required TResult Function()  missing,required TResult Function( String detail)  notStock,}) {final _that = this;
switch (_that) {
case BackupCheck_Ready():
return ready(_that.version);case BackupCheck_Missing():
return missing();case BackupCheck_NotStock():
return notStock(_that.detail);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String version)?  ready,TResult? Function()?  missing,TResult? Function( String detail)?  notStock,}) {final _that = this;
switch (_that) {
case BackupCheck_Ready() when ready != null:
return ready(_that.version);case BackupCheck_Missing() when missing != null:
return missing();case BackupCheck_NotStock() when notStock != null:
return notStock(_that.detail);case _:
  return null;

}
}

}

/// @nodoc


class BackupCheck_Ready extends BackupCheck {
  const BackupCheck_Ready({required this.version}): super._();
  

 final  String version;

/// Create a copy of BackupCheck
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$BackupCheck_ReadyCopyWith<BackupCheck_Ready> get copyWith => _$BackupCheck_ReadyCopyWithImpl<BackupCheck_Ready>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BackupCheck_Ready&&(identical(other.version, version) || other.version == version));
}


@override
int get hashCode {
    return Object.hash(runtimeType,version);
}

@override
String toString() {
    return 'BackupCheck.ready(version: $version)';
}


}

/// @nodoc
abstract mixin class $BackupCheck_ReadyCopyWith<$Res> implements $BackupCheckCopyWith<$Res> {
  factory $BackupCheck_ReadyCopyWith(BackupCheck_Ready value, $Res Function(BackupCheck_Ready) _then) = _$BackupCheck_ReadyCopyWithImpl;
@useResult
$Res call({
 String version
});




}
/// @nodoc
class _$BackupCheck_ReadyCopyWithImpl<$Res>
    implements $BackupCheck_ReadyCopyWith<$Res> {
  _$BackupCheck_ReadyCopyWithImpl(this._self, this._then);

  final BackupCheck_Ready _self;
  final $Res Function(BackupCheck_Ready) _then;

/// Create a copy of BackupCheck
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? version = null,}) {
  return _then(BackupCheck_Ready(
version: null == version ? _self.version : version // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class BackupCheck_Missing extends BackupCheck {
  const BackupCheck_Missing(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BackupCheck_Missing);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'BackupCheck.missing()';
}


}




/// @nodoc


class BackupCheck_NotStock extends BackupCheck {
  const BackupCheck_NotStock({required this.detail}): super._();
  

 final  String detail;

/// Create a copy of BackupCheck
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$BackupCheck_NotStockCopyWith<BackupCheck_NotStock> get copyWith => _$BackupCheck_NotStockCopyWithImpl<BackupCheck_NotStock>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BackupCheck_NotStock&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,detail);
}

@override
String toString() {
    return 'BackupCheck.notStock(detail: $detail)';
}


}

/// @nodoc
abstract mixin class $BackupCheck_NotStockCopyWith<$Res> implements $BackupCheckCopyWith<$Res> {
  factory $BackupCheck_NotStockCopyWith(BackupCheck_NotStock value, $Res Function(BackupCheck_NotStock) _then) = _$BackupCheck_NotStockCopyWithImpl;
@useResult
$Res call({
 String detail
});




}
/// @nodoc
class _$BackupCheck_NotStockCopyWithImpl<$Res>
    implements $BackupCheck_NotStockCopyWith<$Res> {
  _$BackupCheck_NotStockCopyWithImpl(this._self, this._then);

  final BackupCheck_NotStock _self;
  final $Res Function(BackupCheck_NotStock) _then;

/// Create a copy of BackupCheck
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? detail = null,}) {
  return _then(BackupCheck_NotStock(
detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$RestoreEnd {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreEnd);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'RestoreEnd()';
}


}

/// @nodoc
class $RestoreEndCopyWith<$Res>  {
$RestoreEndCopyWith(RestoreEnd _, $Res Function(RestoreEnd) __);
}


/// Adds pattern-matching-related methods to [RestoreEnd].
extension RestoreEndPatterns on RestoreEnd {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( RestoreEnd_Activated value)?  activated,TResult Function( RestoreEnd_Uncertain value)?  uncertain,TResult Function( RestoreEnd_Stopped value)?  stopped,required TResult orElse(),}){
final _that = this;
switch (_that) {
case RestoreEnd_Activated() when activated != null:
return activated(_that);case RestoreEnd_Uncertain() when uncertain != null:
return uncertain(_that);case RestoreEnd_Stopped() when stopped != null:
return stopped(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( RestoreEnd_Activated value)  activated,required TResult Function( RestoreEnd_Uncertain value)  uncertain,required TResult Function( RestoreEnd_Stopped value)  stopped,}){
final _that = this;
switch (_that) {
case RestoreEnd_Activated():
return activated(_that);case RestoreEnd_Uncertain():
return uncertain(_that);case RestoreEnd_Stopped():
return stopped(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( RestoreEnd_Activated value)?  activated,TResult? Function( RestoreEnd_Uncertain value)?  uncertain,TResult? Function( RestoreEnd_Stopped value)?  stopped,}){
final _that = this;
switch (_that) {
case RestoreEnd_Activated() when activated != null:
return activated(_that);case RestoreEnd_Uncertain() when uncertain != null:
return uncertain(_that);case RestoreEnd_Stopped() when stopped != null:
return stopped(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  activated,TResult Function( String detail)?  uncertain,TResult Function( RestoreStop case_,  String detail)?  stopped,required TResult orElse(),}) {final _that = this;
switch (_that) {
case RestoreEnd_Activated() when activated != null:
return activated();case RestoreEnd_Uncertain() when uncertain != null:
return uncertain(_that.detail);case RestoreEnd_Stopped() when stopped != null:
return stopped(_that.case_,_that.detail);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  activated,required TResult Function( String detail)  uncertain,required TResult Function( RestoreStop case_,  String detail)  stopped,}) {final _that = this;
switch (_that) {
case RestoreEnd_Activated():
return activated();case RestoreEnd_Uncertain():
return uncertain(_that.detail);case RestoreEnd_Stopped():
return stopped(_that.case_,_that.detail);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  activated,TResult? Function( String detail)?  uncertain,TResult? Function( RestoreStop case_,  String detail)?  stopped,}) {final _that = this;
switch (_that) {
case RestoreEnd_Activated() when activated != null:
return activated();case RestoreEnd_Uncertain() when uncertain != null:
return uncertain(_that.detail);case RestoreEnd_Stopped() when stopped != null:
return stopped(_that.case_,_that.detail);case _:
  return null;

}
}

}

/// @nodoc


class RestoreEnd_Activated extends RestoreEnd {
  const RestoreEnd_Activated(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreEnd_Activated);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'RestoreEnd.activated()';
}


}




/// @nodoc


class RestoreEnd_Uncertain extends RestoreEnd {
  const RestoreEnd_Uncertain({required this.detail}): super._();
  

 final  String detail;

/// Create a copy of RestoreEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RestoreEnd_UncertainCopyWith<RestoreEnd_Uncertain> get copyWith => _$RestoreEnd_UncertainCopyWithImpl<RestoreEnd_Uncertain>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreEnd_Uncertain&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,detail);
}

@override
String toString() {
    return 'RestoreEnd.uncertain(detail: $detail)';
}


}

/// @nodoc
abstract mixin class $RestoreEnd_UncertainCopyWith<$Res> implements $RestoreEndCopyWith<$Res> {
  factory $RestoreEnd_UncertainCopyWith(RestoreEnd_Uncertain value, $Res Function(RestoreEnd_Uncertain) _then) = _$RestoreEnd_UncertainCopyWithImpl;
@useResult
$Res call({
 String detail
});




}
/// @nodoc
class _$RestoreEnd_UncertainCopyWithImpl<$Res>
    implements $RestoreEnd_UncertainCopyWith<$Res> {
  _$RestoreEnd_UncertainCopyWithImpl(this._self, this._then);

  final RestoreEnd_Uncertain _self;
  final $Res Function(RestoreEnd_Uncertain) _then;

/// Create a copy of RestoreEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? detail = null,}) {
  return _then(RestoreEnd_Uncertain(
detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class RestoreEnd_Stopped extends RestoreEnd {
  const RestoreEnd_Stopped({required this.case_, required this.detail}): super._();
  

 final  RestoreStop case_;
/// The run's own words, for the screen's small print.
 final  String detail;

/// Create a copy of RestoreEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RestoreEnd_StoppedCopyWith<RestoreEnd_Stopped> get copyWith => _$RestoreEnd_StoppedCopyWithImpl<RestoreEnd_Stopped>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreEnd_Stopped&&(identical(other.case_, case_) || other.case_ == case_)&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,case_,detail);
}

@override
String toString() {
    return 'RestoreEnd.stopped(case_: $case_, detail: $detail)';
}


}

/// @nodoc
abstract mixin class $RestoreEnd_StoppedCopyWith<$Res> implements $RestoreEndCopyWith<$Res> {
  factory $RestoreEnd_StoppedCopyWith(RestoreEnd_Stopped value, $Res Function(RestoreEnd_Stopped) _then) = _$RestoreEnd_StoppedCopyWithImpl;
@useResult
$Res call({
 RestoreStop case_, String detail
});




}
/// @nodoc
class _$RestoreEnd_StoppedCopyWithImpl<$Res>
    implements $RestoreEnd_StoppedCopyWith<$Res> {
  _$RestoreEnd_StoppedCopyWithImpl(this._self, this._then);

  final RestoreEnd_Stopped _self;
  final $Res Function(RestoreEnd_Stopped) _then;

/// Create a copy of RestoreEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? case_ = null,Object? detail = null,}) {
  return _then(RestoreEnd_Stopped(
case_: null == case_ ? _self.case_ : case_ // ignore: cast_nullable_to_non_nullable
as RestoreStop,detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$RestoreUpdate {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreUpdate);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'RestoreUpdate()';
}


}

/// @nodoc
class $RestoreUpdateCopyWith<$Res>  {
$RestoreUpdateCopyWith(RestoreUpdate _, $Res Function(RestoreUpdate) __);
}


/// Adds pattern-matching-related methods to [RestoreUpdate].
extension RestoreUpdatePatterns on RestoreUpdate {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( RestoreUpdate_Event value)?  event,TResult Function( RestoreUpdate_Ask value)?  ask,TResult Function( RestoreUpdate_Ended value)?  ended,required TResult orElse(),}){
final _that = this;
switch (_that) {
case RestoreUpdate_Event() when event != null:
return event(_that);case RestoreUpdate_Ask() when ask != null:
return ask(_that);case RestoreUpdate_Ended() when ended != null:
return ended(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( RestoreUpdate_Event value)  event,required TResult Function( RestoreUpdate_Ask value)  ask,required TResult Function( RestoreUpdate_Ended value)  ended,}){
final _that = this;
switch (_that) {
case RestoreUpdate_Event():
return event(_that);case RestoreUpdate_Ask():
return ask(_that);case RestoreUpdate_Ended():
return ended(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( RestoreUpdate_Event value)?  event,TResult? Function( RestoreUpdate_Ask value)?  ask,TResult? Function( RestoreUpdate_Ended value)?  ended,}){
final _that = this;
switch (_that) {
case RestoreUpdate_Event() when event != null:
return event(_that);case RestoreUpdate_Ask() when ask != null:
return ask(_that);case RestoreUpdate_Ended() when ended != null:
return ended(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( InstallEvent event)?  event,TResult Function( int id,  InstallAsk ask)?  ask,TResult Function( RestoreEnd end)?  ended,required TResult orElse(),}) {final _that = this;
switch (_that) {
case RestoreUpdate_Event() when event != null:
return event(_that.event);case RestoreUpdate_Ask() when ask != null:
return ask(_that.id,_that.ask);case RestoreUpdate_Ended() when ended != null:
return ended(_that.end);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( InstallEvent event)  event,required TResult Function( int id,  InstallAsk ask)  ask,required TResult Function( RestoreEnd end)  ended,}) {final _that = this;
switch (_that) {
case RestoreUpdate_Event():
return event(_that.event);case RestoreUpdate_Ask():
return ask(_that.id,_that.ask);case RestoreUpdate_Ended():
return ended(_that.end);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( InstallEvent event)?  event,TResult? Function( int id,  InstallAsk ask)?  ask,TResult? Function( RestoreEnd end)?  ended,}) {final _that = this;
switch (_that) {
case RestoreUpdate_Event() when event != null:
return event(_that.event);case RestoreUpdate_Ask() when ask != null:
return ask(_that.id,_that.ask);case RestoreUpdate_Ended() when ended != null:
return ended(_that.end);case _:
  return null;

}
}

}

/// @nodoc


class RestoreUpdate_Event extends RestoreUpdate {
  const RestoreUpdate_Event({required this.event}): super._();
  

 final  InstallEvent event;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RestoreUpdate_EventCopyWith<RestoreUpdate_Event> get copyWith => _$RestoreUpdate_EventCopyWithImpl<RestoreUpdate_Event>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreUpdate_Event&&(identical(other.event, event) || other.event == event));
}


@override
int get hashCode {
    return Object.hash(runtimeType,event);
}

@override
String toString() {
    return 'RestoreUpdate.event(event: $event)';
}


}

/// @nodoc
abstract mixin class $RestoreUpdate_EventCopyWith<$Res> implements $RestoreUpdateCopyWith<$Res> {
  factory $RestoreUpdate_EventCopyWith(RestoreUpdate_Event value, $Res Function(RestoreUpdate_Event) _then) = _$RestoreUpdate_EventCopyWithImpl;
@useResult
$Res call({
 InstallEvent event
});


$InstallEventCopyWith<$Res> get event;

}
/// @nodoc
class _$RestoreUpdate_EventCopyWithImpl<$Res>
    implements $RestoreUpdate_EventCopyWith<$Res> {
  _$RestoreUpdate_EventCopyWithImpl(this._self, this._then);

  final RestoreUpdate_Event _self;
  final $Res Function(RestoreUpdate_Event) _then;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? event = null,}) {
  return _then(RestoreUpdate_Event(
event: null == event ? _self.event : event // ignore: cast_nullable_to_non_nullable
as InstallEvent,
  ));
}

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$InstallEventCopyWith<$Res> get event {
  
  return $InstallEventCopyWith<$Res>(_self.event, (value) {
    return _then(_self.copyWith(event: value));
  });
}
}

/// @nodoc


class RestoreUpdate_Ask extends RestoreUpdate {
  const RestoreUpdate_Ask({required this.id, required this.ask}): super._();
  

 final  int id;
 final  InstallAsk ask;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RestoreUpdate_AskCopyWith<RestoreUpdate_Ask> get copyWith => _$RestoreUpdate_AskCopyWithImpl<RestoreUpdate_Ask>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreUpdate_Ask&&(identical(other.id, id) || other.id == id)&&(identical(other.ask, ask) || other.ask == ask));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,ask);
}

@override
String toString() {
    return 'RestoreUpdate.ask(id: $id, ask: $ask)';
}


}

/// @nodoc
abstract mixin class $RestoreUpdate_AskCopyWith<$Res> implements $RestoreUpdateCopyWith<$Res> {
  factory $RestoreUpdate_AskCopyWith(RestoreUpdate_Ask value, $Res Function(RestoreUpdate_Ask) _then) = _$RestoreUpdate_AskCopyWithImpl;
@useResult
$Res call({
 int id, InstallAsk ask
});


$InstallAskCopyWith<$Res> get ask;

}
/// @nodoc
class _$RestoreUpdate_AskCopyWithImpl<$Res>
    implements $RestoreUpdate_AskCopyWith<$Res> {
  _$RestoreUpdate_AskCopyWithImpl(this._self, this._then);

  final RestoreUpdate_Ask _self;
  final $Res Function(RestoreUpdate_Ask) _then;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? ask = null,}) {
  return _then(RestoreUpdate_Ask(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as int,ask: null == ask ? _self.ask : ask // ignore: cast_nullable_to_non_nullable
as InstallAsk,
  ));
}

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$InstallAskCopyWith<$Res> get ask {
  
  return $InstallAskCopyWith<$Res>(_self.ask, (value) {
    return _then(_self.copyWith(ask: value));
  });
}
}

/// @nodoc


class RestoreUpdate_Ended extends RestoreUpdate {
  const RestoreUpdate_Ended({required this.end}): super._();
  

 final  RestoreEnd end;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$RestoreUpdate_EndedCopyWith<RestoreUpdate_Ended> get copyWith => _$RestoreUpdate_EndedCopyWithImpl<RestoreUpdate_Ended>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is RestoreUpdate_Ended&&(identical(other.end, end) || other.end == end));
}


@override
int get hashCode {
    return Object.hash(runtimeType,end);
}

@override
String toString() {
    return 'RestoreUpdate.ended(end: $end)';
}


}

/// @nodoc
abstract mixin class $RestoreUpdate_EndedCopyWith<$Res> implements $RestoreUpdateCopyWith<$Res> {
  factory $RestoreUpdate_EndedCopyWith(RestoreUpdate_Ended value, $Res Function(RestoreUpdate_Ended) _then) = _$RestoreUpdate_EndedCopyWithImpl;
@useResult
$Res call({
 RestoreEnd end
});


$RestoreEndCopyWith<$Res> get end;

}
/// @nodoc
class _$RestoreUpdate_EndedCopyWithImpl<$Res>
    implements $RestoreUpdate_EndedCopyWith<$Res> {
  _$RestoreUpdate_EndedCopyWithImpl(this._self, this._then);

  final RestoreUpdate_Ended _self;
  final $Res Function(RestoreUpdate_Ended) _then;

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? end = null,}) {
  return _then(RestoreUpdate_Ended(
end: null == end ? _self.end : end // ignore: cast_nullable_to_non_nullable
as RestoreEnd,
  ));
}

/// Create a copy of RestoreUpdate
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$RestoreEndCopyWith<$Res> get end {
  
  return $RestoreEndCopyWith<$Res>(_self.end, (value) {
    return _then(_self.copyWith(end: value));
  });
}
}

// dart format on
