// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'join.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$JoinAnswer {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is JoinAnswer);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'JoinAnswer()';
}


}

/// @nodoc
class $JoinAnswerCopyWith<$Res>  {
$JoinAnswerCopyWith(JoinAnswer _, $Res Function(JoinAnswer) __);
}


/// Adds pattern-matching-related methods to [JoinAnswer].
extension JoinAnswerPatterns on JoinAnswer {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( JoinAnswer_Joined value)?  joined,TResult Function( JoinAnswer_Refused value)?  refused,required TResult orElse(),}){
final _that = this;
switch (_that) {
case JoinAnswer_Joined() when joined != null:
return joined(_that);case JoinAnswer_Refused() when refused != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( JoinAnswer_Joined value)  joined,required TResult Function( JoinAnswer_Refused value)  refused,}){
final _that = this;
switch (_that) {
case JoinAnswer_Joined():
return joined(_that);case JoinAnswer_Refused():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( JoinAnswer_Joined value)?  joined,TResult? Function( JoinAnswer_Refused value)?  refused,}){
final _that = this;
switch (_that) {
case JoinAnswer_Joined() when joined != null:
return joined(_that);case JoinAnswer_Refused() when refused != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( ClientRole role)?  joined,TResult Function( FeederRefusal refusal)?  refused,required TResult orElse(),}) {final _that = this;
switch (_that) {
case JoinAnswer_Joined() when joined != null:
return joined(_that.role);case JoinAnswer_Refused() when refused != null:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( ClientRole role)  joined,required TResult Function( FeederRefusal refusal)  refused,}) {final _that = this;
switch (_that) {
case JoinAnswer_Joined():
return joined(_that.role);case JoinAnswer_Refused():
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( ClientRole role)?  joined,TResult? Function( FeederRefusal refusal)?  refused,}) {final _that = this;
switch (_that) {
case JoinAnswer_Joined() when joined != null:
return joined(_that.role);case JoinAnswer_Refused() when refused != null:
return refused(_that.refusal);case _:
  return null;

}
}

}

/// @nodoc


class JoinAnswer_Joined extends JoinAnswer {
  const JoinAnswer_Joined({required this.role}): super._();
  

 final  ClientRole role;

/// Create a copy of JoinAnswer
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$JoinAnswer_JoinedCopyWith<JoinAnswer_Joined> get copyWith => _$JoinAnswer_JoinedCopyWithImpl<JoinAnswer_Joined>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is JoinAnswer_Joined&&(identical(other.role, role) || other.role == role));
}


@override
int get hashCode {
    return Object.hash(runtimeType,role);
}

@override
String toString() {
    return 'JoinAnswer.joined(role: $role)';
}


}

/// @nodoc
abstract mixin class $JoinAnswer_JoinedCopyWith<$Res> implements $JoinAnswerCopyWith<$Res> {
  factory $JoinAnswer_JoinedCopyWith(JoinAnswer_Joined value, $Res Function(JoinAnswer_Joined) _then) = _$JoinAnswer_JoinedCopyWithImpl;
@useResult
$Res call({
 ClientRole role
});




}
/// @nodoc
class _$JoinAnswer_JoinedCopyWithImpl<$Res>
    implements $JoinAnswer_JoinedCopyWith<$Res> {
  _$JoinAnswer_JoinedCopyWithImpl(this._self, this._then);

  final JoinAnswer_Joined _self;
  final $Res Function(JoinAnswer_Joined) _then;

/// Create a copy of JoinAnswer
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? role = null,}) {
  return _then(JoinAnswer_Joined(
role: null == role ? _self.role : role // ignore: cast_nullable_to_non_nullable
as ClientRole,
  ));
}


}

/// @nodoc


class JoinAnswer_Refused extends JoinAnswer {
  const JoinAnswer_Refused({required this.refusal}): super._();
  

 final  FeederRefusal refusal;

/// Create a copy of JoinAnswer
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$JoinAnswer_RefusedCopyWith<JoinAnswer_Refused> get copyWith => _$JoinAnswer_RefusedCopyWithImpl<JoinAnswer_Refused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is JoinAnswer_Refused&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'JoinAnswer.refused(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $JoinAnswer_RefusedCopyWith<$Res> implements $JoinAnswerCopyWith<$Res> {
  factory $JoinAnswer_RefusedCopyWith(JoinAnswer_Refused value, $Res Function(JoinAnswer_Refused) _then) = _$JoinAnswer_RefusedCopyWithImpl;
@useResult
$Res call({
 FeederRefusal refusal
});


$FeederRefusalCopyWith<$Res> get refusal;

}
/// @nodoc
class _$JoinAnswer_RefusedCopyWithImpl<$Res>
    implements $JoinAnswer_RefusedCopyWith<$Res> {
  _$JoinAnswer_RefusedCopyWithImpl(this._self, this._then);

  final JoinAnswer_Refused _self;
  final $Res Function(JoinAnswer_Refused) _then;

/// Create a copy of JoinAnswer
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = null,}) {
  return _then(JoinAnswer_Refused(
refusal: null == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as FeederRefusal,
  ));
}

/// Create a copy of JoinAnswer
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FeederRefusalCopyWith<$Res> get refusal {
  
  return $FeederRefusalCopyWith<$Res>(_self.refusal, (value) {
    return _then(_self.copyWith(refusal: value));
  });
}
}

// dart format on
