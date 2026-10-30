// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'model.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$CallClip {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is CallClip);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'CallClip()';
}


}

/// @nodoc
class $CallClipCopyWith<$Res>  {
$CallClipCopyWith(CallClip _, $Res Function(CallClip) __);
}


/// Adds pattern-matching-related methods to [CallClip].
extension CallClipPatterns on CallClip {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( CallClip_None value)?  none,TResult Function( CallClip_Receiving value)?  receiving,TResult Function( CallClip_Stored value)?  stored,required TResult orElse(),}){
final _that = this;
switch (_that) {
case CallClip_None() when none != null:
return none(_that);case CallClip_Receiving() when receiving != null:
return receiving(_that);case CallClip_Stored() when stored != null:
return stored(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( CallClip_None value)  none,required TResult Function( CallClip_Receiving value)  receiving,required TResult Function( CallClip_Stored value)  stored,}){
final _that = this;
switch (_that) {
case CallClip_None():
return none(_that);case CallClip_Receiving():
return receiving(_that);case CallClip_Stored():
return stored(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( CallClip_None value)?  none,TResult? Function( CallClip_Receiving value)?  receiving,TResult? Function( CallClip_Stored value)?  stored,}){
final _that = this;
switch (_that) {
case CallClip_None() when none != null:
return none(_that);case CallClip_Receiving() when receiving != null:
return receiving(_that);case CallClip_Stored() when stored != null:
return stored(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  none,TResult Function( int bytes,  int crc,  int received)?  receiving,TResult Function( int bytes,  int crc)?  stored,required TResult orElse(),}) {final _that = this;
switch (_that) {
case CallClip_None() when none != null:
return none();case CallClip_Receiving() when receiving != null:
return receiving(_that.bytes,_that.crc,_that.received);case CallClip_Stored() when stored != null:
return stored(_that.bytes,_that.crc);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  none,required TResult Function( int bytes,  int crc,  int received)  receiving,required TResult Function( int bytes,  int crc)  stored,}) {final _that = this;
switch (_that) {
case CallClip_None():
return none();case CallClip_Receiving():
return receiving(_that.bytes,_that.crc,_that.received);case CallClip_Stored():
return stored(_that.bytes,_that.crc);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  none,TResult? Function( int bytes,  int crc,  int received)?  receiving,TResult? Function( int bytes,  int crc)?  stored,}) {final _that = this;
switch (_that) {
case CallClip_None() when none != null:
return none();case CallClip_Receiving() when receiving != null:
return receiving(_that.bytes,_that.crc,_that.received);case CallClip_Stored() when stored != null:
return stored(_that.bytes,_that.crc);case _:
  return null;

}
}

}

/// @nodoc


class CallClip_None extends CallClip {
  const CallClip_None(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is CallClip_None);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'CallClip.none()';
}


}




/// @nodoc


class CallClip_Receiving extends CallClip {
  const CallClip_Receiving({required this.bytes, required this.crc, required this.received}): super._();
  

 final  int bytes;
 final  int crc;
 final  int received;

/// Create a copy of CallClip
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CallClip_ReceivingCopyWith<CallClip_Receiving> get copyWith => _$CallClip_ReceivingCopyWithImpl<CallClip_Receiving>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is CallClip_Receiving&&(identical(other.bytes, bytes) || other.bytes == bytes)&&(identical(other.crc, crc) || other.crc == crc)&&(identical(other.received, received) || other.received == received));
}


@override
int get hashCode {
    return Object.hash(runtimeType,bytes,crc,received);
}

@override
String toString() {
    return 'CallClip.receiving(bytes: $bytes, crc: $crc, received: $received)';
}


}

/// @nodoc
abstract mixin class $CallClip_ReceivingCopyWith<$Res> implements $CallClipCopyWith<$Res> {
  factory $CallClip_ReceivingCopyWith(CallClip_Receiving value, $Res Function(CallClip_Receiving) _then) = _$CallClip_ReceivingCopyWithImpl;
@useResult
$Res call({
 int bytes, int crc, int received
});




}
/// @nodoc
class _$CallClip_ReceivingCopyWithImpl<$Res>
    implements $CallClip_ReceivingCopyWith<$Res> {
  _$CallClip_ReceivingCopyWithImpl(this._self, this._then);

  final CallClip_Receiving _self;
  final $Res Function(CallClip_Receiving) _then;

/// Create a copy of CallClip
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? bytes = null,Object? crc = null,Object? received = null,}) {
  return _then(CallClip_Receiving(
bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as int,crc: null == crc ? _self.crc : crc // ignore: cast_nullable_to_non_nullable
as int,received: null == received ? _self.received : received // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class CallClip_Stored extends CallClip {
  const CallClip_Stored({required this.bytes, required this.crc}): super._();
  

 final  int bytes;
 final  int crc;

/// Create a copy of CallClip
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$CallClip_StoredCopyWith<CallClip_Stored> get copyWith => _$CallClip_StoredCopyWithImpl<CallClip_Stored>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is CallClip_Stored&&(identical(other.bytes, bytes) || other.bytes == bytes)&&(identical(other.crc, crc) || other.crc == crc));
}


@override
int get hashCode {
    return Object.hash(runtimeType,bytes,crc);
}

@override
String toString() {
    return 'CallClip.stored(bytes: $bytes, crc: $crc)';
}


}

/// @nodoc
abstract mixin class $CallClip_StoredCopyWith<$Res> implements $CallClipCopyWith<$Res> {
  factory $CallClip_StoredCopyWith(CallClip_Stored value, $Res Function(CallClip_Stored) _then) = _$CallClip_StoredCopyWithImpl;
@useResult
$Res call({
 int bytes, int crc
});




}
/// @nodoc
class _$CallClip_StoredCopyWithImpl<$Res>
    implements $CallClip_StoredCopyWith<$Res> {
  _$CallClip_StoredCopyWithImpl(this._self, this._then);

  final CallClip_Stored _self;
  final $Res Function(CallClip_Stored) _then;

/// Create a copy of CallClip
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? bytes = null,Object? crc = null,}) {
  return _then(CallClip_Stored(
bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as int,crc: null == crc ? _self.crc : crc // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc
mixin _$Desiccant {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Desiccant);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'Desiccant()';
}


}

/// @nodoc
class $DesiccantCopyWith<$Res>  {
$DesiccantCopyWith(Desiccant _, $Res Function(Desiccant) __);
}


/// Adds pattern-matching-related methods to [Desiccant].
extension DesiccantPatterns on Desiccant {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Desiccant_Unset value)?  unset,TResult Function( Desiccant_Unknown value)?  unknown,TResult Function( Desiccant_Remaining value)?  remaining,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Desiccant_Unset() when unset != null:
return unset(_that);case Desiccant_Unknown() when unknown != null:
return unknown(_that);case Desiccant_Remaining() when remaining != null:
return remaining(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Desiccant_Unset value)  unset,required TResult Function( Desiccant_Unknown value)  unknown,required TResult Function( Desiccant_Remaining value)  remaining,}){
final _that = this;
switch (_that) {
case Desiccant_Unset():
return unset(_that);case Desiccant_Unknown():
return unknown(_that);case Desiccant_Remaining():
return remaining(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Desiccant_Unset value)?  unset,TResult? Function( Desiccant_Unknown value)?  unknown,TResult? Function( Desiccant_Remaining value)?  remaining,}){
final _that = this;
switch (_that) {
case Desiccant_Unset() when unset != null:
return unset(_that);case Desiccant_Unknown() when unknown != null:
return unknown(_that);case Desiccant_Remaining() when remaining != null:
return remaining(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  unset,TResult Function()?  unknown,TResult Function( int days,  bool due)?  remaining,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Desiccant_Unset() when unset != null:
return unset();case Desiccant_Unknown() when unknown != null:
return unknown();case Desiccant_Remaining() when remaining != null:
return remaining(_that.days,_that.due);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  unset,required TResult Function()  unknown,required TResult Function( int days,  bool due)  remaining,}) {final _that = this;
switch (_that) {
case Desiccant_Unset():
return unset();case Desiccant_Unknown():
return unknown();case Desiccant_Remaining():
return remaining(_that.days,_that.due);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  unset,TResult? Function()?  unknown,TResult? Function( int days,  bool due)?  remaining,}) {final _that = this;
switch (_that) {
case Desiccant_Unset() when unset != null:
return unset();case Desiccant_Unknown() when unknown != null:
return unknown();case Desiccant_Remaining() when remaining != null:
return remaining(_that.days,_that.due);case _:
  return null;

}
}

}

/// @nodoc


class Desiccant_Unset extends Desiccant {
  const Desiccant_Unset(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Desiccant_Unset);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'Desiccant.unset()';
}


}




/// @nodoc


class Desiccant_Unknown extends Desiccant {
  const Desiccant_Unknown(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Desiccant_Unknown);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'Desiccant.unknown()';
}


}




/// @nodoc


class Desiccant_Remaining extends Desiccant {
  const Desiccant_Remaining({required this.days, required this.due}): super._();
  

 final  int days;
 final  bool due;

/// Create a copy of Desiccant
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Desiccant_RemainingCopyWith<Desiccant_Remaining> get copyWith => _$Desiccant_RemainingCopyWithImpl<Desiccant_Remaining>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Desiccant_Remaining&&(identical(other.days, days) || other.days == days)&&(identical(other.due, due) || other.due == due));
}


@override
int get hashCode {
    return Object.hash(runtimeType,days,due);
}

@override
String toString() {
    return 'Desiccant.remaining(days: $days, due: $due)';
}


}

/// @nodoc
abstract mixin class $Desiccant_RemainingCopyWith<$Res> implements $DesiccantCopyWith<$Res> {
  factory $Desiccant_RemainingCopyWith(Desiccant_Remaining value, $Res Function(Desiccant_Remaining) _then) = _$Desiccant_RemainingCopyWithImpl;
@useResult
$Res call({
 int days, bool due
});




}
/// @nodoc
class _$Desiccant_RemainingCopyWithImpl<$Res>
    implements $Desiccant_RemainingCopyWith<$Res> {
  _$Desiccant_RemainingCopyWithImpl(this._self, this._then);

  final Desiccant_Remaining _self;
  final $Res Function(Desiccant_Remaining) _then;

/// Create a copy of Desiccant
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? days = null,Object? due = null,}) {
  return _then(Desiccant_Remaining(
days: null == days ? _self.days : days // ignore: cast_nullable_to_non_nullable
as int,due: null == due ? _self.due : due // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc
mixin _$MealReminder {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealReminder);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MealReminder()';
}


}

/// @nodoc
class $MealReminderCopyWith<$Res>  {
$MealReminderCopyWith(MealReminder _, $Res Function(MealReminder) __);
}


/// Adds pattern-matching-related methods to [MealReminder].
extension MealReminderPatterns on MealReminder {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( MealReminder_Off value)?  off,TResult Function( MealReminder_Before value)?  before,required TResult orElse(),}){
final _that = this;
switch (_that) {
case MealReminder_Off() when off != null:
return off(_that);case MealReminder_Before() when before != null:
return before(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( MealReminder_Off value)  off,required TResult Function( MealReminder_Before value)  before,}){
final _that = this;
switch (_that) {
case MealReminder_Off():
return off(_that);case MealReminder_Before():
return before(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( MealReminder_Off value)?  off,TResult? Function( MealReminder_Before value)?  before,}){
final _that = this;
switch (_that) {
case MealReminder_Off() when off != null:
return off(_that);case MealReminder_Before() when before != null:
return before(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  off,TResult Function( int minutes)?  before,required TResult orElse(),}) {final _that = this;
switch (_that) {
case MealReminder_Off() when off != null:
return off();case MealReminder_Before() when before != null:
return before(_that.minutes);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  off,required TResult Function( int minutes)  before,}) {final _that = this;
switch (_that) {
case MealReminder_Off():
return off();case MealReminder_Before():
return before(_that.minutes);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  off,TResult? Function( int minutes)?  before,}) {final _that = this;
switch (_that) {
case MealReminder_Off() when off != null:
return off();case MealReminder_Before() when before != null:
return before(_that.minutes);case _:
  return null;

}
}

}

/// @nodoc


class MealReminder_Off extends MealReminder {
  const MealReminder_Off(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealReminder_Off);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MealReminder.off()';
}


}




/// @nodoc


class MealReminder_Before extends MealReminder {
  const MealReminder_Before({required this.minutes}): super._();
  

 final  int minutes;

/// Create a copy of MealReminder
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MealReminder_BeforeCopyWith<MealReminder_Before> get copyWith => _$MealReminder_BeforeCopyWithImpl<MealReminder_Before>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealReminder_Before&&(identical(other.minutes, minutes) || other.minutes == minutes));
}


@override
int get hashCode {
    return Object.hash(runtimeType,minutes);
}

@override
String toString() {
    return 'MealReminder.before(minutes: $minutes)';
}


}

/// @nodoc
abstract mixin class $MealReminder_BeforeCopyWith<$Res> implements $MealReminderCopyWith<$Res> {
  factory $MealReminder_BeforeCopyWith(MealReminder_Before value, $Res Function(MealReminder_Before) _then) = _$MealReminder_BeforeCopyWithImpl;
@useResult
$Res call({
 int minutes
});




}
/// @nodoc
class _$MealReminder_BeforeCopyWithImpl<$Res>
    implements $MealReminder_BeforeCopyWith<$Res> {
  _$MealReminder_BeforeCopyWithImpl(this._self, this._then);

  final MealReminder_Before _self;
  final $Res Function(MealReminder_Before) _then;

/// Create a copy of MealReminder
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? minutes = null,}) {
  return _then(MealReminder_Before(
minutes: null == minutes ? _self.minutes : minutes // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc
mixin _$MissedReason {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MissedReason);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MissedReason()';
}


}

/// @nodoc
class $MissedReasonCopyWith<$Res>  {
$MissedReasonCopyWith(MissedReason _, $Res Function(MissedReason) __);
}


/// Adds pattern-matching-related methods to [MissedReason].
extension MissedReasonPatterns on MissedReason {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( MissedReason_Late value)?  late_,TResult Function( MissedReason_Blocked value)?  blocked,TResult Function( MissedReason_Interrupted value)?  interrupted,required TResult orElse(),}){
final _that = this;
switch (_that) {
case MissedReason_Late() when late_ != null:
return late_(_that);case MissedReason_Blocked() when blocked != null:
return blocked(_that);case MissedReason_Interrupted() when interrupted != null:
return interrupted(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( MissedReason_Late value)  late_,required TResult Function( MissedReason_Blocked value)  blocked,required TResult Function( MissedReason_Interrupted value)  interrupted,}){
final _that = this;
switch (_that) {
case MissedReason_Late():
return late_(_that);case MissedReason_Blocked():
return blocked(_that);case MissedReason_Interrupted():
return interrupted(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( MissedReason_Late value)?  late_,TResult? Function( MissedReason_Blocked value)?  blocked,TResult? Function( MissedReason_Interrupted value)?  interrupted,}){
final _that = this;
switch (_that) {
case MissedReason_Late() when late_ != null:
return late_(_that);case MissedReason_Blocked() when blocked != null:
return blocked(_that);case MissedReason_Interrupted() when interrupted != null:
return interrupted(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  late_,TResult Function( Blocked reason)?  blocked,TResult Function()?  interrupted,required TResult orElse(),}) {final _that = this;
switch (_that) {
case MissedReason_Late() when late_ != null:
return late_();case MissedReason_Blocked() when blocked != null:
return blocked(_that.reason);case MissedReason_Interrupted() when interrupted != null:
return interrupted();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  late_,required TResult Function( Blocked reason)  blocked,required TResult Function()  interrupted,}) {final _that = this;
switch (_that) {
case MissedReason_Late():
return late_();case MissedReason_Blocked():
return blocked(_that.reason);case MissedReason_Interrupted():
return interrupted();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  late_,TResult? Function( Blocked reason)?  blocked,TResult? Function()?  interrupted,}) {final _that = this;
switch (_that) {
case MissedReason_Late() when late_ != null:
return late_();case MissedReason_Blocked() when blocked != null:
return blocked(_that.reason);case MissedReason_Interrupted() when interrupted != null:
return interrupted();case _:
  return null;

}
}

}

/// @nodoc


class MissedReason_Late extends MissedReason {
  const MissedReason_Late(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MissedReason_Late);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MissedReason.late_()';
}


}




/// @nodoc


class MissedReason_Blocked extends MissedReason {
  const MissedReason_Blocked({required this.reason}): super._();
  

 final  Blocked reason;

/// Create a copy of MissedReason
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MissedReason_BlockedCopyWith<MissedReason_Blocked> get copyWith => _$MissedReason_BlockedCopyWithImpl<MissedReason_Blocked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MissedReason_Blocked&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'MissedReason.blocked(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $MissedReason_BlockedCopyWith<$Res> implements $MissedReasonCopyWith<$Res> {
  factory $MissedReason_BlockedCopyWith(MissedReason_Blocked value, $Res Function(MissedReason_Blocked) _then) = _$MissedReason_BlockedCopyWithImpl;
@useResult
$Res call({
 Blocked reason
});




}
/// @nodoc
class _$MissedReason_BlockedCopyWithImpl<$Res>
    implements $MissedReason_BlockedCopyWith<$Res> {
  _$MissedReason_BlockedCopyWithImpl(this._self, this._then);

  final MissedReason_Blocked _self;
  final $Res Function(MissedReason_Blocked) _then;

/// Create a copy of MissedReason
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(MissedReason_Blocked(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as Blocked,
  ));
}


}

/// @nodoc


class MissedReason_Interrupted extends MissedReason {
  const MissedReason_Interrupted(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MissedReason_Interrupted);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MissedReason.interrupted()';
}


}




/// @nodoc
mixin _$Stats {

 Object get field0;



@override
bool operator ==(Object other) {
  final _this = this as Stats;
  return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats&&const DeepCollectionEquality().equals(other.field0, _this.field0));
}


@override
int get hashCode {
  final _this = this as Stats;
  return Object.hash(runtimeType,const DeepCollectionEquality().hash(_this.field0));
}

@override
String toString() {
  final _this = this as Stats;
  return 'Stats(field0: ${_this.field0})';
}


}

/// @nodoc
class $StatsCopyWith<$Res>  {
$StatsCopyWith(Stats _, $Res Function(Stats) __);
}


/// Adds pattern-matching-related methods to [Stats].
extension StatsPatterns on Stats {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Stats_Counters value)?  counters,TResult Function( Stats_Memory value)?  memory,TResult Function( Stats_Flash value)?  flash,TResult Function( Stats_Power value)?  power,TResult Function( Stats_Food value)?  food,TResult Function( Stats_Feeds value)?  feeds,TResult Function( Stats_Portions value)?  portions,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Stats_Counters() when counters != null:
return counters(_that);case Stats_Memory() when memory != null:
return memory(_that);case Stats_Flash() when flash != null:
return flash(_that);case Stats_Power() when power != null:
return power(_that);case Stats_Food() when food != null:
return food(_that);case Stats_Feeds() when feeds != null:
return feeds(_that);case Stats_Portions() when portions != null:
return portions(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Stats_Counters value)  counters,required TResult Function( Stats_Memory value)  memory,required TResult Function( Stats_Flash value)  flash,required TResult Function( Stats_Power value)  power,required TResult Function( Stats_Food value)  food,required TResult Function( Stats_Feeds value)  feeds,required TResult Function( Stats_Portions value)  portions,}){
final _that = this;
switch (_that) {
case Stats_Counters():
return counters(_that);case Stats_Memory():
return memory(_that);case Stats_Flash():
return flash(_that);case Stats_Power():
return power(_that);case Stats_Food():
return food(_that);case Stats_Feeds():
return feeds(_that);case Stats_Portions():
return portions(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Stats_Counters value)?  counters,TResult? Function( Stats_Memory value)?  memory,TResult? Function( Stats_Flash value)?  flash,TResult? Function( Stats_Power value)?  power,TResult? Function( Stats_Food value)?  food,TResult? Function( Stats_Feeds value)?  feeds,TResult? Function( Stats_Portions value)?  portions,}){
final _that = this;
switch (_that) {
case Stats_Counters() when counters != null:
return counters(_that);case Stats_Memory() when memory != null:
return memory(_that);case Stats_Flash() when flash != null:
return flash(_that);case Stats_Power() when power != null:
return power(_that);case Stats_Food() when food != null:
return food(_that);case Stats_Feeds() when feeds != null:
return feeds(_that);case Stats_Portions() when portions != null:
return portions(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( Counters field0)?  counters,TResult Function( Memory field0)?  memory,TResult Function( Flash field0)?  flash,TResult Function( PowerStats field0)?  power,TResult Function( Food field0)?  food,TResult Function( FeedStats field0)?  feeds,TResult Function( Portions field0)?  portions,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Stats_Counters() when counters != null:
return counters(_that.field0);case Stats_Memory() when memory != null:
return memory(_that.field0);case Stats_Flash() when flash != null:
return flash(_that.field0);case Stats_Power() when power != null:
return power(_that.field0);case Stats_Food() when food != null:
return food(_that.field0);case Stats_Feeds() when feeds != null:
return feeds(_that.field0);case Stats_Portions() when portions != null:
return portions(_that.field0);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( Counters field0)  counters,required TResult Function( Memory field0)  memory,required TResult Function( Flash field0)  flash,required TResult Function( PowerStats field0)  power,required TResult Function( Food field0)  food,required TResult Function( FeedStats field0)  feeds,required TResult Function( Portions field0)  portions,}) {final _that = this;
switch (_that) {
case Stats_Counters():
return counters(_that.field0);case Stats_Memory():
return memory(_that.field0);case Stats_Flash():
return flash(_that.field0);case Stats_Power():
return power(_that.field0);case Stats_Food():
return food(_that.field0);case Stats_Feeds():
return feeds(_that.field0);case Stats_Portions():
return portions(_that.field0);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( Counters field0)?  counters,TResult? Function( Memory field0)?  memory,TResult? Function( Flash field0)?  flash,TResult? Function( PowerStats field0)?  power,TResult? Function( Food field0)?  food,TResult? Function( FeedStats field0)?  feeds,TResult? Function( Portions field0)?  portions,}) {final _that = this;
switch (_that) {
case Stats_Counters() when counters != null:
return counters(_that.field0);case Stats_Memory() when memory != null:
return memory(_that.field0);case Stats_Flash() when flash != null:
return flash(_that.field0);case Stats_Power() when power != null:
return power(_that.field0);case Stats_Food() when food != null:
return food(_that.field0);case Stats_Feeds() when feeds != null:
return feeds(_that.field0);case Stats_Portions() when portions != null:
return portions(_that.field0);case _:
  return null;

}
}

}

/// @nodoc


class Stats_Counters extends Stats {
  const Stats_Counters(this.field0): super._();
  

@override final  Counters field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_CountersCopyWith<Stats_Counters> get copyWith => _$Stats_CountersCopyWithImpl<Stats_Counters>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Counters&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.counters(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_CountersCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_CountersCopyWith(Stats_Counters value, $Res Function(Stats_Counters) _then) = _$Stats_CountersCopyWithImpl;
@useResult
$Res call({
 Counters field0
});




}
/// @nodoc
class _$Stats_CountersCopyWithImpl<$Res>
    implements $Stats_CountersCopyWith<$Res> {
  _$Stats_CountersCopyWithImpl(this._self, this._then);

  final Stats_Counters _self;
  final $Res Function(Stats_Counters) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Counters(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Counters,
  ));
}


}

/// @nodoc


class Stats_Memory extends Stats {
  const Stats_Memory(this.field0): super._();
  

@override final  Memory field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_MemoryCopyWith<Stats_Memory> get copyWith => _$Stats_MemoryCopyWithImpl<Stats_Memory>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Memory&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.memory(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_MemoryCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_MemoryCopyWith(Stats_Memory value, $Res Function(Stats_Memory) _then) = _$Stats_MemoryCopyWithImpl;
@useResult
$Res call({
 Memory field0
});




}
/// @nodoc
class _$Stats_MemoryCopyWithImpl<$Res>
    implements $Stats_MemoryCopyWith<$Res> {
  _$Stats_MemoryCopyWithImpl(this._self, this._then);

  final Stats_Memory _self;
  final $Res Function(Stats_Memory) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Memory(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Memory,
  ));
}


}

/// @nodoc


class Stats_Flash extends Stats {
  const Stats_Flash(this.field0): super._();
  

@override final  Flash field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_FlashCopyWith<Stats_Flash> get copyWith => _$Stats_FlashCopyWithImpl<Stats_Flash>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Flash&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.flash(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_FlashCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_FlashCopyWith(Stats_Flash value, $Res Function(Stats_Flash) _then) = _$Stats_FlashCopyWithImpl;
@useResult
$Res call({
 Flash field0
});




}
/// @nodoc
class _$Stats_FlashCopyWithImpl<$Res>
    implements $Stats_FlashCopyWith<$Res> {
  _$Stats_FlashCopyWithImpl(this._self, this._then);

  final Stats_Flash _self;
  final $Res Function(Stats_Flash) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Flash(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Flash,
  ));
}


}

/// @nodoc


class Stats_Power extends Stats {
  const Stats_Power(this.field0): super._();
  

@override final  PowerStats field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_PowerCopyWith<Stats_Power> get copyWith => _$Stats_PowerCopyWithImpl<Stats_Power>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Power&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.power(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_PowerCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_PowerCopyWith(Stats_Power value, $Res Function(Stats_Power) _then) = _$Stats_PowerCopyWithImpl;
@useResult
$Res call({
 PowerStats field0
});




}
/// @nodoc
class _$Stats_PowerCopyWithImpl<$Res>
    implements $Stats_PowerCopyWith<$Res> {
  _$Stats_PowerCopyWithImpl(this._self, this._then);

  final Stats_Power _self;
  final $Res Function(Stats_Power) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Power(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as PowerStats,
  ));
}


}

/// @nodoc


class Stats_Food extends Stats {
  const Stats_Food(this.field0): super._();
  

@override final  Food field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_FoodCopyWith<Stats_Food> get copyWith => _$Stats_FoodCopyWithImpl<Stats_Food>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Food&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.food(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_FoodCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_FoodCopyWith(Stats_Food value, $Res Function(Stats_Food) _then) = _$Stats_FoodCopyWithImpl;
@useResult
$Res call({
 Food field0
});




}
/// @nodoc
class _$Stats_FoodCopyWithImpl<$Res>
    implements $Stats_FoodCopyWith<$Res> {
  _$Stats_FoodCopyWithImpl(this._self, this._then);

  final Stats_Food _self;
  final $Res Function(Stats_Food) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Food(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Food,
  ));
}


}

/// @nodoc


class Stats_Feeds extends Stats {
  const Stats_Feeds(this.field0): super._();
  

@override final  FeedStats field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_FeedsCopyWith<Stats_Feeds> get copyWith => _$Stats_FeedsCopyWithImpl<Stats_Feeds>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Feeds&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.feeds(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_FeedsCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_FeedsCopyWith(Stats_Feeds value, $Res Function(Stats_Feeds) _then) = _$Stats_FeedsCopyWithImpl;
@useResult
$Res call({
 FeedStats field0
});




}
/// @nodoc
class _$Stats_FeedsCopyWithImpl<$Res>
    implements $Stats_FeedsCopyWith<$Res> {
  _$Stats_FeedsCopyWithImpl(this._self, this._then);

  final Stats_Feeds _self;
  final $Res Function(Stats_Feeds) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Feeds(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as FeedStats,
  ));
}


}

/// @nodoc


class Stats_Portions extends Stats {
  const Stats_Portions(this.field0): super._();
  

@override final  Portions field0;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Stats_PortionsCopyWith<Stats_Portions> get copyWith => _$Stats_PortionsCopyWithImpl<Stats_Portions>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Stats_Portions&&(identical(other.field0, field0) || other.field0 == field0));
}


@override
int get hashCode {
    return Object.hash(runtimeType,field0);
}

@override
String toString() {
    return 'Stats.portions(field0: $field0)';
}


}

/// @nodoc
abstract mixin class $Stats_PortionsCopyWith<$Res> implements $StatsCopyWith<$Res> {
  factory $Stats_PortionsCopyWith(Stats_Portions value, $Res Function(Stats_Portions) _then) = _$Stats_PortionsCopyWithImpl;
@useResult
$Res call({
 Portions field0
});




}
/// @nodoc
class _$Stats_PortionsCopyWithImpl<$Res>
    implements $Stats_PortionsCopyWith<$Res> {
  _$Stats_PortionsCopyWithImpl(this._self, this._then);

  final Stats_Portions _self;
  final $Res Function(Stats_Portions) _then;

/// Create a copy of Stats
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? field0 = null,}) {
  return _then(Stats_Portions(
null == field0 ? _self.field0 : field0 // ignore: cast_nullable_to_non_nullable
as Portions,
  ));
}


}

/// @nodoc
mixin _$TraceEntry {

 int get atMs;
/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntryCopyWith<TraceEntry> get copyWith => _$TraceEntryCopyWithImpl<TraceEntry>(this as TraceEntry, _$identity);



@override
bool operator ==(Object other) {
  final _this = this as TraceEntry;
  return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry&&(identical(other.atMs, _this.atMs) || other.atMs == _this.atMs));
}


@override
int get hashCode {
  final _this = this as TraceEntry;
  return Object.hash(runtimeType,_this.atMs);
}

@override
String toString() {
  final _this = this as TraceEntry;
  return 'TraceEntry(atMs: ${_this.atMs})';
}


}

/// @nodoc
abstract mixin class $TraceEntryCopyWith<$Res>  {
  factory $TraceEntryCopyWith(TraceEntry value, $Res Function(TraceEntry) _then) = _$TraceEntryCopyWithImpl;
@useResult
$Res call({
 int atMs
});




}
/// @nodoc
class _$TraceEntryCopyWithImpl<$Res>
    implements $TraceEntryCopyWith<$Res> {
  _$TraceEntryCopyWithImpl(this._self, this._then);

  final TraceEntry _self;
  final $Res Function(TraceEntry) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') @override $Res call({Object? atMs = null,}) {
  return _then(_self.copyWith(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,
  ));
}

}


/// Adds pattern-matching-related methods to [TraceEntry].
extension TraceEntryPatterns on TraceEntry {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( TraceEntry_Current value)?  current,TResult Function( TraceEntry_Mark value)?  mark,TResult Function( TraceEntry_Beams value)?  beams,TResult Function( TraceEntry_Battery value)?  battery,TResult Function( TraceEntry_Motor value)?  motor,required TResult orElse(),}){
final _that = this;
switch (_that) {
case TraceEntry_Current() when current != null:
return current(_that);case TraceEntry_Mark() when mark != null:
return mark(_that);case TraceEntry_Beams() when beams != null:
return beams(_that);case TraceEntry_Battery() when battery != null:
return battery(_that);case TraceEntry_Motor() when motor != null:
return motor(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( TraceEntry_Current value)  current,required TResult Function( TraceEntry_Mark value)  mark,required TResult Function( TraceEntry_Beams value)  beams,required TResult Function( TraceEntry_Battery value)  battery,required TResult Function( TraceEntry_Motor value)  motor,}){
final _that = this;
switch (_that) {
case TraceEntry_Current():
return current(_that);case TraceEntry_Mark():
return mark(_that);case TraceEntry_Beams():
return beams(_that);case TraceEntry_Battery():
return battery(_that);case TraceEntry_Motor():
return motor(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( TraceEntry_Current value)?  current,TResult? Function( TraceEntry_Mark value)?  mark,TResult? Function( TraceEntry_Beams value)?  beams,TResult? Function( TraceEntry_Battery value)?  battery,TResult? Function( TraceEntry_Motor value)?  motor,}){
final _that = this;
switch (_that) {
case TraceEntry_Current() when current != null:
return current(_that);case TraceEntry_Mark() when mark != null:
return mark(_that);case TraceEntry_Beams() when beams != null:
return beams(_that);case TraceEntry_Battery() when battery != null:
return battery(_that);case TraceEntry_Motor() when motor != null:
return motor(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( int atMs,  int value)?  current,TResult Function( int atMs,  int edges,  bool high)?  mark,TResult Function( int atMs,  bool outletClear,  bool food)?  beams,TResult Function( int atMs,  int mv)?  battery,TResult Function( int atMs,  TraceMotor motor)?  motor,required TResult orElse(),}) {final _that = this;
switch (_that) {
case TraceEntry_Current() when current != null:
return current(_that.atMs,_that.value);case TraceEntry_Mark() when mark != null:
return mark(_that.atMs,_that.edges,_that.high);case TraceEntry_Beams() when beams != null:
return beams(_that.atMs,_that.outletClear,_that.food);case TraceEntry_Battery() when battery != null:
return battery(_that.atMs,_that.mv);case TraceEntry_Motor() when motor != null:
return motor(_that.atMs,_that.motor);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( int atMs,  int value)  current,required TResult Function( int atMs,  int edges,  bool high)  mark,required TResult Function( int atMs,  bool outletClear,  bool food)  beams,required TResult Function( int atMs,  int mv)  battery,required TResult Function( int atMs,  TraceMotor motor)  motor,}) {final _that = this;
switch (_that) {
case TraceEntry_Current():
return current(_that.atMs,_that.value);case TraceEntry_Mark():
return mark(_that.atMs,_that.edges,_that.high);case TraceEntry_Beams():
return beams(_that.atMs,_that.outletClear,_that.food);case TraceEntry_Battery():
return battery(_that.atMs,_that.mv);case TraceEntry_Motor():
return motor(_that.atMs,_that.motor);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( int atMs,  int value)?  current,TResult? Function( int atMs,  int edges,  bool high)?  mark,TResult? Function( int atMs,  bool outletClear,  bool food)?  beams,TResult? Function( int atMs,  int mv)?  battery,TResult? Function( int atMs,  TraceMotor motor)?  motor,}) {final _that = this;
switch (_that) {
case TraceEntry_Current() when current != null:
return current(_that.atMs,_that.value);case TraceEntry_Mark() when mark != null:
return mark(_that.atMs,_that.edges,_that.high);case TraceEntry_Beams() when beams != null:
return beams(_that.atMs,_that.outletClear,_that.food);case TraceEntry_Battery() when battery != null:
return battery(_that.atMs,_that.mv);case TraceEntry_Motor() when motor != null:
return motor(_that.atMs,_that.motor);case _:
  return null;

}
}

}

/// @nodoc


class TraceEntry_Current extends TraceEntry {
  const TraceEntry_Current({required this.atMs, required this.value}): super._();
  

@override final  int atMs;
 final  int value;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntry_CurrentCopyWith<TraceEntry_Current> get copyWith => _$TraceEntry_CurrentCopyWithImpl<TraceEntry_Current>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry_Current&&(identical(other.atMs, atMs) || other.atMs == atMs)&&(identical(other.value, value) || other.value == value));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs,value);
}

@override
String toString() {
    return 'TraceEntry.current(atMs: $atMs, value: $value)';
}


}

/// @nodoc
abstract mixin class $TraceEntry_CurrentCopyWith<$Res> implements $TraceEntryCopyWith<$Res> {
  factory $TraceEntry_CurrentCopyWith(TraceEntry_Current value, $Res Function(TraceEntry_Current) _then) = _$TraceEntry_CurrentCopyWithImpl;
@override @useResult
$Res call({
 int atMs, int value
});




}
/// @nodoc
class _$TraceEntry_CurrentCopyWithImpl<$Res>
    implements $TraceEntry_CurrentCopyWith<$Res> {
  _$TraceEntry_CurrentCopyWithImpl(this._self, this._then);

  final TraceEntry_Current _self;
  final $Res Function(TraceEntry_Current) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? atMs = null,Object? value = null,}) {
  return _then(TraceEntry_Current(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,value: null == value ? _self.value : value // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class TraceEntry_Mark extends TraceEntry {
  const TraceEntry_Mark({required this.atMs, required this.edges, required this.high}): super._();
  

@override final  int atMs;
 final  int edges;
 final  bool high;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntry_MarkCopyWith<TraceEntry_Mark> get copyWith => _$TraceEntry_MarkCopyWithImpl<TraceEntry_Mark>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry_Mark&&(identical(other.atMs, atMs) || other.atMs == atMs)&&(identical(other.edges, edges) || other.edges == edges)&&(identical(other.high, high) || other.high == high));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs,edges,high);
}

@override
String toString() {
    return 'TraceEntry.mark(atMs: $atMs, edges: $edges, high: $high)';
}


}

/// @nodoc
abstract mixin class $TraceEntry_MarkCopyWith<$Res> implements $TraceEntryCopyWith<$Res> {
  factory $TraceEntry_MarkCopyWith(TraceEntry_Mark value, $Res Function(TraceEntry_Mark) _then) = _$TraceEntry_MarkCopyWithImpl;
@override @useResult
$Res call({
 int atMs, int edges, bool high
});




}
/// @nodoc
class _$TraceEntry_MarkCopyWithImpl<$Res>
    implements $TraceEntry_MarkCopyWith<$Res> {
  _$TraceEntry_MarkCopyWithImpl(this._self, this._then);

  final TraceEntry_Mark _self;
  final $Res Function(TraceEntry_Mark) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? atMs = null,Object? edges = null,Object? high = null,}) {
  return _then(TraceEntry_Mark(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,edges: null == edges ? _self.edges : edges // ignore: cast_nullable_to_non_nullable
as int,high: null == high ? _self.high : high // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class TraceEntry_Beams extends TraceEntry {
  const TraceEntry_Beams({required this.atMs, required this.outletClear, required this.food}): super._();
  

@override final  int atMs;
 final  bool outletClear;
 final  bool food;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntry_BeamsCopyWith<TraceEntry_Beams> get copyWith => _$TraceEntry_BeamsCopyWithImpl<TraceEntry_Beams>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry_Beams&&(identical(other.atMs, atMs) || other.atMs == atMs)&&(identical(other.outletClear, outletClear) || other.outletClear == outletClear)&&(identical(other.food, food) || other.food == food));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs,outletClear,food);
}

@override
String toString() {
    return 'TraceEntry.beams(atMs: $atMs, outletClear: $outletClear, food: $food)';
}


}

/// @nodoc
abstract mixin class $TraceEntry_BeamsCopyWith<$Res> implements $TraceEntryCopyWith<$Res> {
  factory $TraceEntry_BeamsCopyWith(TraceEntry_Beams value, $Res Function(TraceEntry_Beams) _then) = _$TraceEntry_BeamsCopyWithImpl;
@override @useResult
$Res call({
 int atMs, bool outletClear, bool food
});




}
/// @nodoc
class _$TraceEntry_BeamsCopyWithImpl<$Res>
    implements $TraceEntry_BeamsCopyWith<$Res> {
  _$TraceEntry_BeamsCopyWithImpl(this._self, this._then);

  final TraceEntry_Beams _self;
  final $Res Function(TraceEntry_Beams) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? atMs = null,Object? outletClear = null,Object? food = null,}) {
  return _then(TraceEntry_Beams(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,outletClear: null == outletClear ? _self.outletClear : outletClear // ignore: cast_nullable_to_non_nullable
as bool,food: null == food ? _self.food : food // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class TraceEntry_Battery extends TraceEntry {
  const TraceEntry_Battery({required this.atMs, required this.mv}): super._();
  

@override final  int atMs;
 final  int mv;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntry_BatteryCopyWith<TraceEntry_Battery> get copyWith => _$TraceEntry_BatteryCopyWithImpl<TraceEntry_Battery>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry_Battery&&(identical(other.atMs, atMs) || other.atMs == atMs)&&(identical(other.mv, mv) || other.mv == mv));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs,mv);
}

@override
String toString() {
    return 'TraceEntry.battery(atMs: $atMs, mv: $mv)';
}


}

/// @nodoc
abstract mixin class $TraceEntry_BatteryCopyWith<$Res> implements $TraceEntryCopyWith<$Res> {
  factory $TraceEntry_BatteryCopyWith(TraceEntry_Battery value, $Res Function(TraceEntry_Battery) _then) = _$TraceEntry_BatteryCopyWithImpl;
@override @useResult
$Res call({
 int atMs, int mv
});




}
/// @nodoc
class _$TraceEntry_BatteryCopyWithImpl<$Res>
    implements $TraceEntry_BatteryCopyWith<$Res> {
  _$TraceEntry_BatteryCopyWithImpl(this._self, this._then);

  final TraceEntry_Battery _self;
  final $Res Function(TraceEntry_Battery) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? atMs = null,Object? mv = null,}) {
  return _then(TraceEntry_Battery(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,mv: null == mv ? _self.mv : mv // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class TraceEntry_Motor extends TraceEntry {
  const TraceEntry_Motor({required this.atMs, required this.motor}): super._();
  

@override final  int atMs;
 final  TraceMotor motor;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$TraceEntry_MotorCopyWith<TraceEntry_Motor> get copyWith => _$TraceEntry_MotorCopyWithImpl<TraceEntry_Motor>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is TraceEntry_Motor&&(identical(other.atMs, atMs) || other.atMs == atMs)&&(identical(other.motor, motor) || other.motor == motor));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs,motor);
}

@override
String toString() {
    return 'TraceEntry.motor(atMs: $atMs, motor: $motor)';
}


}

/// @nodoc
abstract mixin class $TraceEntry_MotorCopyWith<$Res> implements $TraceEntryCopyWith<$Res> {
  factory $TraceEntry_MotorCopyWith(TraceEntry_Motor value, $Res Function(TraceEntry_Motor) _then) = _$TraceEntry_MotorCopyWithImpl;
@override @useResult
$Res call({
 int atMs, TraceMotor motor
});




}
/// @nodoc
class _$TraceEntry_MotorCopyWithImpl<$Res>
    implements $TraceEntry_MotorCopyWith<$Res> {
  _$TraceEntry_MotorCopyWithImpl(this._self, this._then);

  final TraceEntry_Motor _self;
  final $Res Function(TraceEntry_Motor) _then;

/// Create a copy of TraceEntry
/// with the given fields replaced by the non-null parameter values.
@override @pragma('vm:prefer-inline') $Res call({Object? atMs = null,Object? motor = null,}) {
  return _then(TraceEntry_Motor(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as int,motor: null == motor ? _self.motor : motor // ignore: cast_nullable_to_non_nullable
as TraceMotor,
  ));
}


}

/// @nodoc
mixin _$Window {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Window);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'Window()';
}


}

/// @nodoc
class $WindowCopyWith<$Res>  {
$WindowCopyWith(Window _, $Res Function(Window) __);
}


/// Adds pattern-matching-related methods to [Window].
extension WindowPatterns on Window {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( Window_Always value)?  always,TResult Function( Window_Between value)?  between,required TResult orElse(),}){
final _that = this;
switch (_that) {
case Window_Always() when always != null:
return always(_that);case Window_Between() when between != null:
return between(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( Window_Always value)  always,required TResult Function( Window_Between value)  between,}){
final _that = this;
switch (_that) {
case Window_Always():
return always(_that);case Window_Between():
return between(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( Window_Always value)?  always,TResult? Function( Window_Between value)?  between,}){
final _that = this;
switch (_that) {
case Window_Always() when always != null:
return always(_that);case Window_Between() when between != null:
return between(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  always,TResult Function( int start,  int end)?  between,required TResult orElse(),}) {final _that = this;
switch (_that) {
case Window_Always() when always != null:
return always();case Window_Between() when between != null:
return between(_that.start,_that.end);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  always,required TResult Function( int start,  int end)  between,}) {final _that = this;
switch (_that) {
case Window_Always():
return always();case Window_Between():
return between(_that.start,_that.end);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  always,TResult? Function( int start,  int end)?  between,}) {final _that = this;
switch (_that) {
case Window_Always() when always != null:
return always();case Window_Between() when between != null:
return between(_that.start,_that.end);case _:
  return null;

}
}

}

/// @nodoc


class Window_Always extends Window {
  const Window_Always(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Window_Always);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'Window.always()';
}


}




/// @nodoc


class Window_Between extends Window {
  const Window_Between({required this.start, required this.end}): super._();
  

 final  int start;
 final  int end;

/// Create a copy of Window
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$Window_BetweenCopyWith<Window_Between> get copyWith => _$Window_BetweenCopyWithImpl<Window_Between>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is Window_Between&&(identical(other.start, start) || other.start == start)&&(identical(other.end, end) || other.end == end));
}


@override
int get hashCode {
    return Object.hash(runtimeType,start,end);
}

@override
String toString() {
    return 'Window.between(start: $start, end: $end)';
}


}

/// @nodoc
abstract mixin class $Window_BetweenCopyWith<$Res> implements $WindowCopyWith<$Res> {
  factory $Window_BetweenCopyWith(Window_Between value, $Res Function(Window_Between) _then) = _$Window_BetweenCopyWithImpl;
@useResult
$Res call({
 int start, int end
});




}
/// @nodoc
class _$Window_BetweenCopyWithImpl<$Res>
    implements $Window_BetweenCopyWith<$Res> {
  _$Window_BetweenCopyWithImpl(this._self, this._then);

  final Window_Between _self;
  final $Res Function(Window_Between) _then;

/// Create a copy of Window
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? start = null,Object? end = null,}) {
  return _then(Window_Between(
start: null == start ? _self.start : start // ignore: cast_nullable_to_non_nullable
as int,end: null == end ? _self.end : end // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

// dart format on
