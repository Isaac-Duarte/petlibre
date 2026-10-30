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
mixin _$FedBy {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FedBy()';
}


}

/// @nodoc
class $FedByCopyWith<$Res>  {
$FedByCopyWith(FedBy _, $Res Function(FedBy) __);
}


/// Adds pattern-matching-related methods to [FedBy].
extension FedByPatterns on FedBy {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( FedBy_Panel value)?  panel,TResult Function( FedBy_Client value)?  client,TResult Function( FedBy_Feeder value)?  feeder,TResult Function( FedBy_Plan value)?  plan,TResult Function( FedBy_Unknown value)?  unknown,required TResult orElse(),}){
final _that = this;
switch (_that) {
case FedBy_Panel() when panel != null:
return panel(_that);case FedBy_Client() when client != null:
return client(_that);case FedBy_Feeder() when feeder != null:
return feeder(_that);case FedBy_Plan() when plan != null:
return plan(_that);case FedBy_Unknown() when unknown != null:
return unknown(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( FedBy_Panel value)  panel,required TResult Function( FedBy_Client value)  client,required TResult Function( FedBy_Feeder value)  feeder,required TResult Function( FedBy_Plan value)  plan,required TResult Function( FedBy_Unknown value)  unknown,}){
final _that = this;
switch (_that) {
case FedBy_Panel():
return panel(_that);case FedBy_Client():
return client(_that);case FedBy_Feeder():
return feeder(_that);case FedBy_Plan():
return plan(_that);case FedBy_Unknown():
return unknown(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( FedBy_Panel value)?  panel,TResult? Function( FedBy_Client value)?  client,TResult? Function( FedBy_Feeder value)?  feeder,TResult? Function( FedBy_Plan value)?  plan,TResult? Function( FedBy_Unknown value)?  unknown,}){
final _that = this;
switch (_that) {
case FedBy_Panel() when panel != null:
return panel(_that);case FedBy_Client() when client != null:
return client(_that);case FedBy_Feeder() when feeder != null:
return feeder(_that);case FedBy_Plan() when plan != null:
return plan(_that);case FedBy_Unknown() when unknown != null:
return unknown(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  panel,TResult Function( String fingerprint)?  client,TResult Function()?  feeder,TResult Function( int slot)?  plan,TResult Function()?  unknown,required TResult orElse(),}) {final _that = this;
switch (_that) {
case FedBy_Panel() when panel != null:
return panel();case FedBy_Client() when client != null:
return client(_that.fingerprint);case FedBy_Feeder() when feeder != null:
return feeder();case FedBy_Plan() when plan != null:
return plan(_that.slot);case FedBy_Unknown() when unknown != null:
return unknown();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  panel,required TResult Function( String fingerprint)  client,required TResult Function()  feeder,required TResult Function( int slot)  plan,required TResult Function()  unknown,}) {final _that = this;
switch (_that) {
case FedBy_Panel():
return panel();case FedBy_Client():
return client(_that.fingerprint);case FedBy_Feeder():
return feeder();case FedBy_Plan():
return plan(_that.slot);case FedBy_Unknown():
return unknown();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  panel,TResult? Function( String fingerprint)?  client,TResult? Function()?  feeder,TResult? Function( int slot)?  plan,TResult? Function()?  unknown,}) {final _that = this;
switch (_that) {
case FedBy_Panel() when panel != null:
return panel();case FedBy_Client() when client != null:
return client(_that.fingerprint);case FedBy_Feeder() when feeder != null:
return feeder();case FedBy_Plan() when plan != null:
return plan(_that.slot);case FedBy_Unknown() when unknown != null:
return unknown();case _:
  return null;

}
}

}

/// @nodoc


class FedBy_Panel extends FedBy {
  const FedBy_Panel(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy_Panel);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FedBy.panel()';
}


}




/// @nodoc


class FedBy_Client extends FedBy {
  const FedBy_Client({required this.fingerprint}): super._();
  

 final  String fingerprint;

/// Create a copy of FedBy
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FedBy_ClientCopyWith<FedBy_Client> get copyWith => _$FedBy_ClientCopyWithImpl<FedBy_Client>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy_Client&&(identical(other.fingerprint, fingerprint) || other.fingerprint == fingerprint));
}


@override
int get hashCode {
    return Object.hash(runtimeType,fingerprint);
}

@override
String toString() {
    return 'FedBy.client(fingerprint: $fingerprint)';
}


}

/// @nodoc
abstract mixin class $FedBy_ClientCopyWith<$Res> implements $FedByCopyWith<$Res> {
  factory $FedBy_ClientCopyWith(FedBy_Client value, $Res Function(FedBy_Client) _then) = _$FedBy_ClientCopyWithImpl;
@useResult
$Res call({
 String fingerprint
});




}
/// @nodoc
class _$FedBy_ClientCopyWithImpl<$Res>
    implements $FedBy_ClientCopyWith<$Res> {
  _$FedBy_ClientCopyWithImpl(this._self, this._then);

  final FedBy_Client _self;
  final $Res Function(FedBy_Client) _then;

/// Create a copy of FedBy
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fingerprint = null,}) {
  return _then(FedBy_Client(
fingerprint: null == fingerprint ? _self.fingerprint : fingerprint // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class FedBy_Feeder extends FedBy {
  const FedBy_Feeder(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy_Feeder);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FedBy.feeder()';
}


}




/// @nodoc


class FedBy_Plan extends FedBy {
  const FedBy_Plan({required this.slot}): super._();
  

 final  int slot;

/// Create a copy of FedBy
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$FedBy_PlanCopyWith<FedBy_Plan> get copyWith => _$FedBy_PlanCopyWithImpl<FedBy_Plan>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy_Plan&&(identical(other.slot, slot) || other.slot == slot));
}


@override
int get hashCode {
    return Object.hash(runtimeType,slot);
}

@override
String toString() {
    return 'FedBy.plan(slot: $slot)';
}


}

/// @nodoc
abstract mixin class $FedBy_PlanCopyWith<$Res> implements $FedByCopyWith<$Res> {
  factory $FedBy_PlanCopyWith(FedBy_Plan value, $Res Function(FedBy_Plan) _then) = _$FedBy_PlanCopyWithImpl;
@useResult
$Res call({
 int slot
});




}
/// @nodoc
class _$FedBy_PlanCopyWithImpl<$Res>
    implements $FedBy_PlanCopyWith<$Res> {
  _$FedBy_PlanCopyWithImpl(this._self, this._then);

  final FedBy_Plan _self;
  final $Res Function(FedBy_Plan) _then;

/// Create a copy of FedBy
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? slot = null,}) {
  return _then(FedBy_Plan(
slot: null == slot ? _self.slot : slot // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class FedBy_Unknown extends FedBy {
  const FedBy_Unknown(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is FedBy_Unknown);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'FedBy.unknown()';
}


}




/// @nodoc
mixin _$HistoryBody {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'HistoryBody()';
}


}

/// @nodoc
class $HistoryBodyCopyWith<$Res>  {
$HistoryBodyCopyWith(HistoryBody _, $Res Function(HistoryBody) __);
}


/// Adds pattern-matching-related methods to [HistoryBody].
extension HistoryBodyPatterns on HistoryBody {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( HistoryBody_Fed value)?  fed,TResult Function( HistoryBody_Missed value)?  missed,TResult Function( HistoryBody_Restarted value)?  restarted,TResult Function( HistoryBody_Lock value)?  lock,TResult Function( HistoryBody_SettingsChanged value)?  settingsChanged,TResult Function( HistoryBody_WifiChanged value)?  wifiChanged,TResult Function( HistoryBody_PlansChanged value)?  plansChanged,TResult Function( HistoryBody_Joined value)?  joined,TResult Function( HistoryBody_Revoked value)?  revoked,TResult Function( HistoryBody_ClockSet value)?  clockSet,TResult Function( HistoryBody_Lost value)?  lost,TResult Function( HistoryBody_FeedingBlocked value)?  feedingBlocked,TResult Function( HistoryBody_MealSoon value)?  mealSoon,TResult Function( HistoryBody_Supply value)?  supply,TResult Function( HistoryBody_DispenseRefused value)?  dispenseRefused,TResult Function( HistoryBody_Food value)?  food,TResult Function( HistoryBody_Gap value)?  gap,TResult Function( HistoryBody_Other value)?  other,required TResult orElse(),}){
final _that = this;
switch (_that) {
case HistoryBody_Fed() when fed != null:
return fed(_that);case HistoryBody_Missed() when missed != null:
return missed(_that);case HistoryBody_Restarted() when restarted != null:
return restarted(_that);case HistoryBody_Lock() when lock != null:
return lock(_that);case HistoryBody_SettingsChanged() when settingsChanged != null:
return settingsChanged(_that);case HistoryBody_WifiChanged() when wifiChanged != null:
return wifiChanged(_that);case HistoryBody_PlansChanged() when plansChanged != null:
return plansChanged(_that);case HistoryBody_Joined() when joined != null:
return joined(_that);case HistoryBody_Revoked() when revoked != null:
return revoked(_that);case HistoryBody_ClockSet() when clockSet != null:
return clockSet(_that);case HistoryBody_Lost() when lost != null:
return lost(_that);case HistoryBody_FeedingBlocked() when feedingBlocked != null:
return feedingBlocked(_that);case HistoryBody_MealSoon() when mealSoon != null:
return mealSoon(_that);case HistoryBody_Supply() when supply != null:
return supply(_that);case HistoryBody_DispenseRefused() when dispenseRefused != null:
return dispenseRefused(_that);case HistoryBody_Food() when food != null:
return food(_that);case HistoryBody_Gap() when gap != null:
return gap(_that);case HistoryBody_Other() when other != null:
return other(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( HistoryBody_Fed value)  fed,required TResult Function( HistoryBody_Missed value)  missed,required TResult Function( HistoryBody_Restarted value)  restarted,required TResult Function( HistoryBody_Lock value)  lock,required TResult Function( HistoryBody_SettingsChanged value)  settingsChanged,required TResult Function( HistoryBody_WifiChanged value)  wifiChanged,required TResult Function( HistoryBody_PlansChanged value)  plansChanged,required TResult Function( HistoryBody_Joined value)  joined,required TResult Function( HistoryBody_Revoked value)  revoked,required TResult Function( HistoryBody_ClockSet value)  clockSet,required TResult Function( HistoryBody_Lost value)  lost,required TResult Function( HistoryBody_FeedingBlocked value)  feedingBlocked,required TResult Function( HistoryBody_MealSoon value)  mealSoon,required TResult Function( HistoryBody_Supply value)  supply,required TResult Function( HistoryBody_DispenseRefused value)  dispenseRefused,required TResult Function( HistoryBody_Food value)  food,required TResult Function( HistoryBody_Gap value)  gap,required TResult Function( HistoryBody_Other value)  other,}){
final _that = this;
switch (_that) {
case HistoryBody_Fed():
return fed(_that);case HistoryBody_Missed():
return missed(_that);case HistoryBody_Restarted():
return restarted(_that);case HistoryBody_Lock():
return lock(_that);case HistoryBody_SettingsChanged():
return settingsChanged(_that);case HistoryBody_WifiChanged():
return wifiChanged(_that);case HistoryBody_PlansChanged():
return plansChanged(_that);case HistoryBody_Joined():
return joined(_that);case HistoryBody_Revoked():
return revoked(_that);case HistoryBody_ClockSet():
return clockSet(_that);case HistoryBody_Lost():
return lost(_that);case HistoryBody_FeedingBlocked():
return feedingBlocked(_that);case HistoryBody_MealSoon():
return mealSoon(_that);case HistoryBody_Supply():
return supply(_that);case HistoryBody_DispenseRefused():
return dispenseRefused(_that);case HistoryBody_Food():
return food(_that);case HistoryBody_Gap():
return gap(_that);case HistoryBody_Other():
return other(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( HistoryBody_Fed value)?  fed,TResult? Function( HistoryBody_Missed value)?  missed,TResult? Function( HistoryBody_Restarted value)?  restarted,TResult? Function( HistoryBody_Lock value)?  lock,TResult? Function( HistoryBody_SettingsChanged value)?  settingsChanged,TResult? Function( HistoryBody_WifiChanged value)?  wifiChanged,TResult? Function( HistoryBody_PlansChanged value)?  plansChanged,TResult? Function( HistoryBody_Joined value)?  joined,TResult? Function( HistoryBody_Revoked value)?  revoked,TResult? Function( HistoryBody_ClockSet value)?  clockSet,TResult? Function( HistoryBody_Lost value)?  lost,TResult? Function( HistoryBody_FeedingBlocked value)?  feedingBlocked,TResult? Function( HistoryBody_MealSoon value)?  mealSoon,TResult? Function( HistoryBody_Supply value)?  supply,TResult? Function( HistoryBody_DispenseRefused value)?  dispenseRefused,TResult? Function( HistoryBody_Food value)?  food,TResult? Function( HistoryBody_Gap value)?  gap,TResult? Function( HistoryBody_Other value)?  other,}){
final _that = this;
switch (_that) {
case HistoryBody_Fed() when fed != null:
return fed(_that);case HistoryBody_Missed() when missed != null:
return missed(_that);case HistoryBody_Restarted() when restarted != null:
return restarted(_that);case HistoryBody_Lock() when lock != null:
return lock(_that);case HistoryBody_SettingsChanged() when settingsChanged != null:
return settingsChanged(_that);case HistoryBody_WifiChanged() when wifiChanged != null:
return wifiChanged(_that);case HistoryBody_PlansChanged() when plansChanged != null:
return plansChanged(_that);case HistoryBody_Joined() when joined != null:
return joined(_that);case HistoryBody_Revoked() when revoked != null:
return revoked(_that);case HistoryBody_ClockSet() when clockSet != null:
return clockSet(_that);case HistoryBody_Lost() when lost != null:
return lost(_that);case HistoryBody_FeedingBlocked() when feedingBlocked != null:
return feedingBlocked(_that);case HistoryBody_MealSoon() when mealSoon != null:
return mealSoon(_that);case HistoryBody_Supply() when supply != null:
return supply(_that);case HistoryBody_DispenseRefused() when dispenseRefused != null:
return dispenseRefused(_that);case HistoryBody_Food() when food != null:
return food(_that);case HistoryBody_Gap() when gap != null:
return gap(_that);case HistoryBody_Other() when other != null:
return other(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( FeedResult result)?  fed,TResult Function( int slot,  PlatformInt64 dueMs,  MissedReason reason)?  missed,TResult Function()?  restarted,TResult Function( bool locked,  FedBy by)?  lock,TResult Function( FedBy by)?  settingsChanged,TResult Function( FedBy by)?  wifiChanged,TResult Function( FedBy by)?  plansChanged,TResult Function( String fingerprint)?  joined,TResult Function( String fingerprint,  FedBy by)?  revoked,TResult Function( FedBy by)?  clockSet,TResult Function( int count)?  lost,TResult Function( Blocked? reason)?  feedingBlocked,TResult Function( int minute,  int portions)?  mealSoon,TResult Function( bool? adapter)?  supply,TResult Function( Blocked reason)?  dispenseRefused,TResult Function( FoodLevel level)?  food,TResult Function( BigInt from,  BigInt to)?  gap,TResult Function()?  other,required TResult orElse(),}) {final _that = this;
switch (_that) {
case HistoryBody_Fed() when fed != null:
return fed(_that.result);case HistoryBody_Missed() when missed != null:
return missed(_that.slot,_that.dueMs,_that.reason);case HistoryBody_Restarted() when restarted != null:
return restarted();case HistoryBody_Lock() when lock != null:
return lock(_that.locked,_that.by);case HistoryBody_SettingsChanged() when settingsChanged != null:
return settingsChanged(_that.by);case HistoryBody_WifiChanged() when wifiChanged != null:
return wifiChanged(_that.by);case HistoryBody_PlansChanged() when plansChanged != null:
return plansChanged(_that.by);case HistoryBody_Joined() when joined != null:
return joined(_that.fingerprint);case HistoryBody_Revoked() when revoked != null:
return revoked(_that.fingerprint,_that.by);case HistoryBody_ClockSet() when clockSet != null:
return clockSet(_that.by);case HistoryBody_Lost() when lost != null:
return lost(_that.count);case HistoryBody_FeedingBlocked() when feedingBlocked != null:
return feedingBlocked(_that.reason);case HistoryBody_MealSoon() when mealSoon != null:
return mealSoon(_that.minute,_that.portions);case HistoryBody_Supply() when supply != null:
return supply(_that.adapter);case HistoryBody_DispenseRefused() when dispenseRefused != null:
return dispenseRefused(_that.reason);case HistoryBody_Food() when food != null:
return food(_that.level);case HistoryBody_Gap() when gap != null:
return gap(_that.from,_that.to);case HistoryBody_Other() when other != null:
return other();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( FeedResult result)  fed,required TResult Function( int slot,  PlatformInt64 dueMs,  MissedReason reason)  missed,required TResult Function()  restarted,required TResult Function( bool locked,  FedBy by)  lock,required TResult Function( FedBy by)  settingsChanged,required TResult Function( FedBy by)  wifiChanged,required TResult Function( FedBy by)  plansChanged,required TResult Function( String fingerprint)  joined,required TResult Function( String fingerprint,  FedBy by)  revoked,required TResult Function( FedBy by)  clockSet,required TResult Function( int count)  lost,required TResult Function( Blocked? reason)  feedingBlocked,required TResult Function( int minute,  int portions)  mealSoon,required TResult Function( bool? adapter)  supply,required TResult Function( Blocked reason)  dispenseRefused,required TResult Function( FoodLevel level)  food,required TResult Function( BigInt from,  BigInt to)  gap,required TResult Function()  other,}) {final _that = this;
switch (_that) {
case HistoryBody_Fed():
return fed(_that.result);case HistoryBody_Missed():
return missed(_that.slot,_that.dueMs,_that.reason);case HistoryBody_Restarted():
return restarted();case HistoryBody_Lock():
return lock(_that.locked,_that.by);case HistoryBody_SettingsChanged():
return settingsChanged(_that.by);case HistoryBody_WifiChanged():
return wifiChanged(_that.by);case HistoryBody_PlansChanged():
return plansChanged(_that.by);case HistoryBody_Joined():
return joined(_that.fingerprint);case HistoryBody_Revoked():
return revoked(_that.fingerprint,_that.by);case HistoryBody_ClockSet():
return clockSet(_that.by);case HistoryBody_Lost():
return lost(_that.count);case HistoryBody_FeedingBlocked():
return feedingBlocked(_that.reason);case HistoryBody_MealSoon():
return mealSoon(_that.minute,_that.portions);case HistoryBody_Supply():
return supply(_that.adapter);case HistoryBody_DispenseRefused():
return dispenseRefused(_that.reason);case HistoryBody_Food():
return food(_that.level);case HistoryBody_Gap():
return gap(_that.from,_that.to);case HistoryBody_Other():
return other();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( FeedResult result)?  fed,TResult? Function( int slot,  PlatformInt64 dueMs,  MissedReason reason)?  missed,TResult? Function()?  restarted,TResult? Function( bool locked,  FedBy by)?  lock,TResult? Function( FedBy by)?  settingsChanged,TResult? Function( FedBy by)?  wifiChanged,TResult? Function( FedBy by)?  plansChanged,TResult? Function( String fingerprint)?  joined,TResult? Function( String fingerprint,  FedBy by)?  revoked,TResult? Function( FedBy by)?  clockSet,TResult? Function( int count)?  lost,TResult? Function( Blocked? reason)?  feedingBlocked,TResult? Function( int minute,  int portions)?  mealSoon,TResult? Function( bool? adapter)?  supply,TResult? Function( Blocked reason)?  dispenseRefused,TResult? Function( FoodLevel level)?  food,TResult? Function( BigInt from,  BigInt to)?  gap,TResult? Function()?  other,}) {final _that = this;
switch (_that) {
case HistoryBody_Fed() when fed != null:
return fed(_that.result);case HistoryBody_Missed() when missed != null:
return missed(_that.slot,_that.dueMs,_that.reason);case HistoryBody_Restarted() when restarted != null:
return restarted();case HistoryBody_Lock() when lock != null:
return lock(_that.locked,_that.by);case HistoryBody_SettingsChanged() when settingsChanged != null:
return settingsChanged(_that.by);case HistoryBody_WifiChanged() when wifiChanged != null:
return wifiChanged(_that.by);case HistoryBody_PlansChanged() when plansChanged != null:
return plansChanged(_that.by);case HistoryBody_Joined() when joined != null:
return joined(_that.fingerprint);case HistoryBody_Revoked() when revoked != null:
return revoked(_that.fingerprint,_that.by);case HistoryBody_ClockSet() when clockSet != null:
return clockSet(_that.by);case HistoryBody_Lost() when lost != null:
return lost(_that.count);case HistoryBody_FeedingBlocked() when feedingBlocked != null:
return feedingBlocked(_that.reason);case HistoryBody_MealSoon() when mealSoon != null:
return mealSoon(_that.minute,_that.portions);case HistoryBody_Supply() when supply != null:
return supply(_that.adapter);case HistoryBody_DispenseRefused() when dispenseRefused != null:
return dispenseRefused(_that.reason);case HistoryBody_Food() when food != null:
return food(_that.level);case HistoryBody_Gap() when gap != null:
return gap(_that.from,_that.to);case HistoryBody_Other() when other != null:
return other();case _:
  return null;

}
}

}

/// @nodoc


class HistoryBody_Fed extends HistoryBody {
  const HistoryBody_Fed({required this.result}): super._();
  

 final  FeedResult result;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_FedCopyWith<HistoryBody_Fed> get copyWith => _$HistoryBody_FedCopyWithImpl<HistoryBody_Fed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Fed&&(identical(other.result, result) || other.result == result));
}


@override
int get hashCode {
    return Object.hash(runtimeType,result);
}

@override
String toString() {
    return 'HistoryBody.fed(result: $result)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_FedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_FedCopyWith(HistoryBody_Fed value, $Res Function(HistoryBody_Fed) _then) = _$HistoryBody_FedCopyWithImpl;
@useResult
$Res call({
 FeedResult result
});




}
/// @nodoc
class _$HistoryBody_FedCopyWithImpl<$Res>
    implements $HistoryBody_FedCopyWith<$Res> {
  _$HistoryBody_FedCopyWithImpl(this._self, this._then);

  final HistoryBody_Fed _self;
  final $Res Function(HistoryBody_Fed) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? result = null,}) {
  return _then(HistoryBody_Fed(
result: null == result ? _self.result : result // ignore: cast_nullable_to_non_nullable
as FeedResult,
  ));
}


}

/// @nodoc


class HistoryBody_Missed extends HistoryBody {
  const HistoryBody_Missed({required this.slot, required this.dueMs, required this.reason}): super._();
  

 final  int slot;
 final  PlatformInt64 dueMs;
 final  MissedReason reason;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_MissedCopyWith<HistoryBody_Missed> get copyWith => _$HistoryBody_MissedCopyWithImpl<HistoryBody_Missed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Missed&&(identical(other.slot, slot) || other.slot == slot)&&(identical(other.dueMs, dueMs) || other.dueMs == dueMs)&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,slot,dueMs,reason);
}

@override
String toString() {
    return 'HistoryBody.missed(slot: $slot, dueMs: $dueMs, reason: $reason)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_MissedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_MissedCopyWith(HistoryBody_Missed value, $Res Function(HistoryBody_Missed) _then) = _$HistoryBody_MissedCopyWithImpl;
@useResult
$Res call({
 int slot, PlatformInt64 dueMs, MissedReason reason
});


$MissedReasonCopyWith<$Res> get reason;

}
/// @nodoc
class _$HistoryBody_MissedCopyWithImpl<$Res>
    implements $HistoryBody_MissedCopyWith<$Res> {
  _$HistoryBody_MissedCopyWithImpl(this._self, this._then);

  final HistoryBody_Missed _self;
  final $Res Function(HistoryBody_Missed) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? slot = null,Object? dueMs = null,Object? reason = null,}) {
  return _then(HistoryBody_Missed(
slot: null == slot ? _self.slot : slot // ignore: cast_nullable_to_non_nullable
as int,dueMs: null == dueMs ? _self.dueMs : dueMs // ignore: cast_nullable_to_non_nullable
as PlatformInt64,reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as MissedReason,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$MissedReasonCopyWith<$Res> get reason {
  
  return $MissedReasonCopyWith<$Res>(_self.reason, (value) {
    return _then(_self.copyWith(reason: value));
  });
}
}

/// @nodoc


class HistoryBody_Restarted extends HistoryBody {
  const HistoryBody_Restarted(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Restarted);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'HistoryBody.restarted()';
}


}




/// @nodoc


class HistoryBody_Lock extends HistoryBody {
  const HistoryBody_Lock({required this.locked, required this.by}): super._();
  

 final  bool locked;
 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_LockCopyWith<HistoryBody_Lock> get copyWith => _$HistoryBody_LockCopyWithImpl<HistoryBody_Lock>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Lock&&(identical(other.locked, locked) || other.locked == locked)&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,locked,by);
}

@override
String toString() {
    return 'HistoryBody.lock(locked: $locked, by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_LockCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_LockCopyWith(HistoryBody_Lock value, $Res Function(HistoryBody_Lock) _then) = _$HistoryBody_LockCopyWithImpl;
@useResult
$Res call({
 bool locked, FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_LockCopyWithImpl<$Res>
    implements $HistoryBody_LockCopyWith<$Res> {
  _$HistoryBody_LockCopyWithImpl(this._self, this._then);

  final HistoryBody_Lock _self;
  final $Res Function(HistoryBody_Lock) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? locked = null,Object? by = null,}) {
  return _then(HistoryBody_Lock(
locked: null == locked ? _self.locked : locked // ignore: cast_nullable_to_non_nullable
as bool,by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_SettingsChanged extends HistoryBody {
  const HistoryBody_SettingsChanged({required this.by}): super._();
  

 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_SettingsChangedCopyWith<HistoryBody_SettingsChanged> get copyWith => _$HistoryBody_SettingsChangedCopyWithImpl<HistoryBody_SettingsChanged>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_SettingsChanged&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,by);
}

@override
String toString() {
    return 'HistoryBody.settingsChanged(by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_SettingsChangedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_SettingsChangedCopyWith(HistoryBody_SettingsChanged value, $Res Function(HistoryBody_SettingsChanged) _then) = _$HistoryBody_SettingsChangedCopyWithImpl;
@useResult
$Res call({
 FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_SettingsChangedCopyWithImpl<$Res>
    implements $HistoryBody_SettingsChangedCopyWith<$Res> {
  _$HistoryBody_SettingsChangedCopyWithImpl(this._self, this._then);

  final HistoryBody_SettingsChanged _self;
  final $Res Function(HistoryBody_SettingsChanged) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? by = null,}) {
  return _then(HistoryBody_SettingsChanged(
by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_WifiChanged extends HistoryBody {
  const HistoryBody_WifiChanged({required this.by}): super._();
  

 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_WifiChangedCopyWith<HistoryBody_WifiChanged> get copyWith => _$HistoryBody_WifiChangedCopyWithImpl<HistoryBody_WifiChanged>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_WifiChanged&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,by);
}

@override
String toString() {
    return 'HistoryBody.wifiChanged(by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_WifiChangedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_WifiChangedCopyWith(HistoryBody_WifiChanged value, $Res Function(HistoryBody_WifiChanged) _then) = _$HistoryBody_WifiChangedCopyWithImpl;
@useResult
$Res call({
 FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_WifiChangedCopyWithImpl<$Res>
    implements $HistoryBody_WifiChangedCopyWith<$Res> {
  _$HistoryBody_WifiChangedCopyWithImpl(this._self, this._then);

  final HistoryBody_WifiChanged _self;
  final $Res Function(HistoryBody_WifiChanged) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? by = null,}) {
  return _then(HistoryBody_WifiChanged(
by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_PlansChanged extends HistoryBody {
  const HistoryBody_PlansChanged({required this.by}): super._();
  

 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_PlansChangedCopyWith<HistoryBody_PlansChanged> get copyWith => _$HistoryBody_PlansChangedCopyWithImpl<HistoryBody_PlansChanged>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_PlansChanged&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,by);
}

@override
String toString() {
    return 'HistoryBody.plansChanged(by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_PlansChangedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_PlansChangedCopyWith(HistoryBody_PlansChanged value, $Res Function(HistoryBody_PlansChanged) _then) = _$HistoryBody_PlansChangedCopyWithImpl;
@useResult
$Res call({
 FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_PlansChangedCopyWithImpl<$Res>
    implements $HistoryBody_PlansChangedCopyWith<$Res> {
  _$HistoryBody_PlansChangedCopyWithImpl(this._self, this._then);

  final HistoryBody_PlansChanged _self;
  final $Res Function(HistoryBody_PlansChanged) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? by = null,}) {
  return _then(HistoryBody_PlansChanged(
by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_Joined extends HistoryBody {
  const HistoryBody_Joined({required this.fingerprint}): super._();
  

 final  String fingerprint;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_JoinedCopyWith<HistoryBody_Joined> get copyWith => _$HistoryBody_JoinedCopyWithImpl<HistoryBody_Joined>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Joined&&(identical(other.fingerprint, fingerprint) || other.fingerprint == fingerprint));
}


@override
int get hashCode {
    return Object.hash(runtimeType,fingerprint);
}

@override
String toString() {
    return 'HistoryBody.joined(fingerprint: $fingerprint)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_JoinedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_JoinedCopyWith(HistoryBody_Joined value, $Res Function(HistoryBody_Joined) _then) = _$HistoryBody_JoinedCopyWithImpl;
@useResult
$Res call({
 String fingerprint
});




}
/// @nodoc
class _$HistoryBody_JoinedCopyWithImpl<$Res>
    implements $HistoryBody_JoinedCopyWith<$Res> {
  _$HistoryBody_JoinedCopyWithImpl(this._self, this._then);

  final HistoryBody_Joined _self;
  final $Res Function(HistoryBody_Joined) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fingerprint = null,}) {
  return _then(HistoryBody_Joined(
fingerprint: null == fingerprint ? _self.fingerprint : fingerprint // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class HistoryBody_Revoked extends HistoryBody {
  const HistoryBody_Revoked({required this.fingerprint, required this.by}): super._();
  

 final  String fingerprint;
 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_RevokedCopyWith<HistoryBody_Revoked> get copyWith => _$HistoryBody_RevokedCopyWithImpl<HistoryBody_Revoked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Revoked&&(identical(other.fingerprint, fingerprint) || other.fingerprint == fingerprint)&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,fingerprint,by);
}

@override
String toString() {
    return 'HistoryBody.revoked(fingerprint: $fingerprint, by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_RevokedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_RevokedCopyWith(HistoryBody_Revoked value, $Res Function(HistoryBody_Revoked) _then) = _$HistoryBody_RevokedCopyWithImpl;
@useResult
$Res call({
 String fingerprint, FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_RevokedCopyWithImpl<$Res>
    implements $HistoryBody_RevokedCopyWith<$Res> {
  _$HistoryBody_RevokedCopyWithImpl(this._self, this._then);

  final HistoryBody_Revoked _self;
  final $Res Function(HistoryBody_Revoked) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fingerprint = null,Object? by = null,}) {
  return _then(HistoryBody_Revoked(
fingerprint: null == fingerprint ? _self.fingerprint : fingerprint // ignore: cast_nullable_to_non_nullable
as String,by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_ClockSet extends HistoryBody {
  const HistoryBody_ClockSet({required this.by}): super._();
  

 final  FedBy by;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_ClockSetCopyWith<HistoryBody_ClockSet> get copyWith => _$HistoryBody_ClockSetCopyWithImpl<HistoryBody_ClockSet>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_ClockSet&&(identical(other.by, by) || other.by == by));
}


@override
int get hashCode {
    return Object.hash(runtimeType,by);
}

@override
String toString() {
    return 'HistoryBody.clockSet(by: $by)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_ClockSetCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_ClockSetCopyWith(HistoryBody_ClockSet value, $Res Function(HistoryBody_ClockSet) _then) = _$HistoryBody_ClockSetCopyWithImpl;
@useResult
$Res call({
 FedBy by
});


$FedByCopyWith<$Res> get by;

}
/// @nodoc
class _$HistoryBody_ClockSetCopyWithImpl<$Res>
    implements $HistoryBody_ClockSetCopyWith<$Res> {
  _$HistoryBody_ClockSetCopyWithImpl(this._self, this._then);

  final HistoryBody_ClockSet _self;
  final $Res Function(HistoryBody_ClockSet) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? by = null,}) {
  return _then(HistoryBody_ClockSet(
by: null == by ? _self.by : by // ignore: cast_nullable_to_non_nullable
as FedBy,
  ));
}

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$FedByCopyWith<$Res> get by {
  
  return $FedByCopyWith<$Res>(_self.by, (value) {
    return _then(_self.copyWith(by: value));
  });
}
}

/// @nodoc


class HistoryBody_Lost extends HistoryBody {
  const HistoryBody_Lost({required this.count}): super._();
  

 final  int count;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_LostCopyWith<HistoryBody_Lost> get copyWith => _$HistoryBody_LostCopyWithImpl<HistoryBody_Lost>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Lost&&(identical(other.count, count) || other.count == count));
}


@override
int get hashCode {
    return Object.hash(runtimeType,count);
}

@override
String toString() {
    return 'HistoryBody.lost(count: $count)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_LostCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_LostCopyWith(HistoryBody_Lost value, $Res Function(HistoryBody_Lost) _then) = _$HistoryBody_LostCopyWithImpl;
@useResult
$Res call({
 int count
});




}
/// @nodoc
class _$HistoryBody_LostCopyWithImpl<$Res>
    implements $HistoryBody_LostCopyWith<$Res> {
  _$HistoryBody_LostCopyWithImpl(this._self, this._then);

  final HistoryBody_Lost _self;
  final $Res Function(HistoryBody_Lost) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? count = null,}) {
  return _then(HistoryBody_Lost(
count: null == count ? _self.count : count // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class HistoryBody_FeedingBlocked extends HistoryBody {
  const HistoryBody_FeedingBlocked({this.reason}): super._();
  

 final  Blocked? reason;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_FeedingBlockedCopyWith<HistoryBody_FeedingBlocked> get copyWith => _$HistoryBody_FeedingBlockedCopyWithImpl<HistoryBody_FeedingBlocked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_FeedingBlocked&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'HistoryBody.feedingBlocked(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_FeedingBlockedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_FeedingBlockedCopyWith(HistoryBody_FeedingBlocked value, $Res Function(HistoryBody_FeedingBlocked) _then) = _$HistoryBody_FeedingBlockedCopyWithImpl;
@useResult
$Res call({
 Blocked? reason
});




}
/// @nodoc
class _$HistoryBody_FeedingBlockedCopyWithImpl<$Res>
    implements $HistoryBody_FeedingBlockedCopyWith<$Res> {
  _$HistoryBody_FeedingBlockedCopyWithImpl(this._self, this._then);

  final HistoryBody_FeedingBlocked _self;
  final $Res Function(HistoryBody_FeedingBlocked) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = freezed,}) {
  return _then(HistoryBody_FeedingBlocked(
reason: freezed == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as Blocked?,
  ));
}


}

/// @nodoc


class HistoryBody_MealSoon extends HistoryBody {
  const HistoryBody_MealSoon({required this.minute, required this.portions}): super._();
  

 final  int minute;
 final  int portions;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_MealSoonCopyWith<HistoryBody_MealSoon> get copyWith => _$HistoryBody_MealSoonCopyWithImpl<HistoryBody_MealSoon>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_MealSoon&&(identical(other.minute, minute) || other.minute == minute)&&(identical(other.portions, portions) || other.portions == portions));
}


@override
int get hashCode {
    return Object.hash(runtimeType,minute,portions);
}

@override
String toString() {
    return 'HistoryBody.mealSoon(minute: $minute, portions: $portions)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_MealSoonCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_MealSoonCopyWith(HistoryBody_MealSoon value, $Res Function(HistoryBody_MealSoon) _then) = _$HistoryBody_MealSoonCopyWithImpl;
@useResult
$Res call({
 int minute, int portions
});




}
/// @nodoc
class _$HistoryBody_MealSoonCopyWithImpl<$Res>
    implements $HistoryBody_MealSoonCopyWith<$Res> {
  _$HistoryBody_MealSoonCopyWithImpl(this._self, this._then);

  final HistoryBody_MealSoon _self;
  final $Res Function(HistoryBody_MealSoon) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? minute = null,Object? portions = null,}) {
  return _then(HistoryBody_MealSoon(
minute: null == minute ? _self.minute : minute // ignore: cast_nullable_to_non_nullable
as int,portions: null == portions ? _self.portions : portions // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class HistoryBody_Supply extends HistoryBody {
  const HistoryBody_Supply({this.adapter}): super._();
  

 final  bool? adapter;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_SupplyCopyWith<HistoryBody_Supply> get copyWith => _$HistoryBody_SupplyCopyWithImpl<HistoryBody_Supply>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Supply&&(identical(other.adapter, adapter) || other.adapter == adapter));
}


@override
int get hashCode {
    return Object.hash(runtimeType,adapter);
}

@override
String toString() {
    return 'HistoryBody.supply(adapter: $adapter)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_SupplyCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_SupplyCopyWith(HistoryBody_Supply value, $Res Function(HistoryBody_Supply) _then) = _$HistoryBody_SupplyCopyWithImpl;
@useResult
$Res call({
 bool? adapter
});




}
/// @nodoc
class _$HistoryBody_SupplyCopyWithImpl<$Res>
    implements $HistoryBody_SupplyCopyWith<$Res> {
  _$HistoryBody_SupplyCopyWithImpl(this._self, this._then);

  final HistoryBody_Supply _self;
  final $Res Function(HistoryBody_Supply) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? adapter = freezed,}) {
  return _then(HistoryBody_Supply(
adapter: freezed == adapter ? _self.adapter : adapter // ignore: cast_nullable_to_non_nullable
as bool?,
  ));
}


}

/// @nodoc


class HistoryBody_DispenseRefused extends HistoryBody {
  const HistoryBody_DispenseRefused({required this.reason}): super._();
  

 final  Blocked reason;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_DispenseRefusedCopyWith<HistoryBody_DispenseRefused> get copyWith => _$HistoryBody_DispenseRefusedCopyWithImpl<HistoryBody_DispenseRefused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_DispenseRefused&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'HistoryBody.dispenseRefused(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_DispenseRefusedCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_DispenseRefusedCopyWith(HistoryBody_DispenseRefused value, $Res Function(HistoryBody_DispenseRefused) _then) = _$HistoryBody_DispenseRefusedCopyWithImpl;
@useResult
$Res call({
 Blocked reason
});




}
/// @nodoc
class _$HistoryBody_DispenseRefusedCopyWithImpl<$Res>
    implements $HistoryBody_DispenseRefusedCopyWith<$Res> {
  _$HistoryBody_DispenseRefusedCopyWithImpl(this._self, this._then);

  final HistoryBody_DispenseRefused _self;
  final $Res Function(HistoryBody_DispenseRefused) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(HistoryBody_DispenseRefused(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as Blocked,
  ));
}


}

/// @nodoc


class HistoryBody_Food extends HistoryBody {
  const HistoryBody_Food({required this.level}): super._();
  

 final  FoodLevel level;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_FoodCopyWith<HistoryBody_Food> get copyWith => _$HistoryBody_FoodCopyWithImpl<HistoryBody_Food>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Food&&(identical(other.level, level) || other.level == level));
}


@override
int get hashCode {
    return Object.hash(runtimeType,level);
}

@override
String toString() {
    return 'HistoryBody.food(level: $level)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_FoodCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_FoodCopyWith(HistoryBody_Food value, $Res Function(HistoryBody_Food) _then) = _$HistoryBody_FoodCopyWithImpl;
@useResult
$Res call({
 FoodLevel level
});




}
/// @nodoc
class _$HistoryBody_FoodCopyWithImpl<$Res>
    implements $HistoryBody_FoodCopyWith<$Res> {
  _$HistoryBody_FoodCopyWithImpl(this._self, this._then);

  final HistoryBody_Food _self;
  final $Res Function(HistoryBody_Food) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? level = null,}) {
  return _then(HistoryBody_Food(
level: null == level ? _self.level : level // ignore: cast_nullable_to_non_nullable
as FoodLevel,
  ));
}


}

/// @nodoc


class HistoryBody_Gap extends HistoryBody {
  const HistoryBody_Gap({required this.from, required this.to}): super._();
  

 final  BigInt from;
 final  BigInt to;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$HistoryBody_GapCopyWith<HistoryBody_Gap> get copyWith => _$HistoryBody_GapCopyWithImpl<HistoryBody_Gap>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Gap&&(identical(other.from, from) || other.from == from)&&(identical(other.to, to) || other.to == to));
}


@override
int get hashCode {
    return Object.hash(runtimeType,from,to);
}

@override
String toString() {
    return 'HistoryBody.gap(from: $from, to: $to)';
}


}

/// @nodoc
abstract mixin class $HistoryBody_GapCopyWith<$Res> implements $HistoryBodyCopyWith<$Res> {
  factory $HistoryBody_GapCopyWith(HistoryBody_Gap value, $Res Function(HistoryBody_Gap) _then) = _$HistoryBody_GapCopyWithImpl;
@useResult
$Res call({
 BigInt from, BigInt to
});




}
/// @nodoc
class _$HistoryBody_GapCopyWithImpl<$Res>
    implements $HistoryBody_GapCopyWith<$Res> {
  _$HistoryBody_GapCopyWithImpl(this._self, this._then);

  final HistoryBody_Gap _self;
  final $Res Function(HistoryBody_Gap) _then;

/// Create a copy of HistoryBody
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? from = null,Object? to = null,}) {
  return _then(HistoryBody_Gap(
from: null == from ? _self.from : from // ignore: cast_nullable_to_non_nullable
as BigInt,to: null == to ? _self.to : to // ignore: cast_nullable_to_non_nullable
as BigInt,
  ));
}


}

/// @nodoc


class HistoryBody_Other extends HistoryBody {
  const HistoryBody_Other(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is HistoryBody_Other);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'HistoryBody.other()';
}


}




/// @nodoc
mixin _$MealStanding {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealStanding);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MealStanding()';
}


}

/// @nodoc
class $MealStandingCopyWith<$Res>  {
$MealStandingCopyWith(MealStanding _, $Res Function(MealStanding) __);
}


/// Adds pattern-matching-related methods to [MealStanding].
extension MealStandingPatterns on MealStanding {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( MealStanding_Fed value)?  fed,TResult Function( MealStanding_Passed value)?  passed,TResult Function( MealStanding_Next value)?  next,TResult Function( MealStanding_Later value)?  later,required TResult orElse(),}){
final _that = this;
switch (_that) {
case MealStanding_Fed() when fed != null:
return fed(_that);case MealStanding_Passed() when passed != null:
return passed(_that);case MealStanding_Next() when next != null:
return next(_that);case MealStanding_Later() when later != null:
return later(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( MealStanding_Fed value)  fed,required TResult Function( MealStanding_Passed value)  passed,required TResult Function( MealStanding_Next value)  next,required TResult Function( MealStanding_Later value)  later,}){
final _that = this;
switch (_that) {
case MealStanding_Fed():
return fed(_that);case MealStanding_Passed():
return passed(_that);case MealStanding_Next():
return next(_that);case MealStanding_Later():
return later(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( MealStanding_Fed value)?  fed,TResult? Function( MealStanding_Passed value)?  passed,TResult? Function( MealStanding_Next value)?  next,TResult? Function( MealStanding_Later value)?  later,}){
final _that = this;
switch (_that) {
case MealStanding_Fed() when fed != null:
return fed(_that);case MealStanding_Passed() when passed != null:
return passed(_that);case MealStanding_Next() when next != null:
return next(_that);case MealStanding_Later() when later != null:
return later(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( FeedResult result)?  fed,TResult Function()?  passed,TResult Function( PlatformInt64 atMs)?  next,TResult Function()?  later,required TResult orElse(),}) {final _that = this;
switch (_that) {
case MealStanding_Fed() when fed != null:
return fed(_that.result);case MealStanding_Passed() when passed != null:
return passed();case MealStanding_Next() when next != null:
return next(_that.atMs);case MealStanding_Later() when later != null:
return later();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( FeedResult result)  fed,required TResult Function()  passed,required TResult Function( PlatformInt64 atMs)  next,required TResult Function()  later,}) {final _that = this;
switch (_that) {
case MealStanding_Fed():
return fed(_that.result);case MealStanding_Passed():
return passed();case MealStanding_Next():
return next(_that.atMs);case MealStanding_Later():
return later();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( FeedResult result)?  fed,TResult? Function()?  passed,TResult? Function( PlatformInt64 atMs)?  next,TResult? Function()?  later,}) {final _that = this;
switch (_that) {
case MealStanding_Fed() when fed != null:
return fed(_that.result);case MealStanding_Passed() when passed != null:
return passed();case MealStanding_Next() when next != null:
return next(_that.atMs);case MealStanding_Later() when later != null:
return later();case _:
  return null;

}
}

}

/// @nodoc


class MealStanding_Fed extends MealStanding {
  const MealStanding_Fed({required this.result}): super._();
  

 final  FeedResult result;

/// Create a copy of MealStanding
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MealStanding_FedCopyWith<MealStanding_Fed> get copyWith => _$MealStanding_FedCopyWithImpl<MealStanding_Fed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealStanding_Fed&&(identical(other.result, result) || other.result == result));
}


@override
int get hashCode {
    return Object.hash(runtimeType,result);
}

@override
String toString() {
    return 'MealStanding.fed(result: $result)';
}


}

/// @nodoc
abstract mixin class $MealStanding_FedCopyWith<$Res> implements $MealStandingCopyWith<$Res> {
  factory $MealStanding_FedCopyWith(MealStanding_Fed value, $Res Function(MealStanding_Fed) _then) = _$MealStanding_FedCopyWithImpl;
@useResult
$Res call({
 FeedResult result
});




}
/// @nodoc
class _$MealStanding_FedCopyWithImpl<$Res>
    implements $MealStanding_FedCopyWith<$Res> {
  _$MealStanding_FedCopyWithImpl(this._self, this._then);

  final MealStanding_Fed _self;
  final $Res Function(MealStanding_Fed) _then;

/// Create a copy of MealStanding
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? result = null,}) {
  return _then(MealStanding_Fed(
result: null == result ? _self.result : result // ignore: cast_nullable_to_non_nullable
as FeedResult,
  ));
}


}

/// @nodoc


class MealStanding_Passed extends MealStanding {
  const MealStanding_Passed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealStanding_Passed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MealStanding.passed()';
}


}




/// @nodoc


class MealStanding_Next extends MealStanding {
  const MealStanding_Next({required this.atMs}): super._();
  

 final  PlatformInt64 atMs;

/// Create a copy of MealStanding
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$MealStanding_NextCopyWith<MealStanding_Next> get copyWith => _$MealStanding_NextCopyWithImpl<MealStanding_Next>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealStanding_Next&&(identical(other.atMs, atMs) || other.atMs == atMs));
}


@override
int get hashCode {
    return Object.hash(runtimeType,atMs);
}

@override
String toString() {
    return 'MealStanding.next(atMs: $atMs)';
}


}

/// @nodoc
abstract mixin class $MealStanding_NextCopyWith<$Res> implements $MealStandingCopyWith<$Res> {
  factory $MealStanding_NextCopyWith(MealStanding_Next value, $Res Function(MealStanding_Next) _then) = _$MealStanding_NextCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 atMs
});




}
/// @nodoc
class _$MealStanding_NextCopyWithImpl<$Res>
    implements $MealStanding_NextCopyWith<$Res> {
  _$MealStanding_NextCopyWithImpl(this._self, this._then);

  final MealStanding_Next _self;
  final $Res Function(MealStanding_Next) _then;

/// Create a copy of MealStanding
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? atMs = null,}) {
  return _then(MealStanding_Next(
atMs: null == atMs ? _self.atMs : atMs // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class MealStanding_Later extends MealStanding {
  const MealStanding_Later(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is MealStanding_Later);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'MealStanding.later()';
}


}




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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( SessionUpdate_Opened value)?  opened,TResult Function( SessionUpdate_Status value)?  status,TResult Function( SessionUpdate_FeedStarted value)?  feedStarted,TResult Function( SessionUpdate_FeedRefused value)?  feedRefused,TResult Function( SessionUpdate_FeedUnanswered value)?  feedUnanswered,TResult Function( SessionUpdate_FeedEnded value)?  feedEnded,TResult Function( SessionUpdate_CancelUnconfirmed value)?  cancelUnconfirmed,TResult Function( SessionUpdate_ClockSynced value)?  clockSynced,TResult Function( SessionUpdate_ClockNotSynced value)?  clockNotSynced,TResult Function( SessionUpdate_History value)?  history,TResult Function( SessionUpdate_Log value)?  log,TResult Function( SessionUpdate_Closed value)?  closed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that);case SessionUpdate_Status() when status != null:
return status(_that);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered(_that);case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that);case SessionUpdate_ClockSynced() when clockSynced != null:
return clockSynced(_that);case SessionUpdate_ClockNotSynced() when clockNotSynced != null:
return clockNotSynced(_that);case SessionUpdate_History() when history != null:
return history(_that);case SessionUpdate_Log() when log != null:
return log(_that);case SessionUpdate_Closed() when closed != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( SessionUpdate_Opened value)  opened,required TResult Function( SessionUpdate_Status value)  status,required TResult Function( SessionUpdate_FeedStarted value)  feedStarted,required TResult Function( SessionUpdate_FeedRefused value)  feedRefused,required TResult Function( SessionUpdate_FeedUnanswered value)  feedUnanswered,required TResult Function( SessionUpdate_FeedEnded value)  feedEnded,required TResult Function( SessionUpdate_CancelUnconfirmed value)  cancelUnconfirmed,required TResult Function( SessionUpdate_ClockSynced value)  clockSynced,required TResult Function( SessionUpdate_ClockNotSynced value)  clockNotSynced,required TResult Function( SessionUpdate_History value)  history,required TResult Function( SessionUpdate_Log value)  log,required TResult Function( SessionUpdate_Closed value)  closed,}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened():
return opened(_that);case SessionUpdate_Status():
return status(_that);case SessionUpdate_FeedStarted():
return feedStarted(_that);case SessionUpdate_FeedRefused():
return feedRefused(_that);case SessionUpdate_FeedUnanswered():
return feedUnanswered(_that);case SessionUpdate_FeedEnded():
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed():
return cancelUnconfirmed(_that);case SessionUpdate_ClockSynced():
return clockSynced(_that);case SessionUpdate_ClockNotSynced():
return clockNotSynced(_that);case SessionUpdate_History():
return history(_that);case SessionUpdate_Log():
return log(_that);case SessionUpdate_Closed():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( SessionUpdate_Opened value)?  opened,TResult? Function( SessionUpdate_Status value)?  status,TResult? Function( SessionUpdate_FeedStarted value)?  feedStarted,TResult? Function( SessionUpdate_FeedRefused value)?  feedRefused,TResult? Function( SessionUpdate_FeedUnanswered value)?  feedUnanswered,TResult? Function( SessionUpdate_FeedEnded value)?  feedEnded,TResult? Function( SessionUpdate_CancelUnconfirmed value)?  cancelUnconfirmed,TResult? Function( SessionUpdate_ClockSynced value)?  clockSynced,TResult? Function( SessionUpdate_ClockNotSynced value)?  clockNotSynced,TResult? Function( SessionUpdate_History value)?  history,TResult? Function( SessionUpdate_Log value)?  log,TResult? Function( SessionUpdate_Closed value)?  closed,}){
final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that);case SessionUpdate_Status() when status != null:
return status(_that);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered(_that);case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that);case SessionUpdate_ClockSynced() when clockSynced != null:
return clockSynced(_that);case SessionUpdate_ClockNotSynced() when clockNotSynced != null:
return clockNotSynced(_that);case SessionUpdate_History() when history != null:
return history(_that);case SessionUpdate_Log() when log != null:
return log(_that);case SessionUpdate_Closed() when closed != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String build,  ClientRole role)?  opened,TResult Function( Card card)?  status,TResult Function( int feed,  int portions)?  feedStarted,TResult Function( FeedRefusal refusal)?  feedRefused,TResult Function()?  feedUnanswered,TResult Function( FeedResult result)?  feedEnded,TResult Function( FeedRefusal? refusal)?  cancelUnconfirmed,TResult Function()?  clockSynced,TResult Function()?  clockNotSynced,TResult Function( List<HistoryEntry> entries,  bool caughtUp)?  history,TResult Function( int seq,  int dropped,  List<String> lines)?  log,TResult Function( BridgeError? error)?  closed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that.build,_that.role);case SessionUpdate_Status() when status != null:
return status(_that.card);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered();case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that.refusal);case SessionUpdate_ClockSynced() when clockSynced != null:
return clockSynced();case SessionUpdate_ClockNotSynced() when clockNotSynced != null:
return clockNotSynced();case SessionUpdate_History() when history != null:
return history(_that.entries,_that.caughtUp);case SessionUpdate_Log() when log != null:
return log(_that.seq,_that.dropped,_that.lines);case SessionUpdate_Closed() when closed != null:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String build,  ClientRole role)  opened,required TResult Function( Card card)  status,required TResult Function( int feed,  int portions)  feedStarted,required TResult Function( FeedRefusal refusal)  feedRefused,required TResult Function()  feedUnanswered,required TResult Function( FeedResult result)  feedEnded,required TResult Function( FeedRefusal? refusal)  cancelUnconfirmed,required TResult Function()  clockSynced,required TResult Function()  clockNotSynced,required TResult Function( List<HistoryEntry> entries,  bool caughtUp)  history,required TResult Function( int seq,  int dropped,  List<String> lines)  log,required TResult Function( BridgeError? error)  closed,}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened():
return opened(_that.build,_that.role);case SessionUpdate_Status():
return status(_that.card);case SessionUpdate_FeedStarted():
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused():
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered():
return feedUnanswered();case SessionUpdate_FeedEnded():
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed():
return cancelUnconfirmed(_that.refusal);case SessionUpdate_ClockSynced():
return clockSynced();case SessionUpdate_ClockNotSynced():
return clockNotSynced();case SessionUpdate_History():
return history(_that.entries,_that.caughtUp);case SessionUpdate_Log():
return log(_that.seq,_that.dropped,_that.lines);case SessionUpdate_Closed():
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String build,  ClientRole role)?  opened,TResult? Function( Card card)?  status,TResult? Function( int feed,  int portions)?  feedStarted,TResult? Function( FeedRefusal refusal)?  feedRefused,TResult? Function()?  feedUnanswered,TResult? Function( FeedResult result)?  feedEnded,TResult? Function( FeedRefusal? refusal)?  cancelUnconfirmed,TResult? Function()?  clockSynced,TResult? Function()?  clockNotSynced,TResult? Function( List<HistoryEntry> entries,  bool caughtUp)?  history,TResult? Function( int seq,  int dropped,  List<String> lines)?  log,TResult? Function( BridgeError? error)?  closed,}) {final _that = this;
switch (_that) {
case SessionUpdate_Opened() when opened != null:
return opened(_that.build,_that.role);case SessionUpdate_Status() when status != null:
return status(_that.card);case SessionUpdate_FeedStarted() when feedStarted != null:
return feedStarted(_that.feed,_that.portions);case SessionUpdate_FeedRefused() when feedRefused != null:
return feedRefused(_that.refusal);case SessionUpdate_FeedUnanswered() when feedUnanswered != null:
return feedUnanswered();case SessionUpdate_FeedEnded() when feedEnded != null:
return feedEnded(_that.result);case SessionUpdate_CancelUnconfirmed() when cancelUnconfirmed != null:
return cancelUnconfirmed(_that.refusal);case SessionUpdate_ClockSynced() when clockSynced != null:
return clockSynced();case SessionUpdate_ClockNotSynced() when clockNotSynced != null:
return clockNotSynced();case SessionUpdate_History() when history != null:
return history(_that.entries,_that.caughtUp);case SessionUpdate_Log() when log != null:
return log(_that.seq,_that.dropped,_that.lines);case SessionUpdate_Closed() when closed != null:
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


class SessionUpdate_ClockSynced extends SessionUpdate {
  const SessionUpdate_ClockSynced(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_ClockSynced);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'SessionUpdate.clockSynced()';
}


}




/// @nodoc


class SessionUpdate_ClockNotSynced extends SessionUpdate {
  const SessionUpdate_ClockNotSynced(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_ClockNotSynced);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'SessionUpdate.clockNotSynced()';
}


}




/// @nodoc


class SessionUpdate_History extends SessionUpdate {
  const SessionUpdate_History({required  List<HistoryEntry> entries, required this.caughtUp}): _entries = entries,super._();
  

 final  List<HistoryEntry> _entries;
 List<HistoryEntry> get entries {
  if (_entries is EqualUnmodifiableListView) return _entries;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_entries);
}

 final  bool caughtUp;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_HistoryCopyWith<SessionUpdate_History> get copyWith => _$SessionUpdate_HistoryCopyWithImpl<SessionUpdate_History>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_History&&const DeepCollectionEquality().equals(other.entries, _entries)&&(identical(other.caughtUp, caughtUp) || other.caughtUp == caughtUp));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_entries),caughtUp);
}

@override
String toString() {
    return 'SessionUpdate.history(entries: $entries, caughtUp: $caughtUp)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_HistoryCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_HistoryCopyWith(SessionUpdate_History value, $Res Function(SessionUpdate_History) _then) = _$SessionUpdate_HistoryCopyWithImpl;
@useResult
$Res call({
 List<HistoryEntry> entries, bool caughtUp
});




}
/// @nodoc
class _$SessionUpdate_HistoryCopyWithImpl<$Res>
    implements $SessionUpdate_HistoryCopyWith<$Res> {
  _$SessionUpdate_HistoryCopyWithImpl(this._self, this._then);

  final SessionUpdate_History _self;
  final $Res Function(SessionUpdate_History) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? entries = null,Object? caughtUp = null,}) {
  return _then(SessionUpdate_History(
entries: null == entries ? _self._entries : entries // ignore: cast_nullable_to_non_nullable
as List<HistoryEntry>,caughtUp: null == caughtUp ? _self.caughtUp : caughtUp // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class SessionUpdate_Log extends SessionUpdate {
  const SessionUpdate_Log({required this.seq, required this.dropped, required  List<String> lines}): _lines = lines,super._();
  

 final  int seq;
 final  int dropped;
 final  List<String> _lines;
 List<String> get lines {
  if (_lines is EqualUnmodifiableListView) return _lines;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_lines);
}


/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$SessionUpdate_LogCopyWith<SessionUpdate_Log> get copyWith => _$SessionUpdate_LogCopyWithImpl<SessionUpdate_Log>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is SessionUpdate_Log&&(identical(other.seq, seq) || other.seq == seq)&&(identical(other.dropped, dropped) || other.dropped == dropped)&&const DeepCollectionEquality().equals(other.lines, _lines));
}


@override
int get hashCode {
    return Object.hash(runtimeType,seq,dropped,const DeepCollectionEquality().hash(_lines));
}

@override
String toString() {
    return 'SessionUpdate.log(seq: $seq, dropped: $dropped, lines: $lines)';
}


}

/// @nodoc
abstract mixin class $SessionUpdate_LogCopyWith<$Res> implements $SessionUpdateCopyWith<$Res> {
  factory $SessionUpdate_LogCopyWith(SessionUpdate_Log value, $Res Function(SessionUpdate_Log) _then) = _$SessionUpdate_LogCopyWithImpl;
@useResult
$Res call({
 int seq, int dropped, List<String> lines
});




}
/// @nodoc
class _$SessionUpdate_LogCopyWithImpl<$Res>
    implements $SessionUpdate_LogCopyWith<$Res> {
  _$SessionUpdate_LogCopyWithImpl(this._self, this._then);

  final SessionUpdate_Log _self;
  final $Res Function(SessionUpdate_Log) _then;

/// Create a copy of SessionUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? seq = null,Object? dropped = null,Object? lines = null,}) {
  return _then(SessionUpdate_Log(
seq: null == seq ? _self.seq : seq // ignore: cast_nullable_to_non_nullable
as int,dropped: null == dropped ? _self.dropped : dropped // ignore: cast_nullable_to_non_nullable
as int,lines: null == lines ? _self._lines : lines // ignore: cast_nullable_to_non_nullable
as List<String>,
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
