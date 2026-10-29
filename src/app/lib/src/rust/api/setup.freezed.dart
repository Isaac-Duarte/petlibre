// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'setup.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$SetupStep {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SetupStep);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'SetupStep()';
}


}

/// @nodoc
class $SetupStepCopyWith<$Res>  {
$SetupStepCopyWith(SetupStep _, $Res Function(SetupStep) __);
}


/// Adds pattern-matching-related methods to [SetupStep].
extension SetupStepPatterns on SetupStep {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( SetupStep_Write value)?  write,TResult Function( SetupStep_EnterCode value)?  enterCode,TResult Function( SetupStep_Done value)?  done,TResult Function( SetupStep_Refused value)?  refused,required TResult orElse(),}){
final _that = this;
switch (_that) {
case SetupStep_Write() when write != null:
return write(_that);case SetupStep_EnterCode() when enterCode != null:
return enterCode(_that);case SetupStep_Done() when done != null:
return done(_that);case SetupStep_Refused() when refused != null:
return refused(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( SetupStep_Write value)  write,required TResult Function( SetupStep_EnterCode value)  enterCode,required TResult Function( SetupStep_Done value)  done,required TResult Function( SetupStep_Refused value)  refused,}){
final _that = this;
switch (_that) {
case SetupStep_Write():
return write(_that);case SetupStep_EnterCode():
return enterCode(_that);case SetupStep_Done():
return done(_that);case SetupStep_Refused():
return refused(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( SetupStep_Write value)?  write,TResult? Function( SetupStep_EnterCode value)?  enterCode,TResult? Function( SetupStep_Done value)?  done,TResult? Function( SetupStep_Refused value)?  refused,}){
final _that = this;
switch (_that) {
case SetupStep_Write() when write != null:
return write(_that);case SetupStep_EnterCode() when enterCode != null:
return enterCode(_that);case SetupStep_Done() when done != null:
return done(_that);case SetupStep_Refused() when refused != null:
return refused(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( List<Uint8List> pieces)?  write,TResult Function( int seconds)?  enterCode,TResult Function( Enrolled enrolled,  WifiSaved wifi,  Uint8List feeder)?  done,TResult Function( SetupRefusal refusal)?  refused,required TResult orElse(),}) {final _that = this;
switch (_that) {
case SetupStep_Write() when write != null:
return write(_that.pieces);case SetupStep_EnterCode() when enterCode != null:
return enterCode(_that.seconds);case SetupStep_Done() when done != null:
return done(_that.enrolled,_that.wifi,_that.feeder);case SetupStep_Refused() when refused != null:
return refused(_that.refusal);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( List<Uint8List> pieces)  write,required TResult Function( int seconds)  enterCode,required TResult Function( Enrolled enrolled,  WifiSaved wifi,  Uint8List feeder)  done,required TResult Function( SetupRefusal refusal)  refused,}) {final _that = this;
switch (_that) {
case SetupStep_Write():
return write(_that.pieces);case SetupStep_EnterCode():
return enterCode(_that.seconds);case SetupStep_Done():
return done(_that.enrolled,_that.wifi,_that.feeder);case SetupStep_Refused():
return refused(_that.refusal);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( List<Uint8List> pieces)?  write,TResult? Function( int seconds)?  enterCode,TResult? Function( Enrolled enrolled,  WifiSaved wifi,  Uint8List feeder)?  done,TResult? Function( SetupRefusal refusal)?  refused,}) {final _that = this;
switch (_that) {
case SetupStep_Write() when write != null:
return write(_that.pieces);case SetupStep_EnterCode() when enterCode != null:
return enterCode(_that.seconds);case SetupStep_Done() when done != null:
return done(_that.enrolled,_that.wifi,_that.feeder);case SetupStep_Refused() when refused != null:
return refused(_that.refusal);case _:
  return null;

}
}

}

/// @nodoc


class SetupStep_Write extends SetupStep {
  const SetupStep_Write({required  List<Uint8List> pieces}): _pieces = pieces,super._();
  

 final  List<Uint8List> _pieces;
 List<Uint8List> get pieces {
  if (_pieces is EqualUnmodifiableListView) return _pieces;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_pieces);
}


/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SetupStep_WriteCopyWith<SetupStep_Write> get copyWith => _$SetupStep_WriteCopyWithImpl<SetupStep_Write>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SetupStep_Write&&const DeepCollectionEquality().equals(other.pieces, _pieces));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_pieces));
}

@override
String toString() {
    return 'SetupStep.write(pieces: $pieces)';
}


}

/// @nodoc
abstract mixin class $SetupStep_WriteCopyWith<$Res> implements $SetupStepCopyWith<$Res> {
  factory $SetupStep_WriteCopyWith(SetupStep_Write value, $Res Function(SetupStep_Write) _then) = _$SetupStep_WriteCopyWithImpl;
@useResult
$Res call({
 List<Uint8List> pieces
});




}
/// @nodoc
class _$SetupStep_WriteCopyWithImpl<$Res>
    implements $SetupStep_WriteCopyWith<$Res> {
  _$SetupStep_WriteCopyWithImpl(this._self, this._then);

  final SetupStep_Write _self;
  final $Res Function(SetupStep_Write) _then;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? pieces = null,}) {
  return _then(SetupStep_Write(
pieces: null == pieces ? _self._pieces : pieces // ignore: cast_nullable_to_non_nullable
as List<Uint8List>,
  ));
}


}

/// @nodoc


class SetupStep_EnterCode extends SetupStep {
  const SetupStep_EnterCode({required this.seconds}): super._();
  

 final  int seconds;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SetupStep_EnterCodeCopyWith<SetupStep_EnterCode> get copyWith => _$SetupStep_EnterCodeCopyWithImpl<SetupStep_EnterCode>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SetupStep_EnterCode&&(identical(other.seconds, seconds) || other.seconds == seconds));
}


@override
int get hashCode {
    return Object.hash(runtimeType,seconds);
}

@override
String toString() {
    return 'SetupStep.enterCode(seconds: $seconds)';
}


}

/// @nodoc
abstract mixin class $SetupStep_EnterCodeCopyWith<$Res> implements $SetupStepCopyWith<$Res> {
  factory $SetupStep_EnterCodeCopyWith(SetupStep_EnterCode value, $Res Function(SetupStep_EnterCode) _then) = _$SetupStep_EnterCodeCopyWithImpl;
@useResult
$Res call({
 int seconds
});




}
/// @nodoc
class _$SetupStep_EnterCodeCopyWithImpl<$Res>
    implements $SetupStep_EnterCodeCopyWith<$Res> {
  _$SetupStep_EnterCodeCopyWithImpl(this._self, this._then);

  final SetupStep_EnterCode _self;
  final $Res Function(SetupStep_EnterCode) _then;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? seconds = null,}) {
  return _then(SetupStep_EnterCode(
seconds: null == seconds ? _self.seconds : seconds // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class SetupStep_Done extends SetupStep {
  const SetupStep_Done({required this.enrolled, required this.wifi, required this.feeder}): super._();
  

 final  Enrolled enrolled;
 final  WifiSaved wifi;
 final  Uint8List feeder;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SetupStep_DoneCopyWith<SetupStep_Done> get copyWith => _$SetupStep_DoneCopyWithImpl<SetupStep_Done>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SetupStep_Done&&(identical(other.enrolled, enrolled) || other.enrolled == enrolled)&&(identical(other.wifi, wifi) || other.wifi == wifi)&&const DeepCollectionEquality().equals(other.feeder, feeder));
}


@override
int get hashCode {
    return Object.hash(runtimeType,enrolled,wifi,const DeepCollectionEquality().hash(feeder));
}

@override
String toString() {
    return 'SetupStep.done(enrolled: $enrolled, wifi: $wifi, feeder: $feeder)';
}


}

/// @nodoc
abstract mixin class $SetupStep_DoneCopyWith<$Res> implements $SetupStepCopyWith<$Res> {
  factory $SetupStep_DoneCopyWith(SetupStep_Done value, $Res Function(SetupStep_Done) _then) = _$SetupStep_DoneCopyWithImpl;
@useResult
$Res call({
 Enrolled enrolled, WifiSaved wifi, Uint8List feeder
});




}
/// @nodoc
class _$SetupStep_DoneCopyWithImpl<$Res>
    implements $SetupStep_DoneCopyWith<$Res> {
  _$SetupStep_DoneCopyWithImpl(this._self, this._then);

  final SetupStep_Done _self;
  final $Res Function(SetupStep_Done) _then;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? enrolled = null,Object? wifi = null,Object? feeder = null,}) {
  return _then(SetupStep_Done(
enrolled: null == enrolled ? _self.enrolled : enrolled // ignore: cast_nullable_to_non_nullable
as Enrolled,wifi: null == wifi ? _self.wifi : wifi // ignore: cast_nullable_to_non_nullable
as WifiSaved,feeder: null == feeder ? _self.feeder : feeder // ignore: cast_nullable_to_non_nullable
as Uint8List,
  ));
}


}

/// @nodoc


class SetupStep_Refused extends SetupStep {
  const SetupStep_Refused({required this.refusal}): super._();
  

 final  SetupRefusal refusal;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SetupStep_RefusedCopyWith<SetupStep_Refused> get copyWith => _$SetupStep_RefusedCopyWithImpl<SetupStep_Refused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SetupStep_Refused&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'SetupStep.refused(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $SetupStep_RefusedCopyWith<$Res> implements $SetupStepCopyWith<$Res> {
  factory $SetupStep_RefusedCopyWith(SetupStep_Refused value, $Res Function(SetupStep_Refused) _then) = _$SetupStep_RefusedCopyWithImpl;
@useResult
$Res call({
 SetupRefusal refusal
});




}
/// @nodoc
class _$SetupStep_RefusedCopyWithImpl<$Res>
    implements $SetupStep_RefusedCopyWith<$Res> {
  _$SetupStep_RefusedCopyWithImpl(this._self, this._then);

  final SetupStep_Refused _self;
  final $Res Function(SetupStep_Refused) _then;

/// Create a copy of SetupStep
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = null,}) {
  return _then(SetupStep_Refused(
refusal: null == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as SetupRefusal,
  ));
}


}

// dart format on
