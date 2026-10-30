// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'update.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$UpdateEnd {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd()';
}


}

/// @nodoc
class $UpdateEndCopyWith<$Res>  {
$UpdateEndCopyWith(UpdateEnd _, $Res Function(UpdateEnd) __);
}


/// Adds pattern-matching-related methods to [UpdateEnd].
extension UpdateEndPatterns on UpdateEnd {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( UpdateEnd_Confirmed value)?  confirmed,TResult Function( UpdateEnd_AlreadyRunning value)?  alreadyRunning,TResult Function( UpdateEnd_Refused value)?  refused,TResult Function( UpdateEnd_Stopped value)?  stopped,TResult Function( UpdateEnd_Lost value)?  lost,TResult Function( UpdateEnd_Uncertain value)?  uncertain,TResult Function( UpdateEnd_NotOffered value)?  notOffered,TResult Function( UpdateEnd_Unconfirmed value)?  unconfirmed,TResult Function( UpdateEnd_WentBack value)?  wentBack,TResult Function( UpdateEnd_Failed value)?  failed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case UpdateEnd_Confirmed() when confirmed != null:
return confirmed(_that);case UpdateEnd_AlreadyRunning() when alreadyRunning != null:
return alreadyRunning(_that);case UpdateEnd_Refused() when refused != null:
return refused(_that);case UpdateEnd_Stopped() when stopped != null:
return stopped(_that);case UpdateEnd_Lost() when lost != null:
return lost(_that);case UpdateEnd_Uncertain() when uncertain != null:
return uncertain(_that);case UpdateEnd_NotOffered() when notOffered != null:
return notOffered(_that);case UpdateEnd_Unconfirmed() when unconfirmed != null:
return unconfirmed(_that);case UpdateEnd_WentBack() when wentBack != null:
return wentBack(_that);case UpdateEnd_Failed() when failed != null:
return failed(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( UpdateEnd_Confirmed value)  confirmed,required TResult Function( UpdateEnd_AlreadyRunning value)  alreadyRunning,required TResult Function( UpdateEnd_Refused value)  refused,required TResult Function( UpdateEnd_Stopped value)  stopped,required TResult Function( UpdateEnd_Lost value)  lost,required TResult Function( UpdateEnd_Uncertain value)  uncertain,required TResult Function( UpdateEnd_NotOffered value)  notOffered,required TResult Function( UpdateEnd_Unconfirmed value)  unconfirmed,required TResult Function( UpdateEnd_WentBack value)  wentBack,required TResult Function( UpdateEnd_Failed value)  failed,}){
final _that = this;
switch (_that) {
case UpdateEnd_Confirmed():
return confirmed(_that);case UpdateEnd_AlreadyRunning():
return alreadyRunning(_that);case UpdateEnd_Refused():
return refused(_that);case UpdateEnd_Stopped():
return stopped(_that);case UpdateEnd_Lost():
return lost(_that);case UpdateEnd_Uncertain():
return uncertain(_that);case UpdateEnd_NotOffered():
return notOffered(_that);case UpdateEnd_Unconfirmed():
return unconfirmed(_that);case UpdateEnd_WentBack():
return wentBack(_that);case UpdateEnd_Failed():
return failed(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( UpdateEnd_Confirmed value)?  confirmed,TResult? Function( UpdateEnd_AlreadyRunning value)?  alreadyRunning,TResult? Function( UpdateEnd_Refused value)?  refused,TResult? Function( UpdateEnd_Stopped value)?  stopped,TResult? Function( UpdateEnd_Lost value)?  lost,TResult? Function( UpdateEnd_Uncertain value)?  uncertain,TResult? Function( UpdateEnd_NotOffered value)?  notOffered,TResult? Function( UpdateEnd_Unconfirmed value)?  unconfirmed,TResult? Function( UpdateEnd_WentBack value)?  wentBack,TResult? Function( UpdateEnd_Failed value)?  failed,}){
final _that = this;
switch (_that) {
case UpdateEnd_Confirmed() when confirmed != null:
return confirmed(_that);case UpdateEnd_AlreadyRunning() when alreadyRunning != null:
return alreadyRunning(_that);case UpdateEnd_Refused() when refused != null:
return refused(_that);case UpdateEnd_Stopped() when stopped != null:
return stopped(_that);case UpdateEnd_Lost() when lost != null:
return lost(_that);case UpdateEnd_Uncertain() when uncertain != null:
return uncertain(_that);case UpdateEnd_NotOffered() when notOffered != null:
return notOffered(_that);case UpdateEnd_Unconfirmed() when unconfirmed != null:
return unconfirmed(_that);case UpdateEnd_WentBack() when wentBack != null:
return wentBack(_that);case UpdateEnd_Failed() when failed != null:
return failed(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  confirmed,TResult Function()?  alreadyRunning,TResult Function( FeederRefusal refusal)?  refused,TResult Function( UpdateStop stop)?  stopped,TResult Function()?  lost,TResult Function()?  uncertain,TResult Function()?  notOffered,TResult Function()?  unconfirmed,TResult Function()?  wentBack,TResult Function( String detail)?  failed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case UpdateEnd_Confirmed() when confirmed != null:
return confirmed();case UpdateEnd_AlreadyRunning() when alreadyRunning != null:
return alreadyRunning();case UpdateEnd_Refused() when refused != null:
return refused(_that.refusal);case UpdateEnd_Stopped() when stopped != null:
return stopped(_that.stop);case UpdateEnd_Lost() when lost != null:
return lost();case UpdateEnd_Uncertain() when uncertain != null:
return uncertain();case UpdateEnd_NotOffered() when notOffered != null:
return notOffered();case UpdateEnd_Unconfirmed() when unconfirmed != null:
return unconfirmed();case UpdateEnd_WentBack() when wentBack != null:
return wentBack();case UpdateEnd_Failed() when failed != null:
return failed(_that.detail);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  confirmed,required TResult Function()  alreadyRunning,required TResult Function( FeederRefusal refusal)  refused,required TResult Function( UpdateStop stop)  stopped,required TResult Function()  lost,required TResult Function()  uncertain,required TResult Function()  notOffered,required TResult Function()  unconfirmed,required TResult Function()  wentBack,required TResult Function( String detail)  failed,}) {final _that = this;
switch (_that) {
case UpdateEnd_Confirmed():
return confirmed();case UpdateEnd_AlreadyRunning():
return alreadyRunning();case UpdateEnd_Refused():
return refused(_that.refusal);case UpdateEnd_Stopped():
return stopped(_that.stop);case UpdateEnd_Lost():
return lost();case UpdateEnd_Uncertain():
return uncertain();case UpdateEnd_NotOffered():
return notOffered();case UpdateEnd_Unconfirmed():
return unconfirmed();case UpdateEnd_WentBack():
return wentBack();case UpdateEnd_Failed():
return failed(_that.detail);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  confirmed,TResult? Function()?  alreadyRunning,TResult? Function( FeederRefusal refusal)?  refused,TResult? Function( UpdateStop stop)?  stopped,TResult? Function()?  lost,TResult? Function()?  uncertain,TResult? Function()?  notOffered,TResult? Function()?  unconfirmed,TResult? Function()?  wentBack,TResult? Function( String detail)?  failed,}) {final _that = this;
switch (_that) {
case UpdateEnd_Confirmed() when confirmed != null:
return confirmed();case UpdateEnd_AlreadyRunning() when alreadyRunning != null:
return alreadyRunning();case UpdateEnd_Refused() when refused != null:
return refused(_that.refusal);case UpdateEnd_Stopped() when stopped != null:
return stopped(_that.stop);case UpdateEnd_Lost() when lost != null:
return lost();case UpdateEnd_Uncertain() when uncertain != null:
return uncertain();case UpdateEnd_NotOffered() when notOffered != null:
return notOffered();case UpdateEnd_Unconfirmed() when unconfirmed != null:
return unconfirmed();case UpdateEnd_WentBack() when wentBack != null:
return wentBack();case UpdateEnd_Failed() when failed != null:
return failed(_that.detail);case _:
  return null;

}
}

}

/// @nodoc


class UpdateEnd_Confirmed extends UpdateEnd {
  const UpdateEnd_Confirmed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Confirmed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.confirmed()';
}


}




/// @nodoc


class UpdateEnd_AlreadyRunning extends UpdateEnd {
  const UpdateEnd_AlreadyRunning(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_AlreadyRunning);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.alreadyRunning()';
}


}




/// @nodoc


class UpdateEnd_Refused extends UpdateEnd {
  const UpdateEnd_Refused({required this.refusal}): super._();
  

 final  FeederRefusal refusal;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateEnd_RefusedCopyWith<UpdateEnd_Refused> get copyWith => _$UpdateEnd_RefusedCopyWithImpl<UpdateEnd_Refused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Refused&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'UpdateEnd.refused(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $UpdateEnd_RefusedCopyWith<$Res> implements $UpdateEndCopyWith<$Res> {
  factory $UpdateEnd_RefusedCopyWith(UpdateEnd_Refused value, $Res Function(UpdateEnd_Refused) _then) = _$UpdateEnd_RefusedCopyWithImpl;
@useResult
$Res call({
 FeederRefusal refusal
});


$FeederRefusalCopyWith<$Res> get refusal;

}
/// @nodoc
class _$UpdateEnd_RefusedCopyWithImpl<$Res>
    implements $UpdateEnd_RefusedCopyWith<$Res> {
  _$UpdateEnd_RefusedCopyWithImpl(this._self, this._then);

  final UpdateEnd_Refused _self;
  final $Res Function(UpdateEnd_Refused) _then;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = null,}) {
  return _then(UpdateEnd_Refused(
refusal: null == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as FeederRefusal,
  ));
}

/// Create a copy of UpdateEnd
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


class UpdateEnd_Stopped extends UpdateEnd {
  const UpdateEnd_Stopped({required this.stop}): super._();
  

 final  UpdateStop stop;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateEnd_StoppedCopyWith<UpdateEnd_Stopped> get copyWith => _$UpdateEnd_StoppedCopyWithImpl<UpdateEnd_Stopped>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Stopped&&(identical(other.stop, stop) || other.stop == stop));
}


@override
int get hashCode {
    return Object.hash(runtimeType,stop);
}

@override
String toString() {
    return 'UpdateEnd.stopped(stop: $stop)';
}


}

/// @nodoc
abstract mixin class $UpdateEnd_StoppedCopyWith<$Res> implements $UpdateEndCopyWith<$Res> {
  factory $UpdateEnd_StoppedCopyWith(UpdateEnd_Stopped value, $Res Function(UpdateEnd_Stopped) _then) = _$UpdateEnd_StoppedCopyWithImpl;
@useResult
$Res call({
 UpdateStop stop
});




}
/// @nodoc
class _$UpdateEnd_StoppedCopyWithImpl<$Res>
    implements $UpdateEnd_StoppedCopyWith<$Res> {
  _$UpdateEnd_StoppedCopyWithImpl(this._self, this._then);

  final UpdateEnd_Stopped _self;
  final $Res Function(UpdateEnd_Stopped) _then;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? stop = null,}) {
  return _then(UpdateEnd_Stopped(
stop: null == stop ? _self.stop : stop // ignore: cast_nullable_to_non_nullable
as UpdateStop,
  ));
}


}

/// @nodoc


class UpdateEnd_Lost extends UpdateEnd {
  const UpdateEnd_Lost(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Lost);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.lost()';
}


}




/// @nodoc


class UpdateEnd_Uncertain extends UpdateEnd {
  const UpdateEnd_Uncertain(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Uncertain);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.uncertain()';
}


}




/// @nodoc


class UpdateEnd_NotOffered extends UpdateEnd {
  const UpdateEnd_NotOffered(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_NotOffered);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.notOffered()';
}


}




/// @nodoc


class UpdateEnd_Unconfirmed extends UpdateEnd {
  const UpdateEnd_Unconfirmed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Unconfirmed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.unconfirmed()';
}


}




/// @nodoc


class UpdateEnd_WentBack extends UpdateEnd {
  const UpdateEnd_WentBack(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_WentBack);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateEnd.wentBack()';
}


}




/// @nodoc


class UpdateEnd_Failed extends UpdateEnd {
  const UpdateEnd_Failed({required this.detail}): super._();
  

 final  String detail;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateEnd_FailedCopyWith<UpdateEnd_Failed> get copyWith => _$UpdateEnd_FailedCopyWithImpl<UpdateEnd_Failed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateEnd_Failed&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,detail);
}

@override
String toString() {
    return 'UpdateEnd.failed(detail: $detail)';
}


}

/// @nodoc
abstract mixin class $UpdateEnd_FailedCopyWith<$Res> implements $UpdateEndCopyWith<$Res> {
  factory $UpdateEnd_FailedCopyWith(UpdateEnd_Failed value, $Res Function(UpdateEnd_Failed) _then) = _$UpdateEnd_FailedCopyWithImpl;
@useResult
$Res call({
 String detail
});




}
/// @nodoc
class _$UpdateEnd_FailedCopyWithImpl<$Res>
    implements $UpdateEnd_FailedCopyWith<$Res> {
  _$UpdateEnd_FailedCopyWithImpl(this._self, this._then);

  final UpdateEnd_Failed _self;
  final $Res Function(UpdateEnd_Failed) _then;

/// Create a copy of UpdateEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? detail = null,}) {
  return _then(UpdateEnd_Failed(
detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$UpdateProgress {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateProgress()';
}


}

/// @nodoc
class $UpdateProgressCopyWith<$Res>  {
$UpdateProgressCopyWith(UpdateProgress _, $Res Function(UpdateProgress) __);
}


/// Adds pattern-matching-related methods to [UpdateProgress].
extension UpdateProgressPatterns on UpdateProgress {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( UpdateProgress_Sending value)?  sending,TResult Function( UpdateProgress_Reconnecting value)?  reconnecting,TResult Function( UpdateProgress_Ask value)?  ask,TResult Function( UpdateProgress_Activated value)?  activated,TResult Function( UpdateProgress_Ended value)?  ended,required TResult orElse(),}){
final _that = this;
switch (_that) {
case UpdateProgress_Sending() when sending != null:
return sending(_that);case UpdateProgress_Reconnecting() when reconnecting != null:
return reconnecting(_that);case UpdateProgress_Ask() when ask != null:
return ask(_that);case UpdateProgress_Activated() when activated != null:
return activated(_that);case UpdateProgress_Ended() when ended != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( UpdateProgress_Sending value)  sending,required TResult Function( UpdateProgress_Reconnecting value)  reconnecting,required TResult Function( UpdateProgress_Ask value)  ask,required TResult Function( UpdateProgress_Activated value)  activated,required TResult Function( UpdateProgress_Ended value)  ended,}){
final _that = this;
switch (_that) {
case UpdateProgress_Sending():
return sending(_that);case UpdateProgress_Reconnecting():
return reconnecting(_that);case UpdateProgress_Ask():
return ask(_that);case UpdateProgress_Activated():
return activated(_that);case UpdateProgress_Ended():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( UpdateProgress_Sending value)?  sending,TResult? Function( UpdateProgress_Reconnecting value)?  reconnecting,TResult? Function( UpdateProgress_Ask value)?  ask,TResult? Function( UpdateProgress_Activated value)?  activated,TResult? Function( UpdateProgress_Ended value)?  ended,}){
final _that = this;
switch (_that) {
case UpdateProgress_Sending() when sending != null:
return sending(_that);case UpdateProgress_Reconnecting() when reconnecting != null:
return reconnecting(_that);case UpdateProgress_Ask() when ask != null:
return ask(_that);case UpdateProgress_Activated() when activated != null:
return activated(_that);case UpdateProgress_Ended() when ended != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( int written,  int length)?  sending,TResult Function()?  reconnecting,TResult Function( int id,  InstallAsk ask)?  ask,TResult Function()?  activated,TResult Function( UpdateEnd end)?  ended,required TResult orElse(),}) {final _that = this;
switch (_that) {
case UpdateProgress_Sending() when sending != null:
return sending(_that.written,_that.length);case UpdateProgress_Reconnecting() when reconnecting != null:
return reconnecting();case UpdateProgress_Ask() when ask != null:
return ask(_that.id,_that.ask);case UpdateProgress_Activated() when activated != null:
return activated();case UpdateProgress_Ended() when ended != null:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( int written,  int length)  sending,required TResult Function()  reconnecting,required TResult Function( int id,  InstallAsk ask)  ask,required TResult Function()  activated,required TResult Function( UpdateEnd end)  ended,}) {final _that = this;
switch (_that) {
case UpdateProgress_Sending():
return sending(_that.written,_that.length);case UpdateProgress_Reconnecting():
return reconnecting();case UpdateProgress_Ask():
return ask(_that.id,_that.ask);case UpdateProgress_Activated():
return activated();case UpdateProgress_Ended():
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( int written,  int length)?  sending,TResult? Function()?  reconnecting,TResult? Function( int id,  InstallAsk ask)?  ask,TResult? Function()?  activated,TResult? Function( UpdateEnd end)?  ended,}) {final _that = this;
switch (_that) {
case UpdateProgress_Sending() when sending != null:
return sending(_that.written,_that.length);case UpdateProgress_Reconnecting() when reconnecting != null:
return reconnecting();case UpdateProgress_Ask() when ask != null:
return ask(_that.id,_that.ask);case UpdateProgress_Activated() when activated != null:
return activated();case UpdateProgress_Ended() when ended != null:
return ended(_that.end);case _:
  return null;

}
}

}

/// @nodoc


class UpdateProgress_Sending extends UpdateProgress {
  const UpdateProgress_Sending({required this.written, required this.length}): super._();
  

 final  int written;
 final  int length;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateProgress_SendingCopyWith<UpdateProgress_Sending> get copyWith => _$UpdateProgress_SendingCopyWithImpl<UpdateProgress_Sending>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress_Sending&&(identical(other.written, written) || other.written == written)&&(identical(other.length, length) || other.length == length));
}


@override
int get hashCode {
    return Object.hash(runtimeType,written,length);
}

@override
String toString() {
    return 'UpdateProgress.sending(written: $written, length: $length)';
}


}

/// @nodoc
abstract mixin class $UpdateProgress_SendingCopyWith<$Res> implements $UpdateProgressCopyWith<$Res> {
  factory $UpdateProgress_SendingCopyWith(UpdateProgress_Sending value, $Res Function(UpdateProgress_Sending) _then) = _$UpdateProgress_SendingCopyWithImpl;
@useResult
$Res call({
 int written, int length
});




}
/// @nodoc
class _$UpdateProgress_SendingCopyWithImpl<$Res>
    implements $UpdateProgress_SendingCopyWith<$Res> {
  _$UpdateProgress_SendingCopyWithImpl(this._self, this._then);

  final UpdateProgress_Sending _self;
  final $Res Function(UpdateProgress_Sending) _then;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? written = null,Object? length = null,}) {
  return _then(UpdateProgress_Sending(
written: null == written ? _self.written : written // ignore: cast_nullable_to_non_nullable
as int,length: null == length ? _self.length : length // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class UpdateProgress_Reconnecting extends UpdateProgress {
  const UpdateProgress_Reconnecting(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress_Reconnecting);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateProgress.reconnecting()';
}


}




/// @nodoc


class UpdateProgress_Ask extends UpdateProgress {
  const UpdateProgress_Ask({required this.id, required this.ask}): super._();
  

 final  int id;
 final  InstallAsk ask;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateProgress_AskCopyWith<UpdateProgress_Ask> get copyWith => _$UpdateProgress_AskCopyWithImpl<UpdateProgress_Ask>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress_Ask&&(identical(other.id, id) || other.id == id)&&(identical(other.ask, ask) || other.ask == ask));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,ask);
}

@override
String toString() {
    return 'UpdateProgress.ask(id: $id, ask: $ask)';
}


}

/// @nodoc
abstract mixin class $UpdateProgress_AskCopyWith<$Res> implements $UpdateProgressCopyWith<$Res> {
  factory $UpdateProgress_AskCopyWith(UpdateProgress_Ask value, $Res Function(UpdateProgress_Ask) _then) = _$UpdateProgress_AskCopyWithImpl;
@useResult
$Res call({
 int id, InstallAsk ask
});


$InstallAskCopyWith<$Res> get ask;

}
/// @nodoc
class _$UpdateProgress_AskCopyWithImpl<$Res>
    implements $UpdateProgress_AskCopyWith<$Res> {
  _$UpdateProgress_AskCopyWithImpl(this._self, this._then);

  final UpdateProgress_Ask _self;
  final $Res Function(UpdateProgress_Ask) _then;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? ask = null,}) {
  return _then(UpdateProgress_Ask(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as int,ask: null == ask ? _self.ask : ask // ignore: cast_nullable_to_non_nullable
as InstallAsk,
  ));
}

/// Create a copy of UpdateProgress
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


class UpdateProgress_Activated extends UpdateProgress {
  const UpdateProgress_Activated(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress_Activated);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'UpdateProgress.activated()';
}


}




/// @nodoc


class UpdateProgress_Ended extends UpdateProgress {
  const UpdateProgress_Ended({required this.end}): super._();
  

 final  UpdateEnd end;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$UpdateProgress_EndedCopyWith<UpdateProgress_Ended> get copyWith => _$UpdateProgress_EndedCopyWithImpl<UpdateProgress_Ended>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is UpdateProgress_Ended&&(identical(other.end, end) || other.end == end));
}


@override
int get hashCode {
    return Object.hash(runtimeType,end);
}

@override
String toString() {
    return 'UpdateProgress.ended(end: $end)';
}


}

/// @nodoc
abstract mixin class $UpdateProgress_EndedCopyWith<$Res> implements $UpdateProgressCopyWith<$Res> {
  factory $UpdateProgress_EndedCopyWith(UpdateProgress_Ended value, $Res Function(UpdateProgress_Ended) _then) = _$UpdateProgress_EndedCopyWithImpl;
@useResult
$Res call({
 UpdateEnd end
});


$UpdateEndCopyWith<$Res> get end;

}
/// @nodoc
class _$UpdateProgress_EndedCopyWithImpl<$Res>
    implements $UpdateProgress_EndedCopyWith<$Res> {
  _$UpdateProgress_EndedCopyWithImpl(this._self, this._then);

  final UpdateProgress_Ended _self;
  final $Res Function(UpdateProgress_Ended) _then;

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? end = null,}) {
  return _then(UpdateProgress_Ended(
end: null == end ? _self.end : end // ignore: cast_nullable_to_non_nullable
as UpdateEnd,
  ));
}

/// Create a copy of UpdateProgress
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$UpdateEndCopyWith<$Res> get end {
  
  return $UpdateEndCopyWith<$Res>(_self.end, (value) {
    return _then(_self.copyWith(end: value));
  });
}
}

// dart format on
