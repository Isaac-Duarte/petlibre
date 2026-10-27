//! First-owner setup and the session.

use wellfed_api::{Credentials, SESSION_COOKIE, SetupState, User};
use reqwest::header::{self, HeaderMap};
use reqwest::{Method, StatusCode};

use crate::{Auth, Client, Error, decode, expect_status};

impl Client {
    /// Whether the server is waiting for its first owner.
    pub async fn setup_state(&self) -> Result<SetupState, Error> {
        let response = self.api(Method::GET, "/setup").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// Create the first owner, and hold their session.
    pub async fn setup(&mut self, credentials: &Credentials) -> Result<User, Error> {
        let response = self
            .api(Method::POST, "/setup")
            .json(credentials)
            .send()
            .await?;
        self.open_session(response, StatusCode::CREATED).await
    }

    /// Log in, and hold the session.
    pub async fn login(&mut self, credentials: &Credentials) -> Result<User, Error> {
        let response = self
            .api(Method::POST, "/session")
            .json(credentials)
            .send()
            .await?;
        self.open_session(response, StatusCode::OK).await
    }

    /// The logged-in user.
    pub async fn session(&self) -> Result<User, Error> {
        let response = self.api(Method::GET, "/session").send().await?;
        decode(response, StatusCode::OK).await
    }

    /// End the session the client holds, and drop it.
    pub async fn logout(&mut self) -> Result<(), Error> {
        let response = self.api(Method::DELETE, "/session").send().await?;
        expect_status(response, StatusCode::NO_CONTENT).await?;

        if matches!(self.auth, Some(Auth::Session(_))) {
            self.auth = None;
        }
        Ok(())
    }

    async fn open_session(
        &mut self,
        response: reqwest::Response,
        expected: StatusCode,
    ) -> Result<User, Error> {
        let response = expect_status(response, expected).await?;
        let session = session_cookie(response.headers()).ok_or(Error::NoSession)?;
        let user = decode(response, expected).await?;

        self.auth = Some(Auth::Session(session));
        Ok(user)
    }
}

/// The session cookie's value from a response's `Set-Cookie` headers.
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|cookie| cookie.split(';').next())
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == SESSION_COOKIE && !value.is_empty())
        .map(|(_, value)| value.to_owned())
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    #[test]
    fn the_session_is_the_named_cookie_with_a_value() {
        let mut headers = HeaderMap::new();
        for cookie in [
            "other=1; Path=/",
            "wellfed_session=; Max-Age=0",
            "wellfed_session=00ff; HttpOnly; SameSite=Strict",
        ] {
            headers.append(header::SET_COOKIE, HeaderValue::from_static(cookie));
        }
        assert_eq!(session_cookie(&headers).as_deref(), Some("00ff"));

        headers.remove(header::SET_COOKIE);
        headers.append(
            header::SET_COOKIE,
            HeaderValue::from_static("wellfed_session=; Max-Age=0"),
        );
        assert_eq!(session_cookie(&headers), None);
    }
}
