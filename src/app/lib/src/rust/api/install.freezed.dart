// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'install.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$BuildCheck {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BuildCheck);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'BuildCheck()';
}


}

/// @nodoc
class $BuildCheckCopyWith<$Res>  {
$BuildCheckCopyWith(BuildCheck _, $Res Function(BuildCheck) __);
}


/// Adds pattern-matching-related methods to [BuildCheck].
extension BuildCheckPatterns on BuildCheck {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( BuildCheck_Reviewable value)?  reviewable,TResult Function( BuildCheck_Unreviewed value)?  unreviewed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case BuildCheck_Reviewable() when reviewable != null:
return reviewable(_that);case BuildCheck_Unreviewed() when unreviewed != null:
return unreviewed(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( BuildCheck_Reviewable value)  reviewable,required TResult Function( BuildCheck_Unreviewed value)  unreviewed,}){
final _that = this;
switch (_that) {
case BuildCheck_Reviewable():
return reviewable(_that);case BuildCheck_Unreviewed():
return unreviewed(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( BuildCheck_Reviewable value)?  reviewable,TResult? Function( BuildCheck_Unreviewed value)?  unreviewed,}){
final _that = this;
switch (_that) {
case BuildCheck_Reviewable() when reviewable != null:
return reviewable(_that);case BuildCheck_Unreviewed() when unreviewed != null:
return unreviewed(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  reviewable,TResult Function( String commit)?  unreviewed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case BuildCheck_Reviewable() when reviewable != null:
return reviewable();case BuildCheck_Unreviewed() when unreviewed != null:
return unreviewed(_that.commit);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  reviewable,required TResult Function( String commit)  unreviewed,}) {final _that = this;
switch (_that) {
case BuildCheck_Reviewable():
return reviewable();case BuildCheck_Unreviewed():
return unreviewed(_that.commit);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  reviewable,TResult? Function( String commit)?  unreviewed,}) {final _that = this;
switch (_that) {
case BuildCheck_Reviewable() when reviewable != null:
return reviewable();case BuildCheck_Unreviewed() when unreviewed != null:
return unreviewed(_that.commit);case _:
  return null;

}
}

}

/// @nodoc


class BuildCheck_Reviewable extends BuildCheck {
  const BuildCheck_Reviewable(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BuildCheck_Reviewable);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'BuildCheck.reviewable()';
}


}




/// @nodoc


class BuildCheck_Unreviewed extends BuildCheck {
  const BuildCheck_Unreviewed({required this.commit}): super._();
  

 final  String commit;

/// Create a copy of BuildCheck
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$BuildCheck_UnreviewedCopyWith<BuildCheck_Unreviewed> get copyWith => _$BuildCheck_UnreviewedCopyWithImpl<BuildCheck_Unreviewed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is BuildCheck_Unreviewed&&(identical(other.commit, commit) || other.commit == commit));
}


@override
int get hashCode {
    return Object.hash(runtimeType,commit);
}

@override
String toString() {
    return 'BuildCheck.unreviewed(commit: $commit)';
}


}

/// @nodoc
abstract mixin class $BuildCheck_UnreviewedCopyWith<$Res> implements $BuildCheckCopyWith<$Res> {
  factory $BuildCheck_UnreviewedCopyWith(BuildCheck_Unreviewed value, $Res Function(BuildCheck_Unreviewed) _then) = _$BuildCheck_UnreviewedCopyWithImpl;
@useResult
$Res call({
 String commit
});




}
/// @nodoc
class _$BuildCheck_UnreviewedCopyWithImpl<$Res>
    implements $BuildCheck_UnreviewedCopyWith<$Res> {
  _$BuildCheck_UnreviewedCopyWithImpl(this._self, this._then);

  final BuildCheck_Unreviewed _self;
  final $Res Function(BuildCheck_Unreviewed) _then;

/// Create a copy of BuildCheck
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? commit = null,}) {
  return _then(BuildCheck_Unreviewed(
commit: null == commit ? _self.commit : commit // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$DnsResult {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'DnsResult()';
}


}

/// @nodoc
class $DnsResultCopyWith<$Res>  {
$DnsResultCopyWith(DnsResult _, $Res Function(DnsResult) __);
}


/// Adds pattern-matching-related methods to [DnsResult].
extension DnsResultPatterns on DnsResult {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( DnsResult_Points value)?  points,TResult Function( DnsResult_Mixed value)?  mixed,TResult Function( DnsResult_Elsewhere value)?  elsewhere,TResult Function( DnsResult_Unresolved value)?  unresolved,TResult Function( DnsResult_BadName value)?  badName,required TResult orElse(),}){
final _that = this;
switch (_that) {
case DnsResult_Points() when points != null:
return points(_that);case DnsResult_Mixed() when mixed != null:
return mixed(_that);case DnsResult_Elsewhere() when elsewhere != null:
return elsewhere(_that);case DnsResult_Unresolved() when unresolved != null:
return unresolved(_that);case DnsResult_BadName() when badName != null:
return badName(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( DnsResult_Points value)  points,required TResult Function( DnsResult_Mixed value)  mixed,required TResult Function( DnsResult_Elsewhere value)  elsewhere,required TResult Function( DnsResult_Unresolved value)  unresolved,required TResult Function( DnsResult_BadName value)  badName,}){
final _that = this;
switch (_that) {
case DnsResult_Points():
return points(_that);case DnsResult_Mixed():
return mixed(_that);case DnsResult_Elsewhere():
return elsewhere(_that);case DnsResult_Unresolved():
return unresolved(_that);case DnsResult_BadName():
return badName(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( DnsResult_Points value)?  points,TResult? Function( DnsResult_Mixed value)?  mixed,TResult? Function( DnsResult_Elsewhere value)?  elsewhere,TResult? Function( DnsResult_Unresolved value)?  unresolved,TResult? Function( DnsResult_BadName value)?  badName,}){
final _that = this;
switch (_that) {
case DnsResult_Points() when points != null:
return points(_that);case DnsResult_Mixed() when mixed != null:
return mixed(_that);case DnsResult_Elsewhere() when elsewhere != null:
return elsewhere(_that);case DnsResult_Unresolved() when unresolved != null:
return unresolved(_that);case DnsResult_BadName() when badName != null:
return badName(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  points,TResult Function( List<String> others)?  mixed,TResult Function( List<String> others)?  elsewhere,TResult Function()?  unresolved,TResult Function()?  badName,required TResult orElse(),}) {final _that = this;
switch (_that) {
case DnsResult_Points() when points != null:
return points();case DnsResult_Mixed() when mixed != null:
return mixed(_that.others);case DnsResult_Elsewhere() when elsewhere != null:
return elsewhere(_that.others);case DnsResult_Unresolved() when unresolved != null:
return unresolved();case DnsResult_BadName() when badName != null:
return badName();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  points,required TResult Function( List<String> others)  mixed,required TResult Function( List<String> others)  elsewhere,required TResult Function()  unresolved,required TResult Function()  badName,}) {final _that = this;
switch (_that) {
case DnsResult_Points():
return points();case DnsResult_Mixed():
return mixed(_that.others);case DnsResult_Elsewhere():
return elsewhere(_that.others);case DnsResult_Unresolved():
return unresolved();case DnsResult_BadName():
return badName();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  points,TResult? Function( List<String> others)?  mixed,TResult? Function( List<String> others)?  elsewhere,TResult? Function()?  unresolved,TResult? Function()?  badName,}) {final _that = this;
switch (_that) {
case DnsResult_Points() when points != null:
return points();case DnsResult_Mixed() when mixed != null:
return mixed(_that.others);case DnsResult_Elsewhere() when elsewhere != null:
return elsewhere(_that.others);case DnsResult_Unresolved() when unresolved != null:
return unresolved();case DnsResult_BadName() when badName != null:
return badName();case _:
  return null;

}
}

}

/// @nodoc


class DnsResult_Points extends DnsResult {
  const DnsResult_Points(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult_Points);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'DnsResult.points()';
}


}




/// @nodoc


class DnsResult_Mixed extends DnsResult {
  const DnsResult_Mixed({required  List<String> others}): _others = others,super._();
  

 final  List<String> _others;
 List<String> get others {
  if (_others is EqualUnmodifiableListView) return _others;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_others);
}


/// Create a copy of DnsResult
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$DnsResult_MixedCopyWith<DnsResult_Mixed> get copyWith => _$DnsResult_MixedCopyWithImpl<DnsResult_Mixed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult_Mixed&&const DeepCollectionEquality().equals(other.others, _others));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_others));
}

@override
String toString() {
    return 'DnsResult.mixed(others: $others)';
}


}

/// @nodoc
abstract mixin class $DnsResult_MixedCopyWith<$Res> implements $DnsResultCopyWith<$Res> {
  factory $DnsResult_MixedCopyWith(DnsResult_Mixed value, $Res Function(DnsResult_Mixed) _then) = _$DnsResult_MixedCopyWithImpl;
@useResult
$Res call({
 List<String> others
});




}
/// @nodoc
class _$DnsResult_MixedCopyWithImpl<$Res>
    implements $DnsResult_MixedCopyWith<$Res> {
  _$DnsResult_MixedCopyWithImpl(this._self, this._then);

  final DnsResult_Mixed _self;
  final $Res Function(DnsResult_Mixed) _then;

/// Create a copy of DnsResult
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? others = null,}) {
  return _then(DnsResult_Mixed(
others: null == others ? _self._others : others // ignore: cast_nullable_to_non_nullable
as List<String>,
  ));
}


}

/// @nodoc


class DnsResult_Elsewhere extends DnsResult {
  const DnsResult_Elsewhere({required  List<String> others}): _others = others,super._();
  

 final  List<String> _others;
 List<String> get others {
  if (_others is EqualUnmodifiableListView) return _others;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_others);
}


/// Create a copy of DnsResult
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$DnsResult_ElsewhereCopyWith<DnsResult_Elsewhere> get copyWith => _$DnsResult_ElsewhereCopyWithImpl<DnsResult_Elsewhere>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult_Elsewhere&&const DeepCollectionEquality().equals(other.others, _others));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_others));
}

@override
String toString() {
    return 'DnsResult.elsewhere(others: $others)';
}


}

/// @nodoc
abstract mixin class $DnsResult_ElsewhereCopyWith<$Res> implements $DnsResultCopyWith<$Res> {
  factory $DnsResult_ElsewhereCopyWith(DnsResult_Elsewhere value, $Res Function(DnsResult_Elsewhere) _then) = _$DnsResult_ElsewhereCopyWithImpl;
@useResult
$Res call({
 List<String> others
});




}
/// @nodoc
class _$DnsResult_ElsewhereCopyWithImpl<$Res>
    implements $DnsResult_ElsewhereCopyWith<$Res> {
  _$DnsResult_ElsewhereCopyWithImpl(this._self, this._then);

  final DnsResult_Elsewhere _self;
  final $Res Function(DnsResult_Elsewhere) _then;

/// Create a copy of DnsResult
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? others = null,}) {
  return _then(DnsResult_Elsewhere(
others: null == others ? _self._others : others // ignore: cast_nullable_to_non_nullable
as List<String>,
  ));
}


}

/// @nodoc


class DnsResult_Unresolved extends DnsResult {
  const DnsResult_Unresolved(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult_Unresolved);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'DnsResult.unresolved()';
}


}




/// @nodoc


class DnsResult_BadName extends DnsResult {
  const DnsResult_BadName(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is DnsResult_BadName);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'DnsResult.badName()';
}


}




/// @nodoc
mixin _$ImageCheck {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ImageCheck);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ImageCheck()';
}


}

/// @nodoc
class $ImageCheckCopyWith<$Res>  {
$ImageCheckCopyWith(ImageCheck _, $Res Function(ImageCheck) __);
}


/// Adds pattern-matching-related methods to [ImageCheck].
extension ImageCheckPatterns on ImageCheck {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ImageCheck_Ready value)?  ready,TResult Function( ImageCheck_Absent value)?  absent,TResult Function( ImageCheck_NotPinned value)?  notPinned,TResult Function( ImageCheck_Mismatch value)?  mismatch,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ImageCheck_Ready() when ready != null:
return ready(_that);case ImageCheck_Absent() when absent != null:
return absent(_that);case ImageCheck_NotPinned() when notPinned != null:
return notPinned(_that);case ImageCheck_Mismatch() when mismatch != null:
return mismatch(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ImageCheck_Ready value)  ready,required TResult Function( ImageCheck_Absent value)  absent,required TResult Function( ImageCheck_NotPinned value)  notPinned,required TResult Function( ImageCheck_Mismatch value)  mismatch,}){
final _that = this;
switch (_that) {
case ImageCheck_Ready():
return ready(_that);case ImageCheck_Absent():
return absent(_that);case ImageCheck_NotPinned():
return notPinned(_that);case ImageCheck_Mismatch():
return mismatch(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ImageCheck_Ready value)?  ready,TResult? Function( ImageCheck_Absent value)?  absent,TResult? Function( ImageCheck_NotPinned value)?  notPinned,TResult? Function( ImageCheck_Mismatch value)?  mismatch,}){
final _that = this;
switch (_that) {
case ImageCheck_Ready() when ready != null:
return ready(_that);case ImageCheck_Absent() when absent != null:
return absent(_that);case ImageCheck_NotPinned() when notPinned != null:
return notPinned(_that);case ImageCheck_Mismatch() when mismatch != null:
return mismatch(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( ImageProvisioning provisioning,  String build,  int version)?  ready,TResult Function()?  absent,TResult Function()?  notPinned,TResult Function( String detail)?  mismatch,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ImageCheck_Ready() when ready != null:
return ready(_that.provisioning,_that.build,_that.version);case ImageCheck_Absent() when absent != null:
return absent();case ImageCheck_NotPinned() when notPinned != null:
return notPinned();case ImageCheck_Mismatch() when mismatch != null:
return mismatch(_that.detail);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( ImageProvisioning provisioning,  String build,  int version)  ready,required TResult Function()  absent,required TResult Function()  notPinned,required TResult Function( String detail)  mismatch,}) {final _that = this;
switch (_that) {
case ImageCheck_Ready():
return ready(_that.provisioning,_that.build,_that.version);case ImageCheck_Absent():
return absent();case ImageCheck_NotPinned():
return notPinned();case ImageCheck_Mismatch():
return mismatch(_that.detail);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( ImageProvisioning provisioning,  String build,  int version)?  ready,TResult? Function()?  absent,TResult? Function()?  notPinned,TResult? Function( String detail)?  mismatch,}) {final _that = this;
switch (_that) {
case ImageCheck_Ready() when ready != null:
return ready(_that.provisioning,_that.build,_that.version);case ImageCheck_Absent() when absent != null:
return absent();case ImageCheck_NotPinned() when notPinned != null:
return notPinned();case ImageCheck_Mismatch() when mismatch != null:
return mismatch(_that.detail);case _:
  return null;

}
}

}

/// @nodoc


class ImageCheck_Ready extends ImageCheck {
  const ImageCheck_Ready({required this.provisioning, required this.build, required this.version}): super._();
  

 final  ImageProvisioning provisioning;
/// The startup identity, 64 hex.
 final  String build;
 final  int version;

/// Create a copy of ImageCheck
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ImageCheck_ReadyCopyWith<ImageCheck_Ready> get copyWith => _$ImageCheck_ReadyCopyWithImpl<ImageCheck_Ready>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ImageCheck_Ready&&(identical(other.provisioning, provisioning) || other.provisioning == provisioning)&&(identical(other.build, build) || other.build == build)&&(identical(other.version, version) || other.version == version));
}


@override
int get hashCode {
    return Object.hash(runtimeType,provisioning,build,version);
}

@override
String toString() {
    return 'ImageCheck.ready(provisioning: $provisioning, build: $build, version: $version)';
}


}

/// @nodoc
abstract mixin class $ImageCheck_ReadyCopyWith<$Res> implements $ImageCheckCopyWith<$Res> {
  factory $ImageCheck_ReadyCopyWith(ImageCheck_Ready value, $Res Function(ImageCheck_Ready) _then) = _$ImageCheck_ReadyCopyWithImpl;
@useResult
$Res call({
 ImageProvisioning provisioning, String build, int version
});




}
/// @nodoc
class _$ImageCheck_ReadyCopyWithImpl<$Res>
    implements $ImageCheck_ReadyCopyWith<$Res> {
  _$ImageCheck_ReadyCopyWithImpl(this._self, this._then);

  final ImageCheck_Ready _self;
  final $Res Function(ImageCheck_Ready) _then;

/// Create a copy of ImageCheck
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? provisioning = null,Object? build = null,Object? version = null,}) {
  return _then(ImageCheck_Ready(
provisioning: null == provisioning ? _self.provisioning : provisioning // ignore: cast_nullable_to_non_nullable
as ImageProvisioning,build: null == build ? _self.build : build // ignore: cast_nullable_to_non_nullable
as String,version: null == version ? _self.version : version // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class ImageCheck_Absent extends ImageCheck {
  const ImageCheck_Absent(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ImageCheck_Absent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ImageCheck.absent()';
}


}




/// @nodoc


class ImageCheck_NotPinned extends ImageCheck {
  const ImageCheck_NotPinned(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ImageCheck_NotPinned);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ImageCheck.notPinned()';
}


}




/// @nodoc


class ImageCheck_Mismatch extends ImageCheck {
  const ImageCheck_Mismatch({required this.detail}): super._();
  

 final  String detail;

/// Create a copy of ImageCheck
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ImageCheck_MismatchCopyWith<ImageCheck_Mismatch> get copyWith => _$ImageCheck_MismatchCopyWithImpl<ImageCheck_Mismatch>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ImageCheck_Mismatch&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,detail);
}

@override
String toString() {
    return 'ImageCheck.mismatch(detail: $detail)';
}


}

/// @nodoc
abstract mixin class $ImageCheck_MismatchCopyWith<$Res> implements $ImageCheckCopyWith<$Res> {
  factory $ImageCheck_MismatchCopyWith(ImageCheck_Mismatch value, $Res Function(ImageCheck_Mismatch) _then) = _$ImageCheck_MismatchCopyWithImpl;
@useResult
$Res call({
 String detail
});




}
/// @nodoc
class _$ImageCheck_MismatchCopyWithImpl<$Res>
    implements $ImageCheck_MismatchCopyWith<$Res> {
  _$ImageCheck_MismatchCopyWithImpl(this._self, this._then);

  final ImageCheck_Mismatch _self;
  final $Res Function(ImageCheck_Mismatch) _then;

/// Create a copy of ImageCheck
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? detail = null,}) {
  return _then(ImageCheck_Mismatch(
detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$InstallAnswer {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAnswer);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAnswer()';
}


}

/// @nodoc
class $InstallAnswerCopyWith<$Res>  {
$InstallAnswerCopyWith(InstallAnswer _, $Res Function(InstallAnswer) __);
}


/// Adds pattern-matching-related methods to [InstallAnswer].
extension InstallAnswerPatterns on InstallAnswer {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallAnswer_Done value)?  done,TResult Function( InstallAnswer_Yes value)?  yes,TResult Function( InstallAnswer_Located value)?  located,TResult Function( InstallAnswer_Failed value)?  failed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallAnswer_Done() when done != null:
return done(_that);case InstallAnswer_Yes() when yes != null:
return yes(_that);case InstallAnswer_Located() when located != null:
return located(_that);case InstallAnswer_Failed() when failed != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallAnswer_Done value)  done,required TResult Function( InstallAnswer_Yes value)  yes,required TResult Function( InstallAnswer_Located value)  located,required TResult Function( InstallAnswer_Failed value)  failed,}){
final _that = this;
switch (_that) {
case InstallAnswer_Done():
return done(_that);case InstallAnswer_Yes():
return yes(_that);case InstallAnswer_Located():
return located(_that);case InstallAnswer_Failed():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallAnswer_Done value)?  done,TResult? Function( InstallAnswer_Yes value)?  yes,TResult? Function( InstallAnswer_Located value)?  located,TResult? Function( InstallAnswer_Failed value)?  failed,}){
final _that = this;
switch (_that) {
case InstallAnswer_Done() when done != null:
return done(_that);case InstallAnswer_Yes() when yes != null:
return yes(_that);case InstallAnswer_Located() when located != null:
return located(_that);case InstallAnswer_Failed() when failed != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  done,TResult Function( bool yes)?  yes,TResult Function( String? address)?  located,TResult Function( String reason)?  failed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallAnswer_Done() when done != null:
return done();case InstallAnswer_Yes() when yes != null:
return yes(_that.yes);case InstallAnswer_Located() when located != null:
return located(_that.address);case InstallAnswer_Failed() when failed != null:
return failed(_that.reason);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  done,required TResult Function( bool yes)  yes,required TResult Function( String? address)  located,required TResult Function( String reason)  failed,}) {final _that = this;
switch (_that) {
case InstallAnswer_Done():
return done();case InstallAnswer_Yes():
return yes(_that.yes);case InstallAnswer_Located():
return located(_that.address);case InstallAnswer_Failed():
return failed(_that.reason);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  done,TResult? Function( bool yes)?  yes,TResult? Function( String? address)?  located,TResult? Function( String reason)?  failed,}) {final _that = this;
switch (_that) {
case InstallAnswer_Done() when done != null:
return done();case InstallAnswer_Yes() when yes != null:
return yes(_that.yes);case InstallAnswer_Located() when located != null:
return located(_that.address);case InstallAnswer_Failed() when failed != null:
return failed(_that.reason);case _:
  return null;

}
}

}

/// @nodoc


class InstallAnswer_Done extends InstallAnswer {
  const InstallAnswer_Done(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAnswer_Done);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAnswer.done()';
}


}




/// @nodoc


class InstallAnswer_Yes extends InstallAnswer {
  const InstallAnswer_Yes({required this.yes}): super._();
  

 final  bool yes;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAnswer_YesCopyWith<InstallAnswer_Yes> get copyWith => _$InstallAnswer_YesCopyWithImpl<InstallAnswer_Yes>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAnswer_Yes&&(identical(other.yes, yes) || other.yes == yes));
}


@override
int get hashCode {
    return Object.hash(runtimeType,yes);
}

@override
String toString() {
    return 'InstallAnswer.yes(yes: $yes)';
}


}

/// @nodoc
abstract mixin class $InstallAnswer_YesCopyWith<$Res> implements $InstallAnswerCopyWith<$Res> {
  factory $InstallAnswer_YesCopyWith(InstallAnswer_Yes value, $Res Function(InstallAnswer_Yes) _then) = _$InstallAnswer_YesCopyWithImpl;
@useResult
$Res call({
 bool yes
});




}
/// @nodoc
class _$InstallAnswer_YesCopyWithImpl<$Res>
    implements $InstallAnswer_YesCopyWith<$Res> {
  _$InstallAnswer_YesCopyWithImpl(this._self, this._then);

  final InstallAnswer_Yes _self;
  final $Res Function(InstallAnswer_Yes) _then;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? yes = null,}) {
  return _then(InstallAnswer_Yes(
yes: null == yes ? _self.yes : yes // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class InstallAnswer_Located extends InstallAnswer {
  const InstallAnswer_Located({this.address}): super._();
  

 final  String? address;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAnswer_LocatedCopyWith<InstallAnswer_Located> get copyWith => _$InstallAnswer_LocatedCopyWithImpl<InstallAnswer_Located>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAnswer_Located&&(identical(other.address, address) || other.address == address));
}


@override
int get hashCode {
    return Object.hash(runtimeType,address);
}

@override
String toString() {
    return 'InstallAnswer.located(address: $address)';
}


}

/// @nodoc
abstract mixin class $InstallAnswer_LocatedCopyWith<$Res> implements $InstallAnswerCopyWith<$Res> {
  factory $InstallAnswer_LocatedCopyWith(InstallAnswer_Located value, $Res Function(InstallAnswer_Located) _then) = _$InstallAnswer_LocatedCopyWithImpl;
@useResult
$Res call({
 String? address
});




}
/// @nodoc
class _$InstallAnswer_LocatedCopyWithImpl<$Res>
    implements $InstallAnswer_LocatedCopyWith<$Res> {
  _$InstallAnswer_LocatedCopyWithImpl(this._self, this._then);

  final InstallAnswer_Located _self;
  final $Res Function(InstallAnswer_Located) _then;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? address = freezed,}) {
  return _then(InstallAnswer_Located(
address: freezed == address ? _self.address : address // ignore: cast_nullable_to_non_nullable
as String?,
  ));
}


}

/// @nodoc


class InstallAnswer_Failed extends InstallAnswer {
  const InstallAnswer_Failed({required this.reason}): super._();
  

 final  String reason;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAnswer_FailedCopyWith<InstallAnswer_Failed> get copyWith => _$InstallAnswer_FailedCopyWithImpl<InstallAnswer_Failed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAnswer_Failed&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'InstallAnswer.failed(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $InstallAnswer_FailedCopyWith<$Res> implements $InstallAnswerCopyWith<$Res> {
  factory $InstallAnswer_FailedCopyWith(InstallAnswer_Failed value, $Res Function(InstallAnswer_Failed) _then) = _$InstallAnswer_FailedCopyWithImpl;
@useResult
$Res call({
 String reason
});




}
/// @nodoc
class _$InstallAnswer_FailedCopyWithImpl<$Res>
    implements $InstallAnswer_FailedCopyWith<$Res> {
  _$InstallAnswer_FailedCopyWithImpl(this._self, this._then);

  final InstallAnswer_Failed _self;
  final $Res Function(InstallAnswer_Failed) _then;

/// Create a copy of InstallAnswer
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(InstallAnswer_Failed(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$InstallAsk {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAsk()';
}


}

/// @nodoc
class $InstallAskCopyWith<$Res>  {
$InstallAskCopyWith(InstallAsk _, $Res Function(InstallAsk) __);
}


/// Adds pattern-matching-related methods to [InstallAsk].
extension InstallAskPatterns on InstallAsk {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallAsk_SetOverride value)?  setOverride,TResult Function( InstallAsk_RestartFeeder value)?  restartFeeder,TResult Function( InstallAsk_RecognizeFeeder value)?  recognizeFeeder,TResult Function( InstallAsk_OpenRecoveryWindow value)?  openRecoveryWindow,TResult Function( InstallAsk_OpenLink value)?  openLink,TResult Function( InstallAsk_WriteLink value)?  writeLink,TResult Function( InstallAsk_CloseLink value)?  closeLink,TResult Function( InstallAsk_Locate value)?  locate,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallAsk_SetOverride() when setOverride != null:
return setOverride(_that);case InstallAsk_RestartFeeder() when restartFeeder != null:
return restartFeeder(_that);case InstallAsk_RecognizeFeeder() when recognizeFeeder != null:
return recognizeFeeder(_that);case InstallAsk_OpenRecoveryWindow() when openRecoveryWindow != null:
return openRecoveryWindow(_that);case InstallAsk_OpenLink() when openLink != null:
return openLink(_that);case InstallAsk_WriteLink() when writeLink != null:
return writeLink(_that);case InstallAsk_CloseLink() when closeLink != null:
return closeLink(_that);case InstallAsk_Locate() when locate != null:
return locate(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallAsk_SetOverride value)  setOverride,required TResult Function( InstallAsk_RestartFeeder value)  restartFeeder,required TResult Function( InstallAsk_RecognizeFeeder value)  recognizeFeeder,required TResult Function( InstallAsk_OpenRecoveryWindow value)  openRecoveryWindow,required TResult Function( InstallAsk_OpenLink value)  openLink,required TResult Function( InstallAsk_WriteLink value)  writeLink,required TResult Function( InstallAsk_CloseLink value)  closeLink,required TResult Function( InstallAsk_Locate value)  locate,}){
final _that = this;
switch (_that) {
case InstallAsk_SetOverride():
return setOverride(_that);case InstallAsk_RestartFeeder():
return restartFeeder(_that);case InstallAsk_RecognizeFeeder():
return recognizeFeeder(_that);case InstallAsk_OpenRecoveryWindow():
return openRecoveryWindow(_that);case InstallAsk_OpenLink():
return openLink(_that);case InstallAsk_WriteLink():
return writeLink(_that);case InstallAsk_CloseLink():
return closeLink(_that);case InstallAsk_Locate():
return locate(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallAsk_SetOverride value)?  setOverride,TResult? Function( InstallAsk_RestartFeeder value)?  restartFeeder,TResult? Function( InstallAsk_RecognizeFeeder value)?  recognizeFeeder,TResult? Function( InstallAsk_OpenRecoveryWindow value)?  openRecoveryWindow,TResult? Function( InstallAsk_OpenLink value)?  openLink,TResult? Function( InstallAsk_WriteLink value)?  writeLink,TResult? Function( InstallAsk_CloseLink value)?  closeLink,TResult? Function( InstallAsk_Locate value)?  locate,}){
final _that = this;
switch (_that) {
case InstallAsk_SetOverride() when setOverride != null:
return setOverride(_that);case InstallAsk_RestartFeeder() when restartFeeder != null:
return restartFeeder(_that);case InstallAsk_RecognizeFeeder() when recognizeFeeder != null:
return recognizeFeeder(_that);case InstallAsk_OpenRecoveryWindow() when openRecoveryWindow != null:
return openRecoveryWindow(_that);case InstallAsk_OpenLink() when openLink != null:
return openLink(_that);case InstallAsk_WriteLink() when writeLink != null:
return writeLink(_that);case InstallAsk_CloseLink() when closeLink != null:
return closeLink(_that);case InstallAsk_Locate() when locate != null:
return locate(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String name,  String broker)?  setOverride,TResult Function()?  restartFeeder,TResult Function( String device,  String address)?  recognizeFeeder,TResult Function()?  openRecoveryWindow,TResult Function()?  openLink,TResult Function( List<Uint8List> pieces)?  writeLink,TResult Function()?  closeLink,TResult Function( String identity)?  locate,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallAsk_SetOverride() when setOverride != null:
return setOverride(_that.name,_that.broker);case InstallAsk_RestartFeeder() when restartFeeder != null:
return restartFeeder();case InstallAsk_RecognizeFeeder() when recognizeFeeder != null:
return recognizeFeeder(_that.device,_that.address);case InstallAsk_OpenRecoveryWindow() when openRecoveryWindow != null:
return openRecoveryWindow();case InstallAsk_OpenLink() when openLink != null:
return openLink();case InstallAsk_WriteLink() when writeLink != null:
return writeLink(_that.pieces);case InstallAsk_CloseLink() when closeLink != null:
return closeLink();case InstallAsk_Locate() when locate != null:
return locate(_that.identity);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String name,  String broker)  setOverride,required TResult Function()  restartFeeder,required TResult Function( String device,  String address)  recognizeFeeder,required TResult Function()  openRecoveryWindow,required TResult Function()  openLink,required TResult Function( List<Uint8List> pieces)  writeLink,required TResult Function()  closeLink,required TResult Function( String identity)  locate,}) {final _that = this;
switch (_that) {
case InstallAsk_SetOverride():
return setOverride(_that.name,_that.broker);case InstallAsk_RestartFeeder():
return restartFeeder();case InstallAsk_RecognizeFeeder():
return recognizeFeeder(_that.device,_that.address);case InstallAsk_OpenRecoveryWindow():
return openRecoveryWindow();case InstallAsk_OpenLink():
return openLink();case InstallAsk_WriteLink():
return writeLink(_that.pieces);case InstallAsk_CloseLink():
return closeLink();case InstallAsk_Locate():
return locate(_that.identity);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String name,  String broker)?  setOverride,TResult? Function()?  restartFeeder,TResult? Function( String device,  String address)?  recognizeFeeder,TResult? Function()?  openRecoveryWindow,TResult? Function()?  openLink,TResult? Function( List<Uint8List> pieces)?  writeLink,TResult? Function()?  closeLink,TResult? Function( String identity)?  locate,}) {final _that = this;
switch (_that) {
case InstallAsk_SetOverride() when setOverride != null:
return setOverride(_that.name,_that.broker);case InstallAsk_RestartFeeder() when restartFeeder != null:
return restartFeeder();case InstallAsk_RecognizeFeeder() when recognizeFeeder != null:
return recognizeFeeder(_that.device,_that.address);case InstallAsk_OpenRecoveryWindow() when openRecoveryWindow != null:
return openRecoveryWindow();case InstallAsk_OpenLink() when openLink != null:
return openLink();case InstallAsk_WriteLink() when writeLink != null:
return writeLink(_that.pieces);case InstallAsk_CloseLink() when closeLink != null:
return closeLink();case InstallAsk_Locate() when locate != null:
return locate(_that.identity);case _:
  return null;

}
}

}

/// @nodoc


class InstallAsk_SetOverride extends InstallAsk {
  const InstallAsk_SetOverride({required this.name, required this.broker}): super._();
  

 final  String name;
 final  String broker;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAsk_SetOverrideCopyWith<InstallAsk_SetOverride> get copyWith => _$InstallAsk_SetOverrideCopyWithImpl<InstallAsk_SetOverride>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_SetOverride&&(identical(other.name, name) || other.name == name)&&(identical(other.broker, broker) || other.broker == broker));
}


@override
int get hashCode {
    return Object.hash(runtimeType,name,broker);
}

@override
String toString() {
    return 'InstallAsk.setOverride(name: $name, broker: $broker)';
}


}

/// @nodoc
abstract mixin class $InstallAsk_SetOverrideCopyWith<$Res> implements $InstallAskCopyWith<$Res> {
  factory $InstallAsk_SetOverrideCopyWith(InstallAsk_SetOverride value, $Res Function(InstallAsk_SetOverride) _then) = _$InstallAsk_SetOverrideCopyWithImpl;
@useResult
$Res call({
 String name, String broker
});




}
/// @nodoc
class _$InstallAsk_SetOverrideCopyWithImpl<$Res>
    implements $InstallAsk_SetOverrideCopyWith<$Res> {
  _$InstallAsk_SetOverrideCopyWithImpl(this._self, this._then);

  final InstallAsk_SetOverride _self;
  final $Res Function(InstallAsk_SetOverride) _then;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? name = null,Object? broker = null,}) {
  return _then(InstallAsk_SetOverride(
name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,broker: null == broker ? _self.broker : broker // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallAsk_RestartFeeder extends InstallAsk {
  const InstallAsk_RestartFeeder(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_RestartFeeder);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAsk.restartFeeder()';
}


}




/// @nodoc


class InstallAsk_RecognizeFeeder extends InstallAsk {
  const InstallAsk_RecognizeFeeder({required this.device, required this.address}): super._();
  

 final  String device;
 final  String address;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAsk_RecognizeFeederCopyWith<InstallAsk_RecognizeFeeder> get copyWith => _$InstallAsk_RecognizeFeederCopyWithImpl<InstallAsk_RecognizeFeeder>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_RecognizeFeeder&&(identical(other.device, device) || other.device == device)&&(identical(other.address, address) || other.address == address));
}


@override
int get hashCode {
    return Object.hash(runtimeType,device,address);
}

@override
String toString() {
    return 'InstallAsk.recognizeFeeder(device: $device, address: $address)';
}


}

/// @nodoc
abstract mixin class $InstallAsk_RecognizeFeederCopyWith<$Res> implements $InstallAskCopyWith<$Res> {
  factory $InstallAsk_RecognizeFeederCopyWith(InstallAsk_RecognizeFeeder value, $Res Function(InstallAsk_RecognizeFeeder) _then) = _$InstallAsk_RecognizeFeederCopyWithImpl;
@useResult
$Res call({
 String device, String address
});




}
/// @nodoc
class _$InstallAsk_RecognizeFeederCopyWithImpl<$Res>
    implements $InstallAsk_RecognizeFeederCopyWith<$Res> {
  _$InstallAsk_RecognizeFeederCopyWithImpl(this._self, this._then);

  final InstallAsk_RecognizeFeeder _self;
  final $Res Function(InstallAsk_RecognizeFeeder) _then;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? device = null,Object? address = null,}) {
  return _then(InstallAsk_RecognizeFeeder(
device: null == device ? _self.device : device // ignore: cast_nullable_to_non_nullable
as String,address: null == address ? _self.address : address // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallAsk_OpenRecoveryWindow extends InstallAsk {
  const InstallAsk_OpenRecoveryWindow(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_OpenRecoveryWindow);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAsk.openRecoveryWindow()';
}


}




/// @nodoc


class InstallAsk_OpenLink extends InstallAsk {
  const InstallAsk_OpenLink(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_OpenLink);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAsk.openLink()';
}


}




/// @nodoc


class InstallAsk_WriteLink extends InstallAsk {
  const InstallAsk_WriteLink({required  List<Uint8List> pieces}): _pieces = pieces,super._();
  

 final  List<Uint8List> _pieces;
 List<Uint8List> get pieces {
  if (_pieces is EqualUnmodifiableListView) return _pieces;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_pieces);
}


/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAsk_WriteLinkCopyWith<InstallAsk_WriteLink> get copyWith => _$InstallAsk_WriteLinkCopyWithImpl<InstallAsk_WriteLink>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_WriteLink&&const DeepCollectionEquality().equals(other.pieces, _pieces));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_pieces));
}

@override
String toString() {
    return 'InstallAsk.writeLink(pieces: $pieces)';
}


}

/// @nodoc
abstract mixin class $InstallAsk_WriteLinkCopyWith<$Res> implements $InstallAskCopyWith<$Res> {
  factory $InstallAsk_WriteLinkCopyWith(InstallAsk_WriteLink value, $Res Function(InstallAsk_WriteLink) _then) = _$InstallAsk_WriteLinkCopyWithImpl;
@useResult
$Res call({
 List<Uint8List> pieces
});




}
/// @nodoc
class _$InstallAsk_WriteLinkCopyWithImpl<$Res>
    implements $InstallAsk_WriteLinkCopyWith<$Res> {
  _$InstallAsk_WriteLinkCopyWithImpl(this._self, this._then);

  final InstallAsk_WriteLink _self;
  final $Res Function(InstallAsk_WriteLink) _then;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? pieces = null,}) {
  return _then(InstallAsk_WriteLink(
pieces: null == pieces ? _self._pieces : pieces // ignore: cast_nullable_to_non_nullable
as List<Uint8List>,
  ));
}


}

/// @nodoc


class InstallAsk_CloseLink extends InstallAsk {
  const InstallAsk_CloseLink(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_CloseLink);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallAsk.closeLink()';
}


}




/// @nodoc


class InstallAsk_Locate extends InstallAsk {
  const InstallAsk_Locate({required this.identity}): super._();
  

 final  String identity;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallAsk_LocateCopyWith<InstallAsk_Locate> get copyWith => _$InstallAsk_LocateCopyWithImpl<InstallAsk_Locate>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallAsk_Locate&&(identical(other.identity, identity) || other.identity == identity));
}


@override
int get hashCode {
    return Object.hash(runtimeType,identity);
}

@override
String toString() {
    return 'InstallAsk.locate(identity: $identity)';
}


}

/// @nodoc
abstract mixin class $InstallAsk_LocateCopyWith<$Res> implements $InstallAskCopyWith<$Res> {
  factory $InstallAsk_LocateCopyWith(InstallAsk_Locate value, $Res Function(InstallAsk_Locate) _then) = _$InstallAsk_LocateCopyWithImpl;
@useResult
$Res call({
 String identity
});




}
/// @nodoc
class _$InstallAsk_LocateCopyWithImpl<$Res>
    implements $InstallAsk_LocateCopyWith<$Res> {
  _$InstallAsk_LocateCopyWithImpl(this._self, this._then);

  final InstallAsk_Locate _self;
  final $Res Function(InstallAsk_Locate) _then;

/// Create a copy of InstallAsk
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? identity = null,}) {
  return _then(InstallAsk_Locate(
identity: null == identity ? _self.identity : identity // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$InstallEnd {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEnd);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEnd()';
}


}

/// @nodoc
class $InstallEndCopyWith<$Res>  {
$InstallEndCopyWith(InstallEnd _, $Res Function(InstallEnd) __);
}


/// Adds pattern-matching-related methods to [InstallEnd].
extension InstallEndPatterns on InstallEnd {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallEnd_Done value)?  done,TResult Function( InstallEnd_Stopped value)?  stopped,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallEnd_Done() when done != null:
return done(_that);case InstallEnd_Stopped() when stopped != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallEnd_Done value)  done,required TResult Function( InstallEnd_Stopped value)  stopped,}){
final _that = this;
switch (_that) {
case InstallEnd_Done():
return done(_that);case InstallEnd_Stopped():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallEnd_Done value)?  done,TResult? Function( InstallEnd_Stopped value)?  stopped,}){
final _that = this;
switch (_that) {
case InstallEnd_Done() when done != null:
return done(_that);case InstallEnd_Stopped() when stopped != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  done,TResult Function( StopCase case_,  Resume resume,  String detail)?  stopped,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallEnd_Done() when done != null:
return done();case InstallEnd_Stopped() when stopped != null:
return stopped(_that.case_,_that.resume,_that.detail);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  done,required TResult Function( StopCase case_,  Resume resume,  String detail)  stopped,}) {final _that = this;
switch (_that) {
case InstallEnd_Done():
return done();case InstallEnd_Stopped():
return stopped(_that.case_,_that.resume,_that.detail);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  done,TResult? Function( StopCase case_,  Resume resume,  String detail)?  stopped,}) {final _that = this;
switch (_that) {
case InstallEnd_Done() when done != null:
return done();case InstallEnd_Stopped() when stopped != null:
return stopped(_that.case_,_that.resume,_that.detail);case _:
  return null;

}
}

}

/// @nodoc


class InstallEnd_Done extends InstallEnd {
  const InstallEnd_Done(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEnd_Done);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEnd.done()';
}


}




/// @nodoc


class InstallEnd_Stopped extends InstallEnd {
  const InstallEnd_Stopped({required this.case_, required this.resume, required this.detail}): super._();
  

 final  StopCase case_;
 final  Resume resume;
/// The run's own words, for the screen's small print.
 final  String detail;

/// Create a copy of InstallEnd
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEnd_StoppedCopyWith<InstallEnd_Stopped> get copyWith => _$InstallEnd_StoppedCopyWithImpl<InstallEnd_Stopped>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEnd_Stopped&&(identical(other.case_, case_) || other.case_ == case_)&&(identical(other.resume, resume) || other.resume == resume)&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,case_,resume,detail);
}

@override
String toString() {
    return 'InstallEnd.stopped(case_: $case_, resume: $resume, detail: $detail)';
}


}

/// @nodoc
abstract mixin class $InstallEnd_StoppedCopyWith<$Res> implements $InstallEndCopyWith<$Res> {
  factory $InstallEnd_StoppedCopyWith(InstallEnd_Stopped value, $Res Function(InstallEnd_Stopped) _then) = _$InstallEnd_StoppedCopyWithImpl;
@useResult
$Res call({
 StopCase case_, Resume resume, String detail
});


$StopCaseCopyWith<$Res> get case_;

}
/// @nodoc
class _$InstallEnd_StoppedCopyWithImpl<$Res>
    implements $InstallEnd_StoppedCopyWith<$Res> {
  _$InstallEnd_StoppedCopyWithImpl(this._self, this._then);

  final InstallEnd_Stopped _self;
  final $Res Function(InstallEnd_Stopped) _then;

/// Create a copy of InstallEnd
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? case_ = null,Object? resume = null,Object? detail = null,}) {
  return _then(InstallEnd_Stopped(
case_: null == case_ ? _self.case_ : case_ // ignore: cast_nullable_to_non_nullable
as StopCase,resume: null == resume ? _self.resume : resume // ignore: cast_nullable_to_non_nullable
as Resume,detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}

/// Create a copy of InstallEnd
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$StopCaseCopyWith<$Res> get case_ {
  
  return $StopCaseCopyWith<$Res>(_self.case_, (value) {
    return _then(_self.copyWith(case_: value));
  });
}
}

/// @nodoc
mixin _$InstallEvent {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent()';
}


}

/// @nodoc
class $InstallEventCopyWith<$Res>  {
$InstallEventCopyWith(InstallEvent _, $Res Function(InstallEvent) __);
}


/// Adds pattern-matching-related methods to [InstallEvent].
extension InstallEventPatterns on InstallEvent {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallEvent_DnsChecked value)?  dnsChecked,TResult Function( InstallEvent_StockServerStarted value)?  stockServerStarted,TResult Function( InstallEvent_AwaitingStock value)?  awaitingStock,TResult Function( InstallEvent_Waiting value)?  waiting,TResult Function( InstallEvent_VersionLost value)?  versionLost,TResult Function( InstallEvent_StockServerEnded value)?  stockServerEnded,TResult Function( InstallEvent_Requesting value)?  requesting,TResult Function( InstallEvent_StockProgress value)?  stockProgress,TResult Function( InstallEvent_StockRestarting value)?  stockRestarting,TResult Function( InstallEvent_KeyKnown value)?  keyKnown,TResult Function( InstallEvent_SetupStarting value)?  setupStarting,TResult Function( InstallEvent_NoSetupFeeder value)?  noSetupFeeder,TResult Function( InstallEvent_Code value)?  code,TResult Function( InstallEvent_PressDispense value)?  pressDispense,TResult Function( InstallEvent_Paired value)?  paired,TResult Function( InstallEvent_Joined value)?  joined,TResult Function( InstallEvent_ReplacementAnswers value)?  replacementAnswers,TResult Function( InstallEvent_ReadingBackup value)?  readingBackup,TResult Function( InstallEvent_BackupRead value)?  backupRead,TResult Function( InstallEvent_BackupKept value)?  backupKept,TResult Function( InstallEvent_StockPlans value)?  stockPlans,TResult Function( InstallEvent_WithoutStockPlans value)?  withoutStockPlans,TResult Function( InstallEvent_Promoting value)?  promoting,TResult Function( InstallEvent_Written value)?  written,TResult Function( InstallEvent_SessionDropped value)?  sessionDropped,TResult Function( InstallEvent_Activated value)?  activated,TResult Function( InstallEvent_PuttingPlans value)?  puttingPlans,TResult Function( InstallEvent_Installed value)?  installed,TResult Function( InstallEvent_Plans value)?  plans,TResult Function( InstallEvent_Note value)?  note,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallEvent_DnsChecked() when dnsChecked != null:
return dnsChecked(_that);case InstallEvent_StockServerStarted() when stockServerStarted != null:
return stockServerStarted(_that);case InstallEvent_AwaitingStock() when awaitingStock != null:
return awaitingStock(_that);case InstallEvent_Waiting() when waiting != null:
return waiting(_that);case InstallEvent_VersionLost() when versionLost != null:
return versionLost(_that);case InstallEvent_StockServerEnded() when stockServerEnded != null:
return stockServerEnded(_that);case InstallEvent_Requesting() when requesting != null:
return requesting(_that);case InstallEvent_StockProgress() when stockProgress != null:
return stockProgress(_that);case InstallEvent_StockRestarting() when stockRestarting != null:
return stockRestarting(_that);case InstallEvent_KeyKnown() when keyKnown != null:
return keyKnown(_that);case InstallEvent_SetupStarting() when setupStarting != null:
return setupStarting(_that);case InstallEvent_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that);case InstallEvent_Code() when code != null:
return code(_that);case InstallEvent_PressDispense() when pressDispense != null:
return pressDispense(_that);case InstallEvent_Paired() when paired != null:
return paired(_that);case InstallEvent_Joined() when joined != null:
return joined(_that);case InstallEvent_ReplacementAnswers() when replacementAnswers != null:
return replacementAnswers(_that);case InstallEvent_ReadingBackup() when readingBackup != null:
return readingBackup(_that);case InstallEvent_BackupRead() when backupRead != null:
return backupRead(_that);case InstallEvent_BackupKept() when backupKept != null:
return backupKept(_that);case InstallEvent_StockPlans() when stockPlans != null:
return stockPlans(_that);case InstallEvent_WithoutStockPlans() when withoutStockPlans != null:
return withoutStockPlans(_that);case InstallEvent_Promoting() when promoting != null:
return promoting(_that);case InstallEvent_Written() when written != null:
return written(_that);case InstallEvent_SessionDropped() when sessionDropped != null:
return sessionDropped(_that);case InstallEvent_Activated() when activated != null:
return activated(_that);case InstallEvent_PuttingPlans() when puttingPlans != null:
return puttingPlans(_that);case InstallEvent_Installed() when installed != null:
return installed(_that);case InstallEvent_Plans() when plans != null:
return plans(_that);case InstallEvent_Note() when note != null:
return note(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallEvent_DnsChecked value)  dnsChecked,required TResult Function( InstallEvent_StockServerStarted value)  stockServerStarted,required TResult Function( InstallEvent_AwaitingStock value)  awaitingStock,required TResult Function( InstallEvent_Waiting value)  waiting,required TResult Function( InstallEvent_VersionLost value)  versionLost,required TResult Function( InstallEvent_StockServerEnded value)  stockServerEnded,required TResult Function( InstallEvent_Requesting value)  requesting,required TResult Function( InstallEvent_StockProgress value)  stockProgress,required TResult Function( InstallEvent_StockRestarting value)  stockRestarting,required TResult Function( InstallEvent_KeyKnown value)  keyKnown,required TResult Function( InstallEvent_SetupStarting value)  setupStarting,required TResult Function( InstallEvent_NoSetupFeeder value)  noSetupFeeder,required TResult Function( InstallEvent_Code value)  code,required TResult Function( InstallEvent_PressDispense value)  pressDispense,required TResult Function( InstallEvent_Paired value)  paired,required TResult Function( InstallEvent_Joined value)  joined,required TResult Function( InstallEvent_ReplacementAnswers value)  replacementAnswers,required TResult Function( InstallEvent_ReadingBackup value)  readingBackup,required TResult Function( InstallEvent_BackupRead value)  backupRead,required TResult Function( InstallEvent_BackupKept value)  backupKept,required TResult Function( InstallEvent_StockPlans value)  stockPlans,required TResult Function( InstallEvent_WithoutStockPlans value)  withoutStockPlans,required TResult Function( InstallEvent_Promoting value)  promoting,required TResult Function( InstallEvent_Written value)  written,required TResult Function( InstallEvent_SessionDropped value)  sessionDropped,required TResult Function( InstallEvent_Activated value)  activated,required TResult Function( InstallEvent_PuttingPlans value)  puttingPlans,required TResult Function( InstallEvent_Installed value)  installed,required TResult Function( InstallEvent_Plans value)  plans,required TResult Function( InstallEvent_Note value)  note,}){
final _that = this;
switch (_that) {
case InstallEvent_DnsChecked():
return dnsChecked(_that);case InstallEvent_StockServerStarted():
return stockServerStarted(_that);case InstallEvent_AwaitingStock():
return awaitingStock(_that);case InstallEvent_Waiting():
return waiting(_that);case InstallEvent_VersionLost():
return versionLost(_that);case InstallEvent_StockServerEnded():
return stockServerEnded(_that);case InstallEvent_Requesting():
return requesting(_that);case InstallEvent_StockProgress():
return stockProgress(_that);case InstallEvent_StockRestarting():
return stockRestarting(_that);case InstallEvent_KeyKnown():
return keyKnown(_that);case InstallEvent_SetupStarting():
return setupStarting(_that);case InstallEvent_NoSetupFeeder():
return noSetupFeeder(_that);case InstallEvent_Code():
return code(_that);case InstallEvent_PressDispense():
return pressDispense(_that);case InstallEvent_Paired():
return paired(_that);case InstallEvent_Joined():
return joined(_that);case InstallEvent_ReplacementAnswers():
return replacementAnswers(_that);case InstallEvent_ReadingBackup():
return readingBackup(_that);case InstallEvent_BackupRead():
return backupRead(_that);case InstallEvent_BackupKept():
return backupKept(_that);case InstallEvent_StockPlans():
return stockPlans(_that);case InstallEvent_WithoutStockPlans():
return withoutStockPlans(_that);case InstallEvent_Promoting():
return promoting(_that);case InstallEvent_Written():
return written(_that);case InstallEvent_SessionDropped():
return sessionDropped(_that);case InstallEvent_Activated():
return activated(_that);case InstallEvent_PuttingPlans():
return puttingPlans(_that);case InstallEvent_Installed():
return installed(_that);case InstallEvent_Plans():
return plans(_that);case InstallEvent_Note():
return note(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallEvent_DnsChecked value)?  dnsChecked,TResult? Function( InstallEvent_StockServerStarted value)?  stockServerStarted,TResult? Function( InstallEvent_AwaitingStock value)?  awaitingStock,TResult? Function( InstallEvent_Waiting value)?  waiting,TResult? Function( InstallEvent_VersionLost value)?  versionLost,TResult? Function( InstallEvent_StockServerEnded value)?  stockServerEnded,TResult? Function( InstallEvent_Requesting value)?  requesting,TResult? Function( InstallEvent_StockProgress value)?  stockProgress,TResult? Function( InstallEvent_StockRestarting value)?  stockRestarting,TResult? Function( InstallEvent_KeyKnown value)?  keyKnown,TResult? Function( InstallEvent_SetupStarting value)?  setupStarting,TResult? Function( InstallEvent_NoSetupFeeder value)?  noSetupFeeder,TResult? Function( InstallEvent_Code value)?  code,TResult? Function( InstallEvent_PressDispense value)?  pressDispense,TResult? Function( InstallEvent_Paired value)?  paired,TResult? Function( InstallEvent_Joined value)?  joined,TResult? Function( InstallEvent_ReplacementAnswers value)?  replacementAnswers,TResult? Function( InstallEvent_ReadingBackup value)?  readingBackup,TResult? Function( InstallEvent_BackupRead value)?  backupRead,TResult? Function( InstallEvent_BackupKept value)?  backupKept,TResult? Function( InstallEvent_StockPlans value)?  stockPlans,TResult? Function( InstallEvent_WithoutStockPlans value)?  withoutStockPlans,TResult? Function( InstallEvent_Promoting value)?  promoting,TResult? Function( InstallEvent_Written value)?  written,TResult? Function( InstallEvent_SessionDropped value)?  sessionDropped,TResult? Function( InstallEvent_Activated value)?  activated,TResult? Function( InstallEvent_PuttingPlans value)?  puttingPlans,TResult? Function( InstallEvent_Installed value)?  installed,TResult? Function( InstallEvent_Plans value)?  plans,TResult? Function( InstallEvent_Note value)?  note,}){
final _that = this;
switch (_that) {
case InstallEvent_DnsChecked() when dnsChecked != null:
return dnsChecked(_that);case InstallEvent_StockServerStarted() when stockServerStarted != null:
return stockServerStarted(_that);case InstallEvent_AwaitingStock() when awaitingStock != null:
return awaitingStock(_that);case InstallEvent_Waiting() when waiting != null:
return waiting(_that);case InstallEvent_VersionLost() when versionLost != null:
return versionLost(_that);case InstallEvent_StockServerEnded() when stockServerEnded != null:
return stockServerEnded(_that);case InstallEvent_Requesting() when requesting != null:
return requesting(_that);case InstallEvent_StockProgress() when stockProgress != null:
return stockProgress(_that);case InstallEvent_StockRestarting() when stockRestarting != null:
return stockRestarting(_that);case InstallEvent_KeyKnown() when keyKnown != null:
return keyKnown(_that);case InstallEvent_SetupStarting() when setupStarting != null:
return setupStarting(_that);case InstallEvent_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that);case InstallEvent_Code() when code != null:
return code(_that);case InstallEvent_PressDispense() when pressDispense != null:
return pressDispense(_that);case InstallEvent_Paired() when paired != null:
return paired(_that);case InstallEvent_Joined() when joined != null:
return joined(_that);case InstallEvent_ReplacementAnswers() when replacementAnswers != null:
return replacementAnswers(_that);case InstallEvent_ReadingBackup() when readingBackup != null:
return readingBackup(_that);case InstallEvent_BackupRead() when backupRead != null:
return backupRead(_that);case InstallEvent_BackupKept() when backupKept != null:
return backupKept(_that);case InstallEvent_StockPlans() when stockPlans != null:
return stockPlans(_that);case InstallEvent_WithoutStockPlans() when withoutStockPlans != null:
return withoutStockPlans(_that);case InstallEvent_Promoting() when promoting != null:
return promoting(_that);case InstallEvent_Written() when written != null:
return written(_that);case InstallEvent_SessionDropped() when sessionDropped != null:
return sessionDropped(_that);case InstallEvent_Activated() when activated != null:
return activated(_that);case InstallEvent_PuttingPlans() when puttingPlans != null:
return puttingPlans(_that);case InstallEvent_Installed() when installed != null:
return installed(_that);case InstallEvent_Plans() when plans != null:
return plans(_that);case InstallEvent_Note() when note != null:
return note(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( DnsResult result)?  dnsChecked,TResult Function()?  stockServerStarted,TResult Function()?  awaitingStock,TResult Function( WaitReason why)?  waiting,TResult Function()?  versionLost,TResult Function( String error)?  stockServerEnded,TResult Function( String build)?  requesting,TResult Function( int percent)?  stockProgress,TResult Function()?  stockRestarting,TResult Function()?  keyKnown,TResult Function()?  setupStarting,TResult Function( String error)?  noSetupFeeder,TResult Function( Uint8List code,  int seconds)?  code,TResult Function()?  pressDispense,TResult Function( Uint8List identity,  String? address,  bool rebooting)?  paired,TResult Function( String address)?  joined,TResult Function( String build,  int bank)?  replacementAnswers,TResult Function( int total)?  readingBackup,TResult Function( BigInt bytes,  int total)?  backupRead,TResult Function( String path)?  backupKept,TResult Function( int converted,  int refused)?  stockPlans,TResult Function()?  withoutStockPlans,TResult Function()?  promoting,TResult Function( int written,  int length)?  written,TResult Function()?  sessionDropped,TResult Function()?  activated,TResult Function()?  puttingPlans,TResult Function( String build)?  installed,TResult Function( PlansOutcome result)?  plans,TResult Function( String text)?  note,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallEvent_DnsChecked() when dnsChecked != null:
return dnsChecked(_that.result);case InstallEvent_StockServerStarted() when stockServerStarted != null:
return stockServerStarted();case InstallEvent_AwaitingStock() when awaitingStock != null:
return awaitingStock();case InstallEvent_Waiting() when waiting != null:
return waiting(_that.why);case InstallEvent_VersionLost() when versionLost != null:
return versionLost();case InstallEvent_StockServerEnded() when stockServerEnded != null:
return stockServerEnded(_that.error);case InstallEvent_Requesting() when requesting != null:
return requesting(_that.build);case InstallEvent_StockProgress() when stockProgress != null:
return stockProgress(_that.percent);case InstallEvent_StockRestarting() when stockRestarting != null:
return stockRestarting();case InstallEvent_KeyKnown() when keyKnown != null:
return keyKnown();case InstallEvent_SetupStarting() when setupStarting != null:
return setupStarting();case InstallEvent_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that.error);case InstallEvent_Code() when code != null:
return code(_that.code,_that.seconds);case InstallEvent_PressDispense() when pressDispense != null:
return pressDispense();case InstallEvent_Paired() when paired != null:
return paired(_that.identity,_that.address,_that.rebooting);case InstallEvent_Joined() when joined != null:
return joined(_that.address);case InstallEvent_ReplacementAnswers() when replacementAnswers != null:
return replacementAnswers(_that.build,_that.bank);case InstallEvent_ReadingBackup() when readingBackup != null:
return readingBackup(_that.total);case InstallEvent_BackupRead() when backupRead != null:
return backupRead(_that.bytes,_that.total);case InstallEvent_BackupKept() when backupKept != null:
return backupKept(_that.path);case InstallEvent_StockPlans() when stockPlans != null:
return stockPlans(_that.converted,_that.refused);case InstallEvent_WithoutStockPlans() when withoutStockPlans != null:
return withoutStockPlans();case InstallEvent_Promoting() when promoting != null:
return promoting();case InstallEvent_Written() when written != null:
return written(_that.written,_that.length);case InstallEvent_SessionDropped() when sessionDropped != null:
return sessionDropped();case InstallEvent_Activated() when activated != null:
return activated();case InstallEvent_PuttingPlans() when puttingPlans != null:
return puttingPlans();case InstallEvent_Installed() when installed != null:
return installed(_that.build);case InstallEvent_Plans() when plans != null:
return plans(_that.result);case InstallEvent_Note() when note != null:
return note(_that.text);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( DnsResult result)  dnsChecked,required TResult Function()  stockServerStarted,required TResult Function()  awaitingStock,required TResult Function( WaitReason why)  waiting,required TResult Function()  versionLost,required TResult Function( String error)  stockServerEnded,required TResult Function( String build)  requesting,required TResult Function( int percent)  stockProgress,required TResult Function()  stockRestarting,required TResult Function()  keyKnown,required TResult Function()  setupStarting,required TResult Function( String error)  noSetupFeeder,required TResult Function( Uint8List code,  int seconds)  code,required TResult Function()  pressDispense,required TResult Function( Uint8List identity,  String? address,  bool rebooting)  paired,required TResult Function( String address)  joined,required TResult Function( String build,  int bank)  replacementAnswers,required TResult Function( int total)  readingBackup,required TResult Function( BigInt bytes,  int total)  backupRead,required TResult Function( String path)  backupKept,required TResult Function( int converted,  int refused)  stockPlans,required TResult Function()  withoutStockPlans,required TResult Function()  promoting,required TResult Function( int written,  int length)  written,required TResult Function()  sessionDropped,required TResult Function()  activated,required TResult Function()  puttingPlans,required TResult Function( String build)  installed,required TResult Function( PlansOutcome result)  plans,required TResult Function( String text)  note,}) {final _that = this;
switch (_that) {
case InstallEvent_DnsChecked():
return dnsChecked(_that.result);case InstallEvent_StockServerStarted():
return stockServerStarted();case InstallEvent_AwaitingStock():
return awaitingStock();case InstallEvent_Waiting():
return waiting(_that.why);case InstallEvent_VersionLost():
return versionLost();case InstallEvent_StockServerEnded():
return stockServerEnded(_that.error);case InstallEvent_Requesting():
return requesting(_that.build);case InstallEvent_StockProgress():
return stockProgress(_that.percent);case InstallEvent_StockRestarting():
return stockRestarting();case InstallEvent_KeyKnown():
return keyKnown();case InstallEvent_SetupStarting():
return setupStarting();case InstallEvent_NoSetupFeeder():
return noSetupFeeder(_that.error);case InstallEvent_Code():
return code(_that.code,_that.seconds);case InstallEvent_PressDispense():
return pressDispense();case InstallEvent_Paired():
return paired(_that.identity,_that.address,_that.rebooting);case InstallEvent_Joined():
return joined(_that.address);case InstallEvent_ReplacementAnswers():
return replacementAnswers(_that.build,_that.bank);case InstallEvent_ReadingBackup():
return readingBackup(_that.total);case InstallEvent_BackupRead():
return backupRead(_that.bytes,_that.total);case InstallEvent_BackupKept():
return backupKept(_that.path);case InstallEvent_StockPlans():
return stockPlans(_that.converted,_that.refused);case InstallEvent_WithoutStockPlans():
return withoutStockPlans();case InstallEvent_Promoting():
return promoting();case InstallEvent_Written():
return written(_that.written,_that.length);case InstallEvent_SessionDropped():
return sessionDropped();case InstallEvent_Activated():
return activated();case InstallEvent_PuttingPlans():
return puttingPlans();case InstallEvent_Installed():
return installed(_that.build);case InstallEvent_Plans():
return plans(_that.result);case InstallEvent_Note():
return note(_that.text);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( DnsResult result)?  dnsChecked,TResult? Function()?  stockServerStarted,TResult? Function()?  awaitingStock,TResult? Function( WaitReason why)?  waiting,TResult? Function()?  versionLost,TResult? Function( String error)?  stockServerEnded,TResult? Function( String build)?  requesting,TResult? Function( int percent)?  stockProgress,TResult? Function()?  stockRestarting,TResult? Function()?  keyKnown,TResult? Function()?  setupStarting,TResult? Function( String error)?  noSetupFeeder,TResult? Function( Uint8List code,  int seconds)?  code,TResult? Function()?  pressDispense,TResult? Function( Uint8List identity,  String? address,  bool rebooting)?  paired,TResult? Function( String address)?  joined,TResult? Function( String build,  int bank)?  replacementAnswers,TResult? Function( int total)?  readingBackup,TResult? Function( BigInt bytes,  int total)?  backupRead,TResult? Function( String path)?  backupKept,TResult? Function( int converted,  int refused)?  stockPlans,TResult? Function()?  withoutStockPlans,TResult? Function()?  promoting,TResult? Function( int written,  int length)?  written,TResult? Function()?  sessionDropped,TResult? Function()?  activated,TResult? Function()?  puttingPlans,TResult? Function( String build)?  installed,TResult? Function( PlansOutcome result)?  plans,TResult? Function( String text)?  note,}) {final _that = this;
switch (_that) {
case InstallEvent_DnsChecked() when dnsChecked != null:
return dnsChecked(_that.result);case InstallEvent_StockServerStarted() when stockServerStarted != null:
return stockServerStarted();case InstallEvent_AwaitingStock() when awaitingStock != null:
return awaitingStock();case InstallEvent_Waiting() when waiting != null:
return waiting(_that.why);case InstallEvent_VersionLost() when versionLost != null:
return versionLost();case InstallEvent_StockServerEnded() when stockServerEnded != null:
return stockServerEnded(_that.error);case InstallEvent_Requesting() when requesting != null:
return requesting(_that.build);case InstallEvent_StockProgress() when stockProgress != null:
return stockProgress(_that.percent);case InstallEvent_StockRestarting() when stockRestarting != null:
return stockRestarting();case InstallEvent_KeyKnown() when keyKnown != null:
return keyKnown();case InstallEvent_SetupStarting() when setupStarting != null:
return setupStarting();case InstallEvent_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that.error);case InstallEvent_Code() when code != null:
return code(_that.code,_that.seconds);case InstallEvent_PressDispense() when pressDispense != null:
return pressDispense();case InstallEvent_Paired() when paired != null:
return paired(_that.identity,_that.address,_that.rebooting);case InstallEvent_Joined() when joined != null:
return joined(_that.address);case InstallEvent_ReplacementAnswers() when replacementAnswers != null:
return replacementAnswers(_that.build,_that.bank);case InstallEvent_ReadingBackup() when readingBackup != null:
return readingBackup(_that.total);case InstallEvent_BackupRead() when backupRead != null:
return backupRead(_that.bytes,_that.total);case InstallEvent_BackupKept() when backupKept != null:
return backupKept(_that.path);case InstallEvent_StockPlans() when stockPlans != null:
return stockPlans(_that.converted,_that.refused);case InstallEvent_WithoutStockPlans() when withoutStockPlans != null:
return withoutStockPlans();case InstallEvent_Promoting() when promoting != null:
return promoting();case InstallEvent_Written() when written != null:
return written(_that.written,_that.length);case InstallEvent_SessionDropped() when sessionDropped != null:
return sessionDropped();case InstallEvent_Activated() when activated != null:
return activated();case InstallEvent_PuttingPlans() when puttingPlans != null:
return puttingPlans();case InstallEvent_Installed() when installed != null:
return installed(_that.build);case InstallEvent_Plans() when plans != null:
return plans(_that.result);case InstallEvent_Note() when note != null:
return note(_that.text);case _:
  return null;

}
}

}

/// @nodoc


class InstallEvent_DnsChecked extends InstallEvent {
  const InstallEvent_DnsChecked({required this.result}): super._();
  

 final  DnsResult result;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_DnsCheckedCopyWith<InstallEvent_DnsChecked> get copyWith => _$InstallEvent_DnsCheckedCopyWithImpl<InstallEvent_DnsChecked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_DnsChecked&&(identical(other.result, result) || other.result == result));
}


@override
int get hashCode {
    return Object.hash(runtimeType,result);
}

@override
String toString() {
    return 'InstallEvent.dnsChecked(result: $result)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_DnsCheckedCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_DnsCheckedCopyWith(InstallEvent_DnsChecked value, $Res Function(InstallEvent_DnsChecked) _then) = _$InstallEvent_DnsCheckedCopyWithImpl;
@useResult
$Res call({
 DnsResult result
});


$DnsResultCopyWith<$Res> get result;

}
/// @nodoc
class _$InstallEvent_DnsCheckedCopyWithImpl<$Res>
    implements $InstallEvent_DnsCheckedCopyWith<$Res> {
  _$InstallEvent_DnsCheckedCopyWithImpl(this._self, this._then);

  final InstallEvent_DnsChecked _self;
  final $Res Function(InstallEvent_DnsChecked) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? result = null,}) {
  return _then(InstallEvent_DnsChecked(
result: null == result ? _self.result : result // ignore: cast_nullable_to_non_nullable
as DnsResult,
  ));
}

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$DnsResultCopyWith<$Res> get result {
  
  return $DnsResultCopyWith<$Res>(_self.result, (value) {
    return _then(_self.copyWith(result: value));
  });
}
}

/// @nodoc


class InstallEvent_StockServerStarted extends InstallEvent {
  const InstallEvent_StockServerStarted(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_StockServerStarted);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.stockServerStarted()';
}


}




/// @nodoc


class InstallEvent_AwaitingStock extends InstallEvent {
  const InstallEvent_AwaitingStock(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_AwaitingStock);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.awaitingStock()';
}


}




/// @nodoc


class InstallEvent_Waiting extends InstallEvent {
  const InstallEvent_Waiting({required this.why}): super._();
  

 final  WaitReason why;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_WaitingCopyWith<InstallEvent_Waiting> get copyWith => _$InstallEvent_WaitingCopyWithImpl<InstallEvent_Waiting>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Waiting&&(identical(other.why, why) || other.why == why));
}


@override
int get hashCode {
    return Object.hash(runtimeType,why);
}

@override
String toString() {
    return 'InstallEvent.waiting(why: $why)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_WaitingCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_WaitingCopyWith(InstallEvent_Waiting value, $Res Function(InstallEvent_Waiting) _then) = _$InstallEvent_WaitingCopyWithImpl;
@useResult
$Res call({
 WaitReason why
});




}
/// @nodoc
class _$InstallEvent_WaitingCopyWithImpl<$Res>
    implements $InstallEvent_WaitingCopyWith<$Res> {
  _$InstallEvent_WaitingCopyWithImpl(this._self, this._then);

  final InstallEvent_Waiting _self;
  final $Res Function(InstallEvent_Waiting) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? why = null,}) {
  return _then(InstallEvent_Waiting(
why: null == why ? _self.why : why // ignore: cast_nullable_to_non_nullable
as WaitReason,
  ));
}


}

/// @nodoc


class InstallEvent_VersionLost extends InstallEvent {
  const InstallEvent_VersionLost(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_VersionLost);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.versionLost()';
}


}




/// @nodoc


class InstallEvent_StockServerEnded extends InstallEvent {
  const InstallEvent_StockServerEnded({required this.error}): super._();
  

 final  String error;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_StockServerEndedCopyWith<InstallEvent_StockServerEnded> get copyWith => _$InstallEvent_StockServerEndedCopyWithImpl<InstallEvent_StockServerEnded>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_StockServerEnded&&(identical(other.error, error) || other.error == error));
}


@override
int get hashCode {
    return Object.hash(runtimeType,error);
}

@override
String toString() {
    return 'InstallEvent.stockServerEnded(error: $error)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_StockServerEndedCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_StockServerEndedCopyWith(InstallEvent_StockServerEnded value, $Res Function(InstallEvent_StockServerEnded) _then) = _$InstallEvent_StockServerEndedCopyWithImpl;
@useResult
$Res call({
 String error
});




}
/// @nodoc
class _$InstallEvent_StockServerEndedCopyWithImpl<$Res>
    implements $InstallEvent_StockServerEndedCopyWith<$Res> {
  _$InstallEvent_StockServerEndedCopyWithImpl(this._self, this._then);

  final InstallEvent_StockServerEnded _self;
  final $Res Function(InstallEvent_StockServerEnded) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? error = null,}) {
  return _then(InstallEvent_StockServerEnded(
error: null == error ? _self.error : error // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_Requesting extends InstallEvent {
  const InstallEvent_Requesting({required this.build}): super._();
  

 final  String build;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_RequestingCopyWith<InstallEvent_Requesting> get copyWith => _$InstallEvent_RequestingCopyWithImpl<InstallEvent_Requesting>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Requesting&&(identical(other.build, build) || other.build == build));
}


@override
int get hashCode {
    return Object.hash(runtimeType,build);
}

@override
String toString() {
    return 'InstallEvent.requesting(build: $build)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_RequestingCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_RequestingCopyWith(InstallEvent_Requesting value, $Res Function(InstallEvent_Requesting) _then) = _$InstallEvent_RequestingCopyWithImpl;
@useResult
$Res call({
 String build
});




}
/// @nodoc
class _$InstallEvent_RequestingCopyWithImpl<$Res>
    implements $InstallEvent_RequestingCopyWith<$Res> {
  _$InstallEvent_RequestingCopyWithImpl(this._self, this._then);

  final InstallEvent_Requesting _self;
  final $Res Function(InstallEvent_Requesting) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? build = null,}) {
  return _then(InstallEvent_Requesting(
build: null == build ? _self.build : build // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_StockProgress extends InstallEvent {
  const InstallEvent_StockProgress({required this.percent}): super._();
  

 final  int percent;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_StockProgressCopyWith<InstallEvent_StockProgress> get copyWith => _$InstallEvent_StockProgressCopyWithImpl<InstallEvent_StockProgress>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_StockProgress&&(identical(other.percent, percent) || other.percent == percent));
}


@override
int get hashCode {
    return Object.hash(runtimeType,percent);
}

@override
String toString() {
    return 'InstallEvent.stockProgress(percent: $percent)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_StockProgressCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_StockProgressCopyWith(InstallEvent_StockProgress value, $Res Function(InstallEvent_StockProgress) _then) = _$InstallEvent_StockProgressCopyWithImpl;
@useResult
$Res call({
 int percent
});




}
/// @nodoc
class _$InstallEvent_StockProgressCopyWithImpl<$Res>
    implements $InstallEvent_StockProgressCopyWith<$Res> {
  _$InstallEvent_StockProgressCopyWithImpl(this._self, this._then);

  final InstallEvent_StockProgress _self;
  final $Res Function(InstallEvent_StockProgress) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? percent = null,}) {
  return _then(InstallEvent_StockProgress(
percent: null == percent ? _self.percent : percent // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_StockRestarting extends InstallEvent {
  const InstallEvent_StockRestarting(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_StockRestarting);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.stockRestarting()';
}


}




/// @nodoc


class InstallEvent_KeyKnown extends InstallEvent {
  const InstallEvent_KeyKnown(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_KeyKnown);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.keyKnown()';
}


}




/// @nodoc


class InstallEvent_SetupStarting extends InstallEvent {
  const InstallEvent_SetupStarting(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_SetupStarting);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.setupStarting()';
}


}




/// @nodoc


class InstallEvent_NoSetupFeeder extends InstallEvent {
  const InstallEvent_NoSetupFeeder({required this.error}): super._();
  

 final  String error;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_NoSetupFeederCopyWith<InstallEvent_NoSetupFeeder> get copyWith => _$InstallEvent_NoSetupFeederCopyWithImpl<InstallEvent_NoSetupFeeder>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_NoSetupFeeder&&(identical(other.error, error) || other.error == error));
}


@override
int get hashCode {
    return Object.hash(runtimeType,error);
}

@override
String toString() {
    return 'InstallEvent.noSetupFeeder(error: $error)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_NoSetupFeederCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_NoSetupFeederCopyWith(InstallEvent_NoSetupFeeder value, $Res Function(InstallEvent_NoSetupFeeder) _then) = _$InstallEvent_NoSetupFeederCopyWithImpl;
@useResult
$Res call({
 String error
});




}
/// @nodoc
class _$InstallEvent_NoSetupFeederCopyWithImpl<$Res>
    implements $InstallEvent_NoSetupFeederCopyWith<$Res> {
  _$InstallEvent_NoSetupFeederCopyWithImpl(this._self, this._then);

  final InstallEvent_NoSetupFeeder _self;
  final $Res Function(InstallEvent_NoSetupFeeder) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? error = null,}) {
  return _then(InstallEvent_NoSetupFeeder(
error: null == error ? _self.error : error // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_Code extends InstallEvent {
  const InstallEvent_Code({required this.code, required this.seconds}): super._();
  

 final  Uint8List code;
 final  int seconds;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_CodeCopyWith<InstallEvent_Code> get copyWith => _$InstallEvent_CodeCopyWithImpl<InstallEvent_Code>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Code&&const DeepCollectionEquality().equals(other.code, code)&&(identical(other.seconds, seconds) || other.seconds == seconds));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(code),seconds);
}

@override
String toString() {
    return 'InstallEvent.code(code: $code, seconds: $seconds)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_CodeCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_CodeCopyWith(InstallEvent_Code value, $Res Function(InstallEvent_Code) _then) = _$InstallEvent_CodeCopyWithImpl;
@useResult
$Res call({
 Uint8List code, int seconds
});




}
/// @nodoc
class _$InstallEvent_CodeCopyWithImpl<$Res>
    implements $InstallEvent_CodeCopyWith<$Res> {
  _$InstallEvent_CodeCopyWithImpl(this._self, this._then);

  final InstallEvent_Code _self;
  final $Res Function(InstallEvent_Code) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? code = null,Object? seconds = null,}) {
  return _then(InstallEvent_Code(
code: null == code ? _self.code : code // ignore: cast_nullable_to_non_nullable
as Uint8List,seconds: null == seconds ? _self.seconds : seconds // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_PressDispense extends InstallEvent {
  const InstallEvent_PressDispense(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_PressDispense);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.pressDispense()';
}


}




/// @nodoc


class InstallEvent_Paired extends InstallEvent {
  const InstallEvent_Paired({required this.identity, this.address, required this.rebooting}): super._();
  

 final  Uint8List identity;
 final  String? address;
 final  bool rebooting;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_PairedCopyWith<InstallEvent_Paired> get copyWith => _$InstallEvent_PairedCopyWithImpl<InstallEvent_Paired>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Paired&&const DeepCollectionEquality().equals(other.identity, identity)&&(identical(other.address, address) || other.address == address)&&(identical(other.rebooting, rebooting) || other.rebooting == rebooting));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(identity),address,rebooting);
}

@override
String toString() {
    return 'InstallEvent.paired(identity: $identity, address: $address, rebooting: $rebooting)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_PairedCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_PairedCopyWith(InstallEvent_Paired value, $Res Function(InstallEvent_Paired) _then) = _$InstallEvent_PairedCopyWithImpl;
@useResult
$Res call({
 Uint8List identity, String? address, bool rebooting
});




}
/// @nodoc
class _$InstallEvent_PairedCopyWithImpl<$Res>
    implements $InstallEvent_PairedCopyWith<$Res> {
  _$InstallEvent_PairedCopyWithImpl(this._self, this._then);

  final InstallEvent_Paired _self;
  final $Res Function(InstallEvent_Paired) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? identity = null,Object? address = freezed,Object? rebooting = null,}) {
  return _then(InstallEvent_Paired(
identity: null == identity ? _self.identity : identity // ignore: cast_nullable_to_non_nullable
as Uint8List,address: freezed == address ? _self.address : address // ignore: cast_nullable_to_non_nullable
as String?,rebooting: null == rebooting ? _self.rebooting : rebooting // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class InstallEvent_Joined extends InstallEvent {
  const InstallEvent_Joined({required this.address}): super._();
  

 final  String address;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_JoinedCopyWith<InstallEvent_Joined> get copyWith => _$InstallEvent_JoinedCopyWithImpl<InstallEvent_Joined>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Joined&&(identical(other.address, address) || other.address == address));
}


@override
int get hashCode {
    return Object.hash(runtimeType,address);
}

@override
String toString() {
    return 'InstallEvent.joined(address: $address)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_JoinedCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_JoinedCopyWith(InstallEvent_Joined value, $Res Function(InstallEvent_Joined) _then) = _$InstallEvent_JoinedCopyWithImpl;
@useResult
$Res call({
 String address
});




}
/// @nodoc
class _$InstallEvent_JoinedCopyWithImpl<$Res>
    implements $InstallEvent_JoinedCopyWith<$Res> {
  _$InstallEvent_JoinedCopyWithImpl(this._self, this._then);

  final InstallEvent_Joined _self;
  final $Res Function(InstallEvent_Joined) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? address = null,}) {
  return _then(InstallEvent_Joined(
address: null == address ? _self.address : address // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_ReplacementAnswers extends InstallEvent {
  const InstallEvent_ReplacementAnswers({required this.build, required this.bank}): super._();
  

 final  String build;
 final  int bank;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_ReplacementAnswersCopyWith<InstallEvent_ReplacementAnswers> get copyWith => _$InstallEvent_ReplacementAnswersCopyWithImpl<InstallEvent_ReplacementAnswers>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_ReplacementAnswers&&(identical(other.build, build) || other.build == build)&&(identical(other.bank, bank) || other.bank == bank));
}


@override
int get hashCode {
    return Object.hash(runtimeType,build,bank);
}

@override
String toString() {
    return 'InstallEvent.replacementAnswers(build: $build, bank: $bank)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_ReplacementAnswersCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_ReplacementAnswersCopyWith(InstallEvent_ReplacementAnswers value, $Res Function(InstallEvent_ReplacementAnswers) _then) = _$InstallEvent_ReplacementAnswersCopyWithImpl;
@useResult
$Res call({
 String build, int bank
});




}
/// @nodoc
class _$InstallEvent_ReplacementAnswersCopyWithImpl<$Res>
    implements $InstallEvent_ReplacementAnswersCopyWith<$Res> {
  _$InstallEvent_ReplacementAnswersCopyWithImpl(this._self, this._then);

  final InstallEvent_ReplacementAnswers _self;
  final $Res Function(InstallEvent_ReplacementAnswers) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? build = null,Object? bank = null,}) {
  return _then(InstallEvent_ReplacementAnswers(
build: null == build ? _self.build : build // ignore: cast_nullable_to_non_nullable
as String,bank: null == bank ? _self.bank : bank // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_ReadingBackup extends InstallEvent {
  const InstallEvent_ReadingBackup({required this.total}): super._();
  

 final  int total;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_ReadingBackupCopyWith<InstallEvent_ReadingBackup> get copyWith => _$InstallEvent_ReadingBackupCopyWithImpl<InstallEvent_ReadingBackup>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_ReadingBackup&&(identical(other.total, total) || other.total == total));
}


@override
int get hashCode {
    return Object.hash(runtimeType,total);
}

@override
String toString() {
    return 'InstallEvent.readingBackup(total: $total)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_ReadingBackupCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_ReadingBackupCopyWith(InstallEvent_ReadingBackup value, $Res Function(InstallEvent_ReadingBackup) _then) = _$InstallEvent_ReadingBackupCopyWithImpl;
@useResult
$Res call({
 int total
});




}
/// @nodoc
class _$InstallEvent_ReadingBackupCopyWithImpl<$Res>
    implements $InstallEvent_ReadingBackupCopyWith<$Res> {
  _$InstallEvent_ReadingBackupCopyWithImpl(this._self, this._then);

  final InstallEvent_ReadingBackup _self;
  final $Res Function(InstallEvent_ReadingBackup) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? total = null,}) {
  return _then(InstallEvent_ReadingBackup(
total: null == total ? _self.total : total // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_BackupRead extends InstallEvent {
  const InstallEvent_BackupRead({required this.bytes, required this.total}): super._();
  

 final  BigInt bytes;
 final  int total;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_BackupReadCopyWith<InstallEvent_BackupRead> get copyWith => _$InstallEvent_BackupReadCopyWithImpl<InstallEvent_BackupRead>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_BackupRead&&(identical(other.bytes, bytes) || other.bytes == bytes)&&(identical(other.total, total) || other.total == total));
}


@override
int get hashCode {
    return Object.hash(runtimeType,bytes,total);
}

@override
String toString() {
    return 'InstallEvent.backupRead(bytes: $bytes, total: $total)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_BackupReadCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_BackupReadCopyWith(InstallEvent_BackupRead value, $Res Function(InstallEvent_BackupRead) _then) = _$InstallEvent_BackupReadCopyWithImpl;
@useResult
$Res call({
 BigInt bytes, int total
});




}
/// @nodoc
class _$InstallEvent_BackupReadCopyWithImpl<$Res>
    implements $InstallEvent_BackupReadCopyWith<$Res> {
  _$InstallEvent_BackupReadCopyWithImpl(this._self, this._then);

  final InstallEvent_BackupRead _self;
  final $Res Function(InstallEvent_BackupRead) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? bytes = null,Object? total = null,}) {
  return _then(InstallEvent_BackupRead(
bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as BigInt,total: null == total ? _self.total : total // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_BackupKept extends InstallEvent {
  const InstallEvent_BackupKept({required this.path}): super._();
  

 final  String path;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_BackupKeptCopyWith<InstallEvent_BackupKept> get copyWith => _$InstallEvent_BackupKeptCopyWithImpl<InstallEvent_BackupKept>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_BackupKept&&(identical(other.path, path) || other.path == path));
}


@override
int get hashCode {
    return Object.hash(runtimeType,path);
}

@override
String toString() {
    return 'InstallEvent.backupKept(path: $path)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_BackupKeptCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_BackupKeptCopyWith(InstallEvent_BackupKept value, $Res Function(InstallEvent_BackupKept) _then) = _$InstallEvent_BackupKeptCopyWithImpl;
@useResult
$Res call({
 String path
});




}
/// @nodoc
class _$InstallEvent_BackupKeptCopyWithImpl<$Res>
    implements $InstallEvent_BackupKeptCopyWith<$Res> {
  _$InstallEvent_BackupKeptCopyWithImpl(this._self, this._then);

  final InstallEvent_BackupKept _self;
  final $Res Function(InstallEvent_BackupKept) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? path = null,}) {
  return _then(InstallEvent_BackupKept(
path: null == path ? _self.path : path // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_StockPlans extends InstallEvent {
  const InstallEvent_StockPlans({required this.converted, required this.refused}): super._();
  

 final  int converted;
 final  int refused;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_StockPlansCopyWith<InstallEvent_StockPlans> get copyWith => _$InstallEvent_StockPlansCopyWithImpl<InstallEvent_StockPlans>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_StockPlans&&(identical(other.converted, converted) || other.converted == converted)&&(identical(other.refused, refused) || other.refused == refused));
}


@override
int get hashCode {
    return Object.hash(runtimeType,converted,refused);
}

@override
String toString() {
    return 'InstallEvent.stockPlans(converted: $converted, refused: $refused)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_StockPlansCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_StockPlansCopyWith(InstallEvent_StockPlans value, $Res Function(InstallEvent_StockPlans) _then) = _$InstallEvent_StockPlansCopyWithImpl;
@useResult
$Res call({
 int converted, int refused
});




}
/// @nodoc
class _$InstallEvent_StockPlansCopyWithImpl<$Res>
    implements $InstallEvent_StockPlansCopyWith<$Res> {
  _$InstallEvent_StockPlansCopyWithImpl(this._self, this._then);

  final InstallEvent_StockPlans _self;
  final $Res Function(InstallEvent_StockPlans) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? converted = null,Object? refused = null,}) {
  return _then(InstallEvent_StockPlans(
converted: null == converted ? _self.converted : converted // ignore: cast_nullable_to_non_nullable
as int,refused: null == refused ? _self.refused : refused // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_WithoutStockPlans extends InstallEvent {
  const InstallEvent_WithoutStockPlans(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_WithoutStockPlans);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.withoutStockPlans()';
}


}




/// @nodoc


class InstallEvent_Promoting extends InstallEvent {
  const InstallEvent_Promoting(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Promoting);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.promoting()';
}


}




/// @nodoc


class InstallEvent_Written extends InstallEvent {
  const InstallEvent_Written({required this.written, required this.length}): super._();
  

 final  int written;
 final  int length;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_WrittenCopyWith<InstallEvent_Written> get copyWith => _$InstallEvent_WrittenCopyWithImpl<InstallEvent_Written>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Written&&(identical(other.written, written) || other.written == written)&&(identical(other.length, length) || other.length == length));
}


@override
int get hashCode {
    return Object.hash(runtimeType,written,length);
}

@override
String toString() {
    return 'InstallEvent.written(written: $written, length: $length)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_WrittenCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_WrittenCopyWith(InstallEvent_Written value, $Res Function(InstallEvent_Written) _then) = _$InstallEvent_WrittenCopyWithImpl;
@useResult
$Res call({
 int written, int length
});




}
/// @nodoc
class _$InstallEvent_WrittenCopyWithImpl<$Res>
    implements $InstallEvent_WrittenCopyWith<$Res> {
  _$InstallEvent_WrittenCopyWithImpl(this._self, this._then);

  final InstallEvent_Written _self;
  final $Res Function(InstallEvent_Written) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? written = null,Object? length = null,}) {
  return _then(InstallEvent_Written(
written: null == written ? _self.written : written // ignore: cast_nullable_to_non_nullable
as int,length: null == length ? _self.length : length // ignore: cast_nullable_to_non_nullable
as int,
  ));
}


}

/// @nodoc


class InstallEvent_SessionDropped extends InstallEvent {
  const InstallEvent_SessionDropped(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_SessionDropped);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.sessionDropped()';
}


}




/// @nodoc


class InstallEvent_Activated extends InstallEvent {
  const InstallEvent_Activated(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Activated);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.activated()';
}


}




/// @nodoc


class InstallEvent_PuttingPlans extends InstallEvent {
  const InstallEvent_PuttingPlans(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_PuttingPlans);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallEvent.puttingPlans()';
}


}




/// @nodoc


class InstallEvent_Installed extends InstallEvent {
  const InstallEvent_Installed({required this.build}): super._();
  

 final  String build;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_InstalledCopyWith<InstallEvent_Installed> get copyWith => _$InstallEvent_InstalledCopyWithImpl<InstallEvent_Installed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Installed&&(identical(other.build, build) || other.build == build));
}


@override
int get hashCode {
    return Object.hash(runtimeType,build);
}

@override
String toString() {
    return 'InstallEvent.installed(build: $build)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_InstalledCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_InstalledCopyWith(InstallEvent_Installed value, $Res Function(InstallEvent_Installed) _then) = _$InstallEvent_InstalledCopyWithImpl;
@useResult
$Res call({
 String build
});




}
/// @nodoc
class _$InstallEvent_InstalledCopyWithImpl<$Res>
    implements $InstallEvent_InstalledCopyWith<$Res> {
  _$InstallEvent_InstalledCopyWithImpl(this._self, this._then);

  final InstallEvent_Installed _self;
  final $Res Function(InstallEvent_Installed) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? build = null,}) {
  return _then(InstallEvent_Installed(
build: null == build ? _self.build : build // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallEvent_Plans extends InstallEvent {
  const InstallEvent_Plans({required this.result}): super._();
  

 final  PlansOutcome result;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_PlansCopyWith<InstallEvent_Plans> get copyWith => _$InstallEvent_PlansCopyWithImpl<InstallEvent_Plans>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Plans&&(identical(other.result, result) || other.result == result));
}


@override
int get hashCode {
    return Object.hash(runtimeType,result);
}

@override
String toString() {
    return 'InstallEvent.plans(result: $result)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_PlansCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_PlansCopyWith(InstallEvent_Plans value, $Res Function(InstallEvent_Plans) _then) = _$InstallEvent_PlansCopyWithImpl;
@useResult
$Res call({
 PlansOutcome result
});




}
/// @nodoc
class _$InstallEvent_PlansCopyWithImpl<$Res>
    implements $InstallEvent_PlansCopyWith<$Res> {
  _$InstallEvent_PlansCopyWithImpl(this._self, this._then);

  final InstallEvent_Plans _self;
  final $Res Function(InstallEvent_Plans) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? result = null,}) {
  return _then(InstallEvent_Plans(
result: null == result ? _self.result : result // ignore: cast_nullable_to_non_nullable
as PlansOutcome,
  ));
}


}

/// @nodoc


class InstallEvent_Note extends InstallEvent {
  const InstallEvent_Note({required this.text}): super._();
  

 final  String text;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallEvent_NoteCopyWith<InstallEvent_Note> get copyWith => _$InstallEvent_NoteCopyWithImpl<InstallEvent_Note>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallEvent_Note&&(identical(other.text, text) || other.text == text));
}


@override
int get hashCode {
    return Object.hash(runtimeType,text);
}

@override
String toString() {
    return 'InstallEvent.note(text: $text)';
}


}

/// @nodoc
abstract mixin class $InstallEvent_NoteCopyWith<$Res> implements $InstallEventCopyWith<$Res> {
  factory $InstallEvent_NoteCopyWith(InstallEvent_Note value, $Res Function(InstallEvent_Note) _then) = _$InstallEvent_NoteCopyWithImpl;
@useResult
$Res call({
 String text
});




}
/// @nodoc
class _$InstallEvent_NoteCopyWithImpl<$Res>
    implements $InstallEvent_NoteCopyWith<$Res> {
  _$InstallEvent_NoteCopyWithImpl(this._self, this._then);

  final InstallEvent_Note _self;
  final $Res Function(InstallEvent_Note) _then;

/// Create a copy of InstallEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? text = null,}) {
  return _then(InstallEvent_Note(
text: null == text ? _self.text : text // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc
mixin _$InstallStart {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallStart);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallStart()';
}


}

/// @nodoc
class $InstallStartCopyWith<$Res>  {
$InstallStartCopyWith(InstallStart _, $Res Function(InstallStart) __);
}


/// Adds pattern-matching-related methods to [InstallStart].
extension InstallStartPatterns on InstallStart {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallStart_Dns value)?  dns,TResult Function( InstallStart_Bank0 value)?  bank0,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallStart_Dns() when dns != null:
return dns(_that);case InstallStart_Bank0() when bank0 != null:
return bank0(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallStart_Dns value)  dns,required TResult Function( InstallStart_Bank0 value)  bank0,}){
final _that = this;
switch (_that) {
case InstallStart_Dns():
return dns(_that);case InstallStart_Bank0():
return bank0(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallStart_Dns value)?  dns,TResult? Function( InstallStart_Bank0 value)?  bank0,}){
final _that = this;
switch (_that) {
case InstallStart_Dns() when dns != null:
return dns(_that);case InstallStart_Bank0() when bank0 != null:
return bank0(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String name)?  dns,TResult Function()?  bank0,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallStart_Dns() when dns != null:
return dns(_that.name);case InstallStart_Bank0() when bank0 != null:
return bank0();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String name)  dns,required TResult Function()  bank0,}) {final _that = this;
switch (_that) {
case InstallStart_Dns():
return dns(_that.name);case InstallStart_Bank0():
return bank0();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String name)?  dns,TResult? Function()?  bank0,}) {final _that = this;
switch (_that) {
case InstallStart_Dns() when dns != null:
return dns(_that.name);case InstallStart_Bank0() when bank0 != null:
return bank0();case _:
  return null;

}
}

}

/// @nodoc


class InstallStart_Dns extends InstallStart {
  const InstallStart_Dns({required this.name}): super._();
  

 final  String name;

/// Create a copy of InstallStart
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallStart_DnsCopyWith<InstallStart_Dns> get copyWith => _$InstallStart_DnsCopyWithImpl<InstallStart_Dns>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallStart_Dns&&(identical(other.name, name) || other.name == name));
}


@override
int get hashCode {
    return Object.hash(runtimeType,name);
}

@override
String toString() {
    return 'InstallStart.dns(name: $name)';
}


}

/// @nodoc
abstract mixin class $InstallStart_DnsCopyWith<$Res> implements $InstallStartCopyWith<$Res> {
  factory $InstallStart_DnsCopyWith(InstallStart_Dns value, $Res Function(InstallStart_Dns) _then) = _$InstallStart_DnsCopyWithImpl;
@useResult
$Res call({
 String name
});




}
/// @nodoc
class _$InstallStart_DnsCopyWithImpl<$Res>
    implements $InstallStart_DnsCopyWith<$Res> {
  _$InstallStart_DnsCopyWithImpl(this._self, this._then);

  final InstallStart_Dns _self;
  final $Res Function(InstallStart_Dns) _then;

/// Create a copy of InstallStart
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? name = null,}) {
  return _then(InstallStart_Dns(
name: null == name ? _self.name : name // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class InstallStart_Bank0 extends InstallStart {
  const InstallStart_Bank0(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallStart_Bank0);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallStart.bank0()';
}


}




/// @nodoc
mixin _$InstallUpdate {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallUpdate);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'InstallUpdate()';
}


}

/// @nodoc
class $InstallUpdateCopyWith<$Res>  {
$InstallUpdateCopyWith(InstallUpdate _, $Res Function(InstallUpdate) __);
}


/// Adds pattern-matching-related methods to [InstallUpdate].
extension InstallUpdatePatterns on InstallUpdate {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( InstallUpdate_Event value)?  event,TResult Function( InstallUpdate_Ask value)?  ask,TResult Function( InstallUpdate_Ended value)?  ended,required TResult orElse(),}){
final _that = this;
switch (_that) {
case InstallUpdate_Event() when event != null:
return event(_that);case InstallUpdate_Ask() when ask != null:
return ask(_that);case InstallUpdate_Ended() when ended != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( InstallUpdate_Event value)  event,required TResult Function( InstallUpdate_Ask value)  ask,required TResult Function( InstallUpdate_Ended value)  ended,}){
final _that = this;
switch (_that) {
case InstallUpdate_Event():
return event(_that);case InstallUpdate_Ask():
return ask(_that);case InstallUpdate_Ended():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( InstallUpdate_Event value)?  event,TResult? Function( InstallUpdate_Ask value)?  ask,TResult? Function( InstallUpdate_Ended value)?  ended,}){
final _that = this;
switch (_that) {
case InstallUpdate_Event() when event != null:
return event(_that);case InstallUpdate_Ask() when ask != null:
return ask(_that);case InstallUpdate_Ended() when ended != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( InstallEvent event)?  event,TResult Function( int id,  InstallAsk ask)?  ask,TResult Function( InstallEnd end)?  ended,required TResult orElse(),}) {final _that = this;
switch (_that) {
case InstallUpdate_Event() when event != null:
return event(_that.event);case InstallUpdate_Ask() when ask != null:
return ask(_that.id,_that.ask);case InstallUpdate_Ended() when ended != null:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( InstallEvent event)  event,required TResult Function( int id,  InstallAsk ask)  ask,required TResult Function( InstallEnd end)  ended,}) {final _that = this;
switch (_that) {
case InstallUpdate_Event():
return event(_that.event);case InstallUpdate_Ask():
return ask(_that.id,_that.ask);case InstallUpdate_Ended():
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( InstallEvent event)?  event,TResult? Function( int id,  InstallAsk ask)?  ask,TResult? Function( InstallEnd end)?  ended,}) {final _that = this;
switch (_that) {
case InstallUpdate_Event() when event != null:
return event(_that.event);case InstallUpdate_Ask() when ask != null:
return ask(_that.id,_that.ask);case InstallUpdate_Ended() when ended != null:
return ended(_that.end);case _:
  return null;

}
}

}

/// @nodoc


class InstallUpdate_Event extends InstallUpdate {
  const InstallUpdate_Event({required this.event}): super._();
  

 final  InstallEvent event;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallUpdate_EventCopyWith<InstallUpdate_Event> get copyWith => _$InstallUpdate_EventCopyWithImpl<InstallUpdate_Event>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallUpdate_Event&&(identical(other.event, event) || other.event == event));
}


@override
int get hashCode {
    return Object.hash(runtimeType,event);
}

@override
String toString() {
    return 'InstallUpdate.event(event: $event)';
}


}

/// @nodoc
abstract mixin class $InstallUpdate_EventCopyWith<$Res> implements $InstallUpdateCopyWith<$Res> {
  factory $InstallUpdate_EventCopyWith(InstallUpdate_Event value, $Res Function(InstallUpdate_Event) _then) = _$InstallUpdate_EventCopyWithImpl;
@useResult
$Res call({
 InstallEvent event
});


$InstallEventCopyWith<$Res> get event;

}
/// @nodoc
class _$InstallUpdate_EventCopyWithImpl<$Res>
    implements $InstallUpdate_EventCopyWith<$Res> {
  _$InstallUpdate_EventCopyWithImpl(this._self, this._then);

  final InstallUpdate_Event _self;
  final $Res Function(InstallUpdate_Event) _then;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? event = null,}) {
  return _then(InstallUpdate_Event(
event: null == event ? _self.event : event // ignore: cast_nullable_to_non_nullable
as InstallEvent,
  ));
}

/// Create a copy of InstallUpdate
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


class InstallUpdate_Ask extends InstallUpdate {
  const InstallUpdate_Ask({required this.id, required this.ask}): super._();
  

 final  int id;
 final  InstallAsk ask;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallUpdate_AskCopyWith<InstallUpdate_Ask> get copyWith => _$InstallUpdate_AskCopyWithImpl<InstallUpdate_Ask>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallUpdate_Ask&&(identical(other.id, id) || other.id == id)&&(identical(other.ask, ask) || other.ask == ask));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,ask);
}

@override
String toString() {
    return 'InstallUpdate.ask(id: $id, ask: $ask)';
}


}

/// @nodoc
abstract mixin class $InstallUpdate_AskCopyWith<$Res> implements $InstallUpdateCopyWith<$Res> {
  factory $InstallUpdate_AskCopyWith(InstallUpdate_Ask value, $Res Function(InstallUpdate_Ask) _then) = _$InstallUpdate_AskCopyWithImpl;
@useResult
$Res call({
 int id, InstallAsk ask
});


$InstallAskCopyWith<$Res> get ask;

}
/// @nodoc
class _$InstallUpdate_AskCopyWithImpl<$Res>
    implements $InstallUpdate_AskCopyWith<$Res> {
  _$InstallUpdate_AskCopyWithImpl(this._self, this._then);

  final InstallUpdate_Ask _self;
  final $Res Function(InstallUpdate_Ask) _then;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? ask = null,}) {
  return _then(InstallUpdate_Ask(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as int,ask: null == ask ? _self.ask : ask // ignore: cast_nullable_to_non_nullable
as InstallAsk,
  ));
}

/// Create a copy of InstallUpdate
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


class InstallUpdate_Ended extends InstallUpdate {
  const InstallUpdate_Ended({required this.end}): super._();
  

 final  InstallEnd end;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$InstallUpdate_EndedCopyWith<InstallUpdate_Ended> get copyWith => _$InstallUpdate_EndedCopyWithImpl<InstallUpdate_Ended>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is InstallUpdate_Ended&&(identical(other.end, end) || other.end == end));
}


@override
int get hashCode {
    return Object.hash(runtimeType,end);
}

@override
String toString() {
    return 'InstallUpdate.ended(end: $end)';
}


}

/// @nodoc
abstract mixin class $InstallUpdate_EndedCopyWith<$Res> implements $InstallUpdateCopyWith<$Res> {
  factory $InstallUpdate_EndedCopyWith(InstallUpdate_Ended value, $Res Function(InstallUpdate_Ended) _then) = _$InstallUpdate_EndedCopyWithImpl;
@useResult
$Res call({
 InstallEnd end
});


$InstallEndCopyWith<$Res> get end;

}
/// @nodoc
class _$InstallUpdate_EndedCopyWithImpl<$Res>
    implements $InstallUpdate_EndedCopyWith<$Res> {
  _$InstallUpdate_EndedCopyWithImpl(this._self, this._then);

  final InstallUpdate_Ended _self;
  final $Res Function(InstallUpdate_Ended) _then;

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? end = null,}) {
  return _then(InstallUpdate_Ended(
end: null == end ? _self.end : end // ignore: cast_nullable_to_non_nullable
as InstallEnd,
  ));
}

/// Create a copy of InstallUpdate
/// with the given fields replaced by the non-null parameter values.
@override
@pragma('vm:prefer-inline')
$InstallEndCopyWith<$Res> get end {
  
  return $InstallEndCopyWith<$Res>(_self.end, (value) {
    return _then(_self.copyWith(end: value));
  });
}
}

/// @nodoc
mixin _$ListenUpdate {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ListenUpdate()';
}


}

/// @nodoc
class $ListenUpdateCopyWith<$Res>  {
$ListenUpdateCopyWith(ListenUpdate _, $Res Function(ListenUpdate) __);
}


/// Adds pattern-matching-related methods to [ListenUpdate].
extension ListenUpdatePatterns on ListenUpdate {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ListenUpdate_Tick value)?  tick,TResult Function( ListenUpdate_Replacement value)?  replacement,TResult Function( ListenUpdate_StockStayed value)?  stockStayed,TResult Function( ListenUpdate_Silent value)?  silent,TResult Function( ListenUpdate_Failed value)?  failed,TResult Function( ListenUpdate_Stopped value)?  stopped,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ListenUpdate_Tick() when tick != null:
return tick(_that);case ListenUpdate_Replacement() when replacement != null:
return replacement(_that);case ListenUpdate_StockStayed() when stockStayed != null:
return stockStayed(_that);case ListenUpdate_Silent() when silent != null:
return silent(_that);case ListenUpdate_Failed() when failed != null:
return failed(_that);case ListenUpdate_Stopped() when stopped != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ListenUpdate_Tick value)  tick,required TResult Function( ListenUpdate_Replacement value)  replacement,required TResult Function( ListenUpdate_StockStayed value)  stockStayed,required TResult Function( ListenUpdate_Silent value)  silent,required TResult Function( ListenUpdate_Failed value)  failed,required TResult Function( ListenUpdate_Stopped value)  stopped,}){
final _that = this;
switch (_that) {
case ListenUpdate_Tick():
return tick(_that);case ListenUpdate_Replacement():
return replacement(_that);case ListenUpdate_StockStayed():
return stockStayed(_that);case ListenUpdate_Silent():
return silent(_that);case ListenUpdate_Failed():
return failed(_that);case ListenUpdate_Stopped():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ListenUpdate_Tick value)?  tick,TResult? Function( ListenUpdate_Replacement value)?  replacement,TResult? Function( ListenUpdate_StockStayed value)?  stockStayed,TResult? Function( ListenUpdate_Silent value)?  silent,TResult? Function( ListenUpdate_Failed value)?  failed,TResult? Function( ListenUpdate_Stopped value)?  stopped,}){
final _that = this;
switch (_that) {
case ListenUpdate_Tick() when tick != null:
return tick(_that);case ListenUpdate_Replacement() when replacement != null:
return replacement(_that);case ListenUpdate_StockStayed() when stockStayed != null:
return stockStayed(_that);case ListenUpdate_Silent() when silent != null:
return silent(_that);case ListenUpdate_Failed() when failed != null:
return failed(_that);case ListenUpdate_Stopped() when stopped != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( int seconds,  bool stock)?  tick,TResult Function( String address)?  replacement,TResult Function( bool resolved)?  stockStayed,TResult Function()?  silent,TResult Function( String detail)?  failed,TResult Function()?  stopped,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ListenUpdate_Tick() when tick != null:
return tick(_that.seconds,_that.stock);case ListenUpdate_Replacement() when replacement != null:
return replacement(_that.address);case ListenUpdate_StockStayed() when stockStayed != null:
return stockStayed(_that.resolved);case ListenUpdate_Silent() when silent != null:
return silent();case ListenUpdate_Failed() when failed != null:
return failed(_that.detail);case ListenUpdate_Stopped() when stopped != null:
return stopped();case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( int seconds,  bool stock)  tick,required TResult Function( String address)  replacement,required TResult Function( bool resolved)  stockStayed,required TResult Function()  silent,required TResult Function( String detail)  failed,required TResult Function()  stopped,}) {final _that = this;
switch (_that) {
case ListenUpdate_Tick():
return tick(_that.seconds,_that.stock);case ListenUpdate_Replacement():
return replacement(_that.address);case ListenUpdate_StockStayed():
return stockStayed(_that.resolved);case ListenUpdate_Silent():
return silent();case ListenUpdate_Failed():
return failed(_that.detail);case ListenUpdate_Stopped():
return stopped();}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( int seconds,  bool stock)?  tick,TResult? Function( String address)?  replacement,TResult? Function( bool resolved)?  stockStayed,TResult? Function()?  silent,TResult? Function( String detail)?  failed,TResult? Function()?  stopped,}) {final _that = this;
switch (_that) {
case ListenUpdate_Tick() when tick != null:
return tick(_that.seconds,_that.stock);case ListenUpdate_Replacement() when replacement != null:
return replacement(_that.address);case ListenUpdate_StockStayed() when stockStayed != null:
return stockStayed(_that.resolved);case ListenUpdate_Silent() when silent != null:
return silent();case ListenUpdate_Failed() when failed != null:
return failed(_that.detail);case ListenUpdate_Stopped() when stopped != null:
return stopped();case _:
  return null;

}
}

}

/// @nodoc


class ListenUpdate_Tick extends ListenUpdate {
  const ListenUpdate_Tick({required this.seconds, required this.stock}): super._();
  

 final  int seconds;
/// Stock has reported its version since the request.
 final  bool stock;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ListenUpdate_TickCopyWith<ListenUpdate_Tick> get copyWith => _$ListenUpdate_TickCopyWithImpl<ListenUpdate_Tick>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_Tick&&(identical(other.seconds, seconds) || other.seconds == seconds)&&(identical(other.stock, stock) || other.stock == stock));
}


@override
int get hashCode {
    return Object.hash(runtimeType,seconds,stock);
}

@override
String toString() {
    return 'ListenUpdate.tick(seconds: $seconds, stock: $stock)';
}


}

/// @nodoc
abstract mixin class $ListenUpdate_TickCopyWith<$Res> implements $ListenUpdateCopyWith<$Res> {
  factory $ListenUpdate_TickCopyWith(ListenUpdate_Tick value, $Res Function(ListenUpdate_Tick) _then) = _$ListenUpdate_TickCopyWithImpl;
@useResult
$Res call({
 int seconds, bool stock
});




}
/// @nodoc
class _$ListenUpdate_TickCopyWithImpl<$Res>
    implements $ListenUpdate_TickCopyWith<$Res> {
  _$ListenUpdate_TickCopyWithImpl(this._self, this._then);

  final ListenUpdate_Tick _self;
  final $Res Function(ListenUpdate_Tick) _then;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? seconds = null,Object? stock = null,}) {
  return _then(ListenUpdate_Tick(
seconds: null == seconds ? _self.seconds : seconds // ignore: cast_nullable_to_non_nullable
as int,stock: null == stock ? _self.stock : stock // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class ListenUpdate_Replacement extends ListenUpdate {
  const ListenUpdate_Replacement({required this.address}): super._();
  

 final  String address;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ListenUpdate_ReplacementCopyWith<ListenUpdate_Replacement> get copyWith => _$ListenUpdate_ReplacementCopyWithImpl<ListenUpdate_Replacement>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_Replacement&&(identical(other.address, address) || other.address == address));
}


@override
int get hashCode {
    return Object.hash(runtimeType,address);
}

@override
String toString() {
    return 'ListenUpdate.replacement(address: $address)';
}


}

/// @nodoc
abstract mixin class $ListenUpdate_ReplacementCopyWith<$Res> implements $ListenUpdateCopyWith<$Res> {
  factory $ListenUpdate_ReplacementCopyWith(ListenUpdate_Replacement value, $Res Function(ListenUpdate_Replacement) _then) = _$ListenUpdate_ReplacementCopyWithImpl;
@useResult
$Res call({
 String address
});




}
/// @nodoc
class _$ListenUpdate_ReplacementCopyWithImpl<$Res>
    implements $ListenUpdate_ReplacementCopyWith<$Res> {
  _$ListenUpdate_ReplacementCopyWithImpl(this._self, this._then);

  final ListenUpdate_Replacement _self;
  final $Res Function(ListenUpdate_Replacement) _then;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? address = null,}) {
  return _then(ListenUpdate_Replacement(
address: null == address ? _self.address : address // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ListenUpdate_StockStayed extends ListenUpdate {
  const ListenUpdate_StockStayed({required this.resolved}): super._();
  

 final  bool resolved;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ListenUpdate_StockStayedCopyWith<ListenUpdate_StockStayed> get copyWith => _$ListenUpdate_StockStayedCopyWithImpl<ListenUpdate_StockStayed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_StockStayed&&(identical(other.resolved, resolved) || other.resolved == resolved));
}


@override
int get hashCode {
    return Object.hash(runtimeType,resolved);
}

@override
String toString() {
    return 'ListenUpdate.stockStayed(resolved: $resolved)';
}


}

/// @nodoc
abstract mixin class $ListenUpdate_StockStayedCopyWith<$Res> implements $ListenUpdateCopyWith<$Res> {
  factory $ListenUpdate_StockStayedCopyWith(ListenUpdate_StockStayed value, $Res Function(ListenUpdate_StockStayed) _then) = _$ListenUpdate_StockStayedCopyWithImpl;
@useResult
$Res call({
 bool resolved
});




}
/// @nodoc
class _$ListenUpdate_StockStayedCopyWithImpl<$Res>
    implements $ListenUpdate_StockStayedCopyWith<$Res> {
  _$ListenUpdate_StockStayedCopyWithImpl(this._self, this._then);

  final ListenUpdate_StockStayed _self;
  final $Res Function(ListenUpdate_StockStayed) _then;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? resolved = null,}) {
  return _then(ListenUpdate_StockStayed(
resolved: null == resolved ? _self.resolved : resolved // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class ListenUpdate_Silent extends ListenUpdate {
  const ListenUpdate_Silent(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_Silent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ListenUpdate.silent()';
}


}




/// @nodoc


class ListenUpdate_Failed extends ListenUpdate {
  const ListenUpdate_Failed({required this.detail}): super._();
  

 final  String detail;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ListenUpdate_FailedCopyWith<ListenUpdate_Failed> get copyWith => _$ListenUpdate_FailedCopyWithImpl<ListenUpdate_Failed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_Failed&&(identical(other.detail, detail) || other.detail == detail));
}


@override
int get hashCode {
    return Object.hash(runtimeType,detail);
}

@override
String toString() {
    return 'ListenUpdate.failed(detail: $detail)';
}


}

/// @nodoc
abstract mixin class $ListenUpdate_FailedCopyWith<$Res> implements $ListenUpdateCopyWith<$Res> {
  factory $ListenUpdate_FailedCopyWith(ListenUpdate_Failed value, $Res Function(ListenUpdate_Failed) _then) = _$ListenUpdate_FailedCopyWithImpl;
@useResult
$Res call({
 String detail
});




}
/// @nodoc
class _$ListenUpdate_FailedCopyWithImpl<$Res>
    implements $ListenUpdate_FailedCopyWith<$Res> {
  _$ListenUpdate_FailedCopyWithImpl(this._self, this._then);

  final ListenUpdate_Failed _self;
  final $Res Function(ListenUpdate_Failed) _then;

/// Create a copy of ListenUpdate
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? detail = null,}) {
  return _then(ListenUpdate_Failed(
detail: null == detail ? _self.detail : detail // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ListenUpdate_Stopped extends ListenUpdate {
  const ListenUpdate_Stopped(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ListenUpdate_Stopped);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ListenUpdate.stopped()';
}


}




/// @nodoc
mixin _$StopCase {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase()';
}


}

/// @nodoc
class $StopCaseCopyWith<$Res>  {
$StopCaseCopyWith(StopCase _, $Res Function(StopCase) __);
}


/// Adds pattern-matching-related methods to [StopCase].
extension StopCasePatterns on StopCase {
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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( StopCase_Uncertain value)?  uncertain,TResult Function( StopCase_UncertainWrite value)?  uncertainWrite,TResult Function( StopCase_OtherFirmware value)?  otherFirmware,TResult Function( StopCase_TwoFeeders value)?  twoFeeders,TResult Function( StopCase_BackupMismatch value)?  backupMismatch,TResult Function( StopCase_NotYourFeeder value)?  notYourFeeder,TResult Function( StopCase_NoFeederCame value)?  noFeederCame,TResult Function( StopCase_EarlierRequestOpen value)?  earlierRequestOpen,TResult Function( StopCase_StockRefused value)?  stockRefused,TResult Function( StopCase_ClockUnset value)?  clockUnset,TResult Function( StopCase_NoSetupFeeder value)?  noSetupFeeder,TResult Function( StopCase_SetupFailed value)?  setupFailed,TResult Function( StopCase_NotAnswering value)?  notAnswering,TResult Function( StopCase_PlansUnreadable value)?  plansUnreadable,TResult Function( StopCase_PromotionFailed value)?  promotionFailed,TResult Function( StopCase_FellBack value)?  fellBack,TResult Function( StopCase_PlansNotApplied value)?  plansNotApplied,TResult Function( StopCase_ServerFailed value)?  serverFailed,TResult Function( StopCase_Cancelled value)?  cancelled,TResult Function( StopCase_Other value)?  other,required TResult orElse(),}){
final _that = this;
switch (_that) {
case StopCase_Uncertain() when uncertain != null:
return uncertain(_that);case StopCase_UncertainWrite() when uncertainWrite != null:
return uncertainWrite(_that);case StopCase_OtherFirmware() when otherFirmware != null:
return otherFirmware(_that);case StopCase_TwoFeeders() when twoFeeders != null:
return twoFeeders(_that);case StopCase_BackupMismatch() when backupMismatch != null:
return backupMismatch(_that);case StopCase_NotYourFeeder() when notYourFeeder != null:
return notYourFeeder(_that);case StopCase_NoFeederCame() when noFeederCame != null:
return noFeederCame(_that);case StopCase_EarlierRequestOpen() when earlierRequestOpen != null:
return earlierRequestOpen(_that);case StopCase_StockRefused() when stockRefused != null:
return stockRefused(_that);case StopCase_ClockUnset() when clockUnset != null:
return clockUnset(_that);case StopCase_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that);case StopCase_SetupFailed() when setupFailed != null:
return setupFailed(_that);case StopCase_NotAnswering() when notAnswering != null:
return notAnswering(_that);case StopCase_PlansUnreadable() when plansUnreadable != null:
return plansUnreadable(_that);case StopCase_PromotionFailed() when promotionFailed != null:
return promotionFailed(_that);case StopCase_FellBack() when fellBack != null:
return fellBack(_that);case StopCase_PlansNotApplied() when plansNotApplied != null:
return plansNotApplied(_that);case StopCase_ServerFailed() when serverFailed != null:
return serverFailed(_that);case StopCase_Cancelled() when cancelled != null:
return cancelled(_that);case StopCase_Other() when other != null:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( StopCase_Uncertain value)  uncertain,required TResult Function( StopCase_UncertainWrite value)  uncertainWrite,required TResult Function( StopCase_OtherFirmware value)  otherFirmware,required TResult Function( StopCase_TwoFeeders value)  twoFeeders,required TResult Function( StopCase_BackupMismatch value)  backupMismatch,required TResult Function( StopCase_NotYourFeeder value)  notYourFeeder,required TResult Function( StopCase_NoFeederCame value)  noFeederCame,required TResult Function( StopCase_EarlierRequestOpen value)  earlierRequestOpen,required TResult Function( StopCase_StockRefused value)  stockRefused,required TResult Function( StopCase_ClockUnset value)  clockUnset,required TResult Function( StopCase_NoSetupFeeder value)  noSetupFeeder,required TResult Function( StopCase_SetupFailed value)  setupFailed,required TResult Function( StopCase_NotAnswering value)  notAnswering,required TResult Function( StopCase_PlansUnreadable value)  plansUnreadable,required TResult Function( StopCase_PromotionFailed value)  promotionFailed,required TResult Function( StopCase_FellBack value)  fellBack,required TResult Function( StopCase_PlansNotApplied value)  plansNotApplied,required TResult Function( StopCase_ServerFailed value)  serverFailed,required TResult Function( StopCase_Cancelled value)  cancelled,required TResult Function( StopCase_Other value)  other,}){
final _that = this;
switch (_that) {
case StopCase_Uncertain():
return uncertain(_that);case StopCase_UncertainWrite():
return uncertainWrite(_that);case StopCase_OtherFirmware():
return otherFirmware(_that);case StopCase_TwoFeeders():
return twoFeeders(_that);case StopCase_BackupMismatch():
return backupMismatch(_that);case StopCase_NotYourFeeder():
return notYourFeeder(_that);case StopCase_NoFeederCame():
return noFeederCame(_that);case StopCase_EarlierRequestOpen():
return earlierRequestOpen(_that);case StopCase_StockRefused():
return stockRefused(_that);case StopCase_ClockUnset():
return clockUnset(_that);case StopCase_NoSetupFeeder():
return noSetupFeeder(_that);case StopCase_SetupFailed():
return setupFailed(_that);case StopCase_NotAnswering():
return notAnswering(_that);case StopCase_PlansUnreadable():
return plansUnreadable(_that);case StopCase_PromotionFailed():
return promotionFailed(_that);case StopCase_FellBack():
return fellBack(_that);case StopCase_PlansNotApplied():
return plansNotApplied(_that);case StopCase_ServerFailed():
return serverFailed(_that);case StopCase_Cancelled():
return cancelled(_that);case StopCase_Other():
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( StopCase_Uncertain value)?  uncertain,TResult? Function( StopCase_UncertainWrite value)?  uncertainWrite,TResult? Function( StopCase_OtherFirmware value)?  otherFirmware,TResult? Function( StopCase_TwoFeeders value)?  twoFeeders,TResult? Function( StopCase_BackupMismatch value)?  backupMismatch,TResult? Function( StopCase_NotYourFeeder value)?  notYourFeeder,TResult? Function( StopCase_NoFeederCame value)?  noFeederCame,TResult? Function( StopCase_EarlierRequestOpen value)?  earlierRequestOpen,TResult? Function( StopCase_StockRefused value)?  stockRefused,TResult? Function( StopCase_ClockUnset value)?  clockUnset,TResult? Function( StopCase_NoSetupFeeder value)?  noSetupFeeder,TResult? Function( StopCase_SetupFailed value)?  setupFailed,TResult? Function( StopCase_NotAnswering value)?  notAnswering,TResult? Function( StopCase_PlansUnreadable value)?  plansUnreadable,TResult? Function( StopCase_PromotionFailed value)?  promotionFailed,TResult? Function( StopCase_FellBack value)?  fellBack,TResult? Function( StopCase_PlansNotApplied value)?  plansNotApplied,TResult? Function( StopCase_ServerFailed value)?  serverFailed,TResult? Function( StopCase_Cancelled value)?  cancelled,TResult? Function( StopCase_Other value)?  other,}){
final _that = this;
switch (_that) {
case StopCase_Uncertain() when uncertain != null:
return uncertain(_that);case StopCase_UncertainWrite() when uncertainWrite != null:
return uncertainWrite(_that);case StopCase_OtherFirmware() when otherFirmware != null:
return otherFirmware(_that);case StopCase_TwoFeeders() when twoFeeders != null:
return twoFeeders(_that);case StopCase_BackupMismatch() when backupMismatch != null:
return backupMismatch(_that);case StopCase_NotYourFeeder() when notYourFeeder != null:
return notYourFeeder(_that);case StopCase_NoFeederCame() when noFeederCame != null:
return noFeederCame(_that);case StopCase_EarlierRequestOpen() when earlierRequestOpen != null:
return earlierRequestOpen(_that);case StopCase_StockRefused() when stockRefused != null:
return stockRefused(_that);case StopCase_ClockUnset() when clockUnset != null:
return clockUnset(_that);case StopCase_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder(_that);case StopCase_SetupFailed() when setupFailed != null:
return setupFailed(_that);case StopCase_NotAnswering() when notAnswering != null:
return notAnswering(_that);case StopCase_PlansUnreadable() when plansUnreadable != null:
return plansUnreadable(_that);case StopCase_PromotionFailed() when promotionFailed != null:
return promotionFailed(_that);case StopCase_FellBack() when fellBack != null:
return fellBack(_that);case StopCase_PlansNotApplied() when plansNotApplied != null:
return plansNotApplied(_that);case StopCase_ServerFailed() when serverFailed != null:
return serverFailed(_that);case StopCase_Cancelled() when cancelled != null:
return cancelled(_that);case StopCase_Other() when other != null:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  uncertain,TResult Function( UncertainWrite write)?  uncertainWrite,TResult Function( String version)?  otherFirmware,TResult Function()?  twoFeeders,TResult Function()?  backupMismatch,TResult Function()?  notYourFeeder,TResult Function()?  noFeederCame,TResult Function()?  earlierRequestOpen,TResult Function( int? code)?  stockRefused,TResult Function()?  clockUnset,TResult Function()?  noSetupFeeder,TResult Function( SetupRefusal? refusal)?  setupFailed,TResult Function()?  notAnswering,TResult Function()?  plansUnreadable,TResult Function()?  promotionFailed,TResult Function()?  fellBack,TResult Function()?  plansNotApplied,TResult Function()?  serverFailed,TResult Function()?  cancelled,TResult Function()?  other,required TResult orElse(),}) {final _that = this;
switch (_that) {
case StopCase_Uncertain() when uncertain != null:
return uncertain();case StopCase_UncertainWrite() when uncertainWrite != null:
return uncertainWrite(_that.write);case StopCase_OtherFirmware() when otherFirmware != null:
return otherFirmware(_that.version);case StopCase_TwoFeeders() when twoFeeders != null:
return twoFeeders();case StopCase_BackupMismatch() when backupMismatch != null:
return backupMismatch();case StopCase_NotYourFeeder() when notYourFeeder != null:
return notYourFeeder();case StopCase_NoFeederCame() when noFeederCame != null:
return noFeederCame();case StopCase_EarlierRequestOpen() when earlierRequestOpen != null:
return earlierRequestOpen();case StopCase_StockRefused() when stockRefused != null:
return stockRefused(_that.code);case StopCase_ClockUnset() when clockUnset != null:
return clockUnset();case StopCase_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder();case StopCase_SetupFailed() when setupFailed != null:
return setupFailed(_that.refusal);case StopCase_NotAnswering() when notAnswering != null:
return notAnswering();case StopCase_PlansUnreadable() when plansUnreadable != null:
return plansUnreadable();case StopCase_PromotionFailed() when promotionFailed != null:
return promotionFailed();case StopCase_FellBack() when fellBack != null:
return fellBack();case StopCase_PlansNotApplied() when plansNotApplied != null:
return plansNotApplied();case StopCase_ServerFailed() when serverFailed != null:
return serverFailed();case StopCase_Cancelled() when cancelled != null:
return cancelled();case StopCase_Other() when other != null:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  uncertain,required TResult Function( UncertainWrite write)  uncertainWrite,required TResult Function( String version)  otherFirmware,required TResult Function()  twoFeeders,required TResult Function()  backupMismatch,required TResult Function()  notYourFeeder,required TResult Function()  noFeederCame,required TResult Function()  earlierRequestOpen,required TResult Function( int? code)  stockRefused,required TResult Function()  clockUnset,required TResult Function()  noSetupFeeder,required TResult Function( SetupRefusal? refusal)  setupFailed,required TResult Function()  notAnswering,required TResult Function()  plansUnreadable,required TResult Function()  promotionFailed,required TResult Function()  fellBack,required TResult Function()  plansNotApplied,required TResult Function()  serverFailed,required TResult Function()  cancelled,required TResult Function()  other,}) {final _that = this;
switch (_that) {
case StopCase_Uncertain():
return uncertain();case StopCase_UncertainWrite():
return uncertainWrite(_that.write);case StopCase_OtherFirmware():
return otherFirmware(_that.version);case StopCase_TwoFeeders():
return twoFeeders();case StopCase_BackupMismatch():
return backupMismatch();case StopCase_NotYourFeeder():
return notYourFeeder();case StopCase_NoFeederCame():
return noFeederCame();case StopCase_EarlierRequestOpen():
return earlierRequestOpen();case StopCase_StockRefused():
return stockRefused(_that.code);case StopCase_ClockUnset():
return clockUnset();case StopCase_NoSetupFeeder():
return noSetupFeeder();case StopCase_SetupFailed():
return setupFailed(_that.refusal);case StopCase_NotAnswering():
return notAnswering();case StopCase_PlansUnreadable():
return plansUnreadable();case StopCase_PromotionFailed():
return promotionFailed();case StopCase_FellBack():
return fellBack();case StopCase_PlansNotApplied():
return plansNotApplied();case StopCase_ServerFailed():
return serverFailed();case StopCase_Cancelled():
return cancelled();case StopCase_Other():
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  uncertain,TResult? Function( UncertainWrite write)?  uncertainWrite,TResult? Function( String version)?  otherFirmware,TResult? Function()?  twoFeeders,TResult? Function()?  backupMismatch,TResult? Function()?  notYourFeeder,TResult? Function()?  noFeederCame,TResult? Function()?  earlierRequestOpen,TResult? Function( int? code)?  stockRefused,TResult? Function()?  clockUnset,TResult? Function()?  noSetupFeeder,TResult? Function( SetupRefusal? refusal)?  setupFailed,TResult? Function()?  notAnswering,TResult? Function()?  plansUnreadable,TResult? Function()?  promotionFailed,TResult? Function()?  fellBack,TResult? Function()?  plansNotApplied,TResult? Function()?  serverFailed,TResult? Function()?  cancelled,TResult? Function()?  other,}) {final _that = this;
switch (_that) {
case StopCase_Uncertain() when uncertain != null:
return uncertain();case StopCase_UncertainWrite() when uncertainWrite != null:
return uncertainWrite(_that.write);case StopCase_OtherFirmware() when otherFirmware != null:
return otherFirmware(_that.version);case StopCase_TwoFeeders() when twoFeeders != null:
return twoFeeders();case StopCase_BackupMismatch() when backupMismatch != null:
return backupMismatch();case StopCase_NotYourFeeder() when notYourFeeder != null:
return notYourFeeder();case StopCase_NoFeederCame() when noFeederCame != null:
return noFeederCame();case StopCase_EarlierRequestOpen() when earlierRequestOpen != null:
return earlierRequestOpen();case StopCase_StockRefused() when stockRefused != null:
return stockRefused(_that.code);case StopCase_ClockUnset() when clockUnset != null:
return clockUnset();case StopCase_NoSetupFeeder() when noSetupFeeder != null:
return noSetupFeeder();case StopCase_SetupFailed() when setupFailed != null:
return setupFailed(_that.refusal);case StopCase_NotAnswering() when notAnswering != null:
return notAnswering();case StopCase_PlansUnreadable() when plansUnreadable != null:
return plansUnreadable();case StopCase_PromotionFailed() when promotionFailed != null:
return promotionFailed();case StopCase_FellBack() when fellBack != null:
return fellBack();case StopCase_PlansNotApplied() when plansNotApplied != null:
return plansNotApplied();case StopCase_ServerFailed() when serverFailed != null:
return serverFailed();case StopCase_Cancelled() when cancelled != null:
return cancelled();case StopCase_Other() when other != null:
return other();case _:
  return null;

}
}

}

/// @nodoc


class StopCase_Uncertain extends StopCase {
  const StopCase_Uncertain(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_Uncertain);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.uncertain()';
}


}




/// @nodoc


class StopCase_UncertainWrite extends StopCase {
  const StopCase_UncertainWrite({required this.write}): super._();
  

 final  UncertainWrite write;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$StopCase_UncertainWriteCopyWith<StopCase_UncertainWrite> get copyWith => _$StopCase_UncertainWriteCopyWithImpl<StopCase_UncertainWrite>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_UncertainWrite&&(identical(other.write, write) || other.write == write));
}


@override
int get hashCode {
    return Object.hash(runtimeType,write);
}

@override
String toString() {
    return 'StopCase.uncertainWrite(write: $write)';
}


}

/// @nodoc
abstract mixin class $StopCase_UncertainWriteCopyWith<$Res> implements $StopCaseCopyWith<$Res> {
  factory $StopCase_UncertainWriteCopyWith(StopCase_UncertainWrite value, $Res Function(StopCase_UncertainWrite) _then) = _$StopCase_UncertainWriteCopyWithImpl;
@useResult
$Res call({
 UncertainWrite write
});




}
/// @nodoc
class _$StopCase_UncertainWriteCopyWithImpl<$Res>
    implements $StopCase_UncertainWriteCopyWith<$Res> {
  _$StopCase_UncertainWriteCopyWithImpl(this._self, this._then);

  final StopCase_UncertainWrite _self;
  final $Res Function(StopCase_UncertainWrite) _then;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? write = null,}) {
  return _then(StopCase_UncertainWrite(
write: null == write ? _self.write : write // ignore: cast_nullable_to_non_nullable
as UncertainWrite,
  ));
}


}

/// @nodoc


class StopCase_OtherFirmware extends StopCase {
  const StopCase_OtherFirmware({required this.version}): super._();
  

 final  String version;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$StopCase_OtherFirmwareCopyWith<StopCase_OtherFirmware> get copyWith => _$StopCase_OtherFirmwareCopyWithImpl<StopCase_OtherFirmware>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_OtherFirmware&&(identical(other.version, version) || other.version == version));
}


@override
int get hashCode {
    return Object.hash(runtimeType,version);
}

@override
String toString() {
    return 'StopCase.otherFirmware(version: $version)';
}


}

/// @nodoc
abstract mixin class $StopCase_OtherFirmwareCopyWith<$Res> implements $StopCaseCopyWith<$Res> {
  factory $StopCase_OtherFirmwareCopyWith(StopCase_OtherFirmware value, $Res Function(StopCase_OtherFirmware) _then) = _$StopCase_OtherFirmwareCopyWithImpl;
@useResult
$Res call({
 String version
});




}
/// @nodoc
class _$StopCase_OtherFirmwareCopyWithImpl<$Res>
    implements $StopCase_OtherFirmwareCopyWith<$Res> {
  _$StopCase_OtherFirmwareCopyWithImpl(this._self, this._then);

  final StopCase_OtherFirmware _self;
  final $Res Function(StopCase_OtherFirmware) _then;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? version = null,}) {
  return _then(StopCase_OtherFirmware(
version: null == version ? _self.version : version // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class StopCase_TwoFeeders extends StopCase {
  const StopCase_TwoFeeders(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_TwoFeeders);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.twoFeeders()';
}


}




/// @nodoc


class StopCase_BackupMismatch extends StopCase {
  const StopCase_BackupMismatch(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_BackupMismatch);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.backupMismatch()';
}


}




/// @nodoc


class StopCase_NotYourFeeder extends StopCase {
  const StopCase_NotYourFeeder(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_NotYourFeeder);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.notYourFeeder()';
}


}




/// @nodoc


class StopCase_NoFeederCame extends StopCase {
  const StopCase_NoFeederCame(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_NoFeederCame);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.noFeederCame()';
}


}




/// @nodoc


class StopCase_EarlierRequestOpen extends StopCase {
  const StopCase_EarlierRequestOpen(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_EarlierRequestOpen);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.earlierRequestOpen()';
}


}




/// @nodoc


class StopCase_StockRefused extends StopCase {
  const StopCase_StockRefused({this.code}): super._();
  

 final  int? code;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$StopCase_StockRefusedCopyWith<StopCase_StockRefused> get copyWith => _$StopCase_StockRefusedCopyWithImpl<StopCase_StockRefused>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_StockRefused&&(identical(other.code, code) || other.code == code));
}


@override
int get hashCode {
    return Object.hash(runtimeType,code);
}

@override
String toString() {
    return 'StopCase.stockRefused(code: $code)';
}


}

/// @nodoc
abstract mixin class $StopCase_StockRefusedCopyWith<$Res> implements $StopCaseCopyWith<$Res> {
  factory $StopCase_StockRefusedCopyWith(StopCase_StockRefused value, $Res Function(StopCase_StockRefused) _then) = _$StopCase_StockRefusedCopyWithImpl;
@useResult
$Res call({
 int? code
});




}
/// @nodoc
class _$StopCase_StockRefusedCopyWithImpl<$Res>
    implements $StopCase_StockRefusedCopyWith<$Res> {
  _$StopCase_StockRefusedCopyWithImpl(this._self, this._then);

  final StopCase_StockRefused _self;
  final $Res Function(StopCase_StockRefused) _then;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? code = freezed,}) {
  return _then(StopCase_StockRefused(
code: freezed == code ? _self.code : code // ignore: cast_nullable_to_non_nullable
as int?,
  ));
}


}

/// @nodoc


class StopCase_ClockUnset extends StopCase {
  const StopCase_ClockUnset(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_ClockUnset);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.clockUnset()';
}


}




/// @nodoc


class StopCase_NoSetupFeeder extends StopCase {
  const StopCase_NoSetupFeeder(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_NoSetupFeeder);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.noSetupFeeder()';
}


}




/// @nodoc


class StopCase_SetupFailed extends StopCase {
  const StopCase_SetupFailed({this.refusal}): super._();
  

 final  SetupRefusal? refusal;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$StopCase_SetupFailedCopyWith<StopCase_SetupFailed> get copyWith => _$StopCase_SetupFailedCopyWithImpl<StopCase_SetupFailed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_SetupFailed&&(identical(other.refusal, refusal) || other.refusal == refusal));
}


@override
int get hashCode {
    return Object.hash(runtimeType,refusal);
}

@override
String toString() {
    return 'StopCase.setupFailed(refusal: $refusal)';
}


}

/// @nodoc
abstract mixin class $StopCase_SetupFailedCopyWith<$Res> implements $StopCaseCopyWith<$Res> {
  factory $StopCase_SetupFailedCopyWith(StopCase_SetupFailed value, $Res Function(StopCase_SetupFailed) _then) = _$StopCase_SetupFailedCopyWithImpl;
@useResult
$Res call({
 SetupRefusal? refusal
});




}
/// @nodoc
class _$StopCase_SetupFailedCopyWithImpl<$Res>
    implements $StopCase_SetupFailedCopyWith<$Res> {
  _$StopCase_SetupFailedCopyWithImpl(this._self, this._then);

  final StopCase_SetupFailed _self;
  final $Res Function(StopCase_SetupFailed) _then;

/// Create a copy of StopCase
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? refusal = freezed,}) {
  return _then(StopCase_SetupFailed(
refusal: freezed == refusal ? _self.refusal : refusal // ignore: cast_nullable_to_non_nullable
as SetupRefusal?,
  ));
}


}

/// @nodoc


class StopCase_NotAnswering extends StopCase {
  const StopCase_NotAnswering(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_NotAnswering);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.notAnswering()';
}


}




/// @nodoc


class StopCase_PlansUnreadable extends StopCase {
  const StopCase_PlansUnreadable(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_PlansUnreadable);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.plansUnreadable()';
}


}




/// @nodoc


class StopCase_PromotionFailed extends StopCase {
  const StopCase_PromotionFailed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_PromotionFailed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.promotionFailed()';
}


}




/// @nodoc


class StopCase_FellBack extends StopCase {
  const StopCase_FellBack(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_FellBack);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.fellBack()';
}


}




/// @nodoc


class StopCase_PlansNotApplied extends StopCase {
  const StopCase_PlansNotApplied(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_PlansNotApplied);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.plansNotApplied()';
}


}




/// @nodoc


class StopCase_ServerFailed extends StopCase {
  const StopCase_ServerFailed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_ServerFailed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.serverFailed()';
}


}




/// @nodoc


class StopCase_Cancelled extends StopCase {
  const StopCase_Cancelled(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_Cancelled);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.cancelled()';
}


}




/// @nodoc


class StopCase_Other extends StopCase {
  const StopCase_Other(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is StopCase_Other);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'StopCase.other()';
}


}




// dart format on
