//! The request headers a write carries, as typed extractors. Each one
//! parses its header once, refuses a malformed value as `invalid`, and
//! documents itself through [`IntoParams`], so a route lists the type in
//! its `params(...)` and its OpenAPI says exactly what the header holds.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, header};
use wellfed_api::{ErrorCode, Generation};
use wellfed_service::IdempotencyKey as Key;
use utoipa::IntoParams;
use utoipa::openapi::path::{Parameter, ParameterBuilder, ParameterIn};
use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
use utoipa::openapi::{Object, Required};
use uuid::Uuid;

use crate::error::ApiError;

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// The generation an edit is based on: `If-Match: "7"` (or bare `7`). A
/// missing header is `precondition_required`, since an edit without a base
/// could overwrite a change it never saw.
pub(crate) struct IfMatch(pub(crate) Generation);

impl<S: Sync> FromRequestParts<S> for IfMatch {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        let Some(value) = parts.headers.get(header::IF_MATCH) else {
            return Err(ApiError::Code(
                ErrorCode::PreconditionRequired,
                Some("send If-Match with the generation the change is based on".into()),
            ));
        };

        value
            .to_str()
            .ok()
            .map(|text| text.trim().trim_matches('"'))
            .and_then(|text| text.parse().ok())
            .map(|generation| Self(Generation(generation)))
            .ok_or_else(|| {
                ApiError::Code(
                    ErrorCode::Invalid,
                    Some("If-Match is a generation, such as \"7\"".into()),
                )
            })
    }
}

impl IntoParams for IfMatch {
    fn into_params(_: impl Fn() -> Option<ParameterIn>) -> Vec<Parameter> {
        let schema = ObjectBuilder::new()
            .schema_type(Type::String)
            .pattern(Some(r#"^"?[0-9]+"?$"#))
            .examples(["\"7\""]);
        vec![header_param(
            "If-Match",
            Required::True,
            "The generation the change is based on",
            schema.build(),
        )]
    }
}

/// A write's `Idempotency-Key`, which the route requires: a retry under
/// the same key is answered once, so a write that must never run twice
/// cannot be sent without one.
pub(crate) struct IdempotencyKey(pub(crate) Key);

impl<S: Sync> FromRequestParts<S> for IdempotencyKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        let key = idempotency_key(&parts.headers)?.ok_or_else(|| {
            ApiError::Code(
                ErrorCode::Invalid,
                Some("this write requires an Idempotency-Key".into()),
            )
        })?;
        Ok(Self(key))
    }
}

impl IntoParams for IdempotencyKey {
    fn into_params(_: impl Fn() -> Option<ParameterIn>) -> Vec<Parameter> {
        vec![idempotency_param(Required::True)]
    }
}

/// A write's `Idempotency-Key`, when the client sent one. A write without
/// one is journalled under a key the server draws, so it cannot be
/// retried.
pub(crate) struct OptionalIdempotencyKey(pub(crate) Option<Key>);

impl<S: Sync> FromRequestParts<S> for OptionalIdempotencyKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        idempotency_key(&parts.headers).map(Self)
    }
}

impl IntoParams for OptionalIdempotencyKey {
    fn into_params(_: impl Fn() -> Option<ParameterIn>) -> Vec<Parameter> {
        vec![idempotency_param(Required::False)]
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<Option<Key>, ApiError> {
    let Some(value) = headers.get(IDEMPOTENCY_KEY) else {
        return Ok(None);
    };

    value
        .to_str()
        .ok()
        .and_then(|text| Uuid::parse_str(text.trim()).ok())
        .map(|uuid| Some(*uuid.as_bytes()))
        .ok_or_else(|| ApiError::Code(ErrorCode::Invalid, Some("Idempotency-Key is a UUID".into())))
}

fn idempotency_param(required: Required) -> Parameter {
    let schema = ObjectBuilder::new()
        .schema_type(Type::String)
        .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)));
    header_param(
        "Idempotency-Key",
        required,
        "A UUID; a retry with the same key is answered once",
        schema.build(),
    )
}

fn header_param(name: &str, required: Required, description: &str, schema: Object) -> Parameter {
    ParameterBuilder::new()
        .name(name)
        .parameter_in(ParameterIn::Header)
        .required(required)
        .description(Some(description))
        .schema(Some(schema))
        .build()
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    fn parts(headers: &[(&str, &str)]) -> Parts {
        let mut request = Request::builder();
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let (parts, ()) = request.body(()).unwrap().into_parts();
        parts
    }

    fn code(error: ApiError) -> ErrorCode {
        match error {
            ApiError::Code(code, _) => code,
            ApiError::Service(error) => panic!("unexpected service error {error:?}"),
        }
    }

    const KEY: &str = "0192c5d0-1111-7000-8000-000000000003";

    #[tokio::test]
    async fn if_match_takes_a_quoted_or_bare_generation() {
        for value in ["\"7\"", "7", " \"7\" "] {
            let IfMatch(generation) =
                IfMatch::from_request_parts(&mut parts(&[("if-match", value)]), &())
                    .await
                    .unwrap();
            assert_eq!(generation, Generation(7));
        }
    }

    #[tokio::test]
    async fn if_match_missing_or_malformed_is_refused() {
        let missing = IfMatch::from_request_parts(&mut parts(&[]), &()).await;
        assert_eq!(
            code(missing.err().unwrap()),
            ErrorCode::PreconditionRequired
        );

        for value in ["*", "\"x\"", "-1", "4294967296"] {
            let bad = IfMatch::from_request_parts(&mut parts(&[("if-match", value)]), &()).await;
            assert_eq!(code(bad.err().unwrap()), ErrorCode::Invalid);
        }
    }

    #[tokio::test]
    async fn an_optional_key_may_be_absent_but_not_malformed() {
        let absent = OptionalIdempotencyKey::from_request_parts(&mut parts(&[]), &()).await;
        assert!(absent.unwrap().0.is_none());

        let sent = OptionalIdempotencyKey::from_request_parts(
            &mut parts(&[("idempotency-key", KEY)]),
            &(),
        )
        .await
        .unwrap();
        assert_eq!(sent.0, Some(*Uuid::parse_str(KEY).unwrap().as_bytes()));

        let bad = OptionalIdempotencyKey::from_request_parts(
            &mut parts(&[("idempotency-key", "7")]),
            &(),
        )
        .await;
        assert_eq!(code(bad.err().unwrap()), ErrorCode::Invalid);
    }

    #[tokio::test]
    async fn a_required_key_must_be_sent() {
        let absent = IdempotencyKey::from_request_parts(&mut parts(&[]), &()).await;
        assert_eq!(code(absent.err().unwrap()), ErrorCode::Invalid);

        let IdempotencyKey(key) =
            IdempotencyKey::from_request_parts(&mut parts(&[("idempotency-key", KEY)]), &())
                .await
                .unwrap();
        assert_eq!(key, *Uuid::parse_str(KEY).unwrap().as_bytes());
    }

    #[test]
    fn each_extractor_documents_its_header() {
        let documented = |params: Vec<Parameter>| {
            let params = serde_json::to_value(params).unwrap();
            let param = &params[0];
            (
                param["name"].as_str().map(str::to_owned),
                param["in"].as_str().map(str::to_owned),
                param["required"].as_bool(),
            )
        };
        let header =
            |name: &str, required| (Some(name.into()), Some("header".into()), Some(required));

        assert_eq!(
            documented(IfMatch::into_params(|| None)),
            header("If-Match", true)
        );
        assert_eq!(
            documented(IdempotencyKey::into_params(|| None)),
            header("Idempotency-Key", true)
        );
        assert_eq!(
            documented(OptionalIdempotencyKey::into_params(|| None)),
            header("Idempotency-Key", false)
        );
    }
}
