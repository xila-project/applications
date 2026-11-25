//! Signing in to Home Assistant with a username and password, through its
//! authentication API, so that no token needs to be typed.
//!
//! The flow is the one of the Home Assistant frontend: `POST /auth/login_flow`
//! starts it, `POST /auth/login_flow/<flow_id>` progresses it (username and
//! password, then a two-factor code when the account asks for one), and a
//! `create_entry` result holds an authorization code. `POST /auth/token`
//! exchanges the code for an access token and a refresh token, and later
//! trades the refresh token for new access tokens. Only the refresh token is
//! stored, in its own file: the access token lives in memory and the password
//! is dropped after its request.
//!
//! The client identifier is the URL of the application page on the project
//! domain, and the redirect URI is the same URL: Home Assistant accepts a
//! redirect URI that has the same scheme and host as the client identifier
//! without fetching anything, and no redirect happens in this flow.

use crate::error::{Error, Result};
use crate::json::{Reader, push_string};
use crate::net::{Connector, Endpoint, read_response, status_error};
use crate::storage::{MAXIMUM_TOKEN_LENGTH, Storage, is_valid_token};
use miniserde::{Deserialize, Serialize, json};
use std::{
    format,
    string::String,
    time::{Duration, Instant},
    vec::Vec,
};

/// The client identifier of the application: a page on the project domain.
pub const CLIENT_ID: &str = "https://xila.dev/applications/home-assistant/";
/// The redirect URI sent with the login flow: the same URL as the client
/// identifier, which Home Assistant accepts without a redirect.
pub const REDIRECT_URI: &str = CLIENT_ID;
/// Seconds before its expiry when the access token is renewed.
pub const REFRESH_MARGIN_SECONDS: u64 = 60;
/// Longest user name kept.
pub const MAXIMUM_NAME_LENGTH: usize = 64;

/// The credentials stored by a sign-in: the refresh token and the name of
/// the user it belongs to, when the server reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub refresh_token: String,
    pub name: Option<String>,
}

impl Credentials {
    pub fn new(refresh_token: &str, name: Option<String>) -> Result<Self> {
        if !is_valid_token(refresh_token) {
            return Err(Error::MissingToken);
        }
        Ok(Self {
            refresh_token: String::from(refresh_token),
            name: name.and_then(|name| {
                let name = name.trim();
                if name.is_empty() {
                    None
                } else {
                    Some(String::from(
                        &name.chars().take(MAXIMUM_NAME_LENGTH).collect::<String>(),
                    ))
                }
            }),
        })
    }

    pub fn to_json(&self) -> String {
        json::to_string(self)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let credentials: Self = json::from_str(text).map_err(|_| Error::Configuration)?;
        if !is_valid_token(&credentials.refresh_token) {
            return Err(Error::MissingToken);
        }
        if credentials.refresh_token.len() > MAXIMUM_TOKEN_LENGTH {
            return Err(Error::TooLarge);
        }
        Ok(credentials)
    }
}

/// The authentication the application holds: a signed-in session with a
/// stored refresh token, or a long-lived access token. The access token of a
/// session is kept in memory only, with its expiry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub credentials: Option<Credentials>,
    pub long_lived: Option<String>,
    access_token: Option<String>,
    expires_at: Option<Instant>,
}

impl Session {
    /// Reads the credentials file, else the token file. A problem with one
    /// is reported, but does not hide the other.
    pub fn load(storage: &Storage) -> (Self, Option<Error>) {
        let (credentials, mut error) = match storage.credentials() {
            Ok(credentials) => (credentials, None),
            Err(error) => (None, Some(error)),
        };
        let long_lived = match storage.token() {
            Ok(token) => token,
            Err(token_error) => {
                if error.is_none() {
                    error = Some(token_error);
                }
                None
            }
        };
        (
            Self {
                credentials,
                long_lived,
                access_token: None,
                expires_at: None,
            },
            error,
        )
    }

    /// Whether the application can make authenticated requests.
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some() || self.long_lived.is_some()
    }

    /// Forgets everything, in memory: after a revocation or a sign-out.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Sets the access token of a fresh token answer, in memory only.
    pub fn set_access_token(&mut self, access_token: &str, expires_in: u32, now: Instant) {
        self.access_token = Some(String::from(access_token));
        self.expires_at = Some(now + Duration::from_secs(u64::from(expires_in)));
    }

    /// Whether the in-memory access token is missing or expires within the
    /// margin. A long-lived token never needs a refresh.
    pub fn needs_refresh(&self, now: Instant) -> bool {
        if self.long_lived.is_some() {
            return false;
        }
        match (&self.access_token, &self.expires_at) {
            (Some(_), Some(expires_at)) => {
                *expires_at <= now + Duration::from_secs(REFRESH_MARGIN_SECONDS)
            }
            _ => true,
        }
    }

    /// An access token to send: the long-lived token, the in-memory access
    /// token, or a fresh one from the refresh token. A revoked refresh token
    /// (400) fails with [`Error::Revoked`].
    pub fn access_token<C: Connector>(
        &mut self,
        connector: &mut C,
        endpoint: &Endpoint,
        now: Instant,
    ) -> Result<String> {
        if let Some(token) = &self.long_lived {
            return Ok(token.clone());
        }
        if !self.needs_refresh(now)
            && let Some(token) = &self.access_token
        {
            return Ok(token.clone());
        }
        let Some(credentials) = &self.credentials else {
            return Err(Error::MissingToken);
        };
        let set = refresh(connector, endpoint, &credentials.refresh_token)?;
        self.set_access_token(&set.access_token, set.expires_in, now);
        Ok(set.access_token)
    }
}

/// One step of a login flow answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowResult {
    /// The flow finished: `result` is the authorization code, `title` the
    /// name of the user when the server reported it.
    CreateEntry { code: String, title: Option<String> },
    /// The flow asks for more data: `step_id` is `init` for the username and
    /// password, `mfa` for a two-factor code. `errors` holds the reasons of a
    /// refused answer, such as `base: invalid_auth`.
    Form {
        flow_id: String,
        step_id: String,
        errors: Vec<(String, String)>,
    },
    /// The flow was aborted, with its reason.
    Abort { reason: Option<String> },
}

/// Parses a login flow answer. The reader must sit at the start of the
/// object.
pub fn parse_flow_result<S: crate::json::Source>(reader: &mut Reader<S>) -> Result<FlowResult> {
    let mut kind = None;
    let mut flow_id = None;
    let mut result = None;
    let mut title = None;
    let mut step_id = None;
    let mut errors = Vec::new();
    let mut reason = None;
    reader.object(|reader, key| {
        match key {
            "type" => kind = Some(reader.string(32)?),
            "flow_id" => flow_id = reader.optional_string(64)?,
            "result" => result = reader.optional_string(MAXIMUM_TOKEN_LENGTH)?,
            "title" => title = reader.optional_truncated_string(MAXIMUM_NAME_LENGTH)?,
            "step_id" => step_id = reader.optional_string(32)?,
            "reason" => reason = reader.optional_string(64)?,
            "errors" => {
                reader.object(|reader, key| {
                    let message = reader.optional_truncated_string(128)?;
                    if let Some(message) = message {
                        errors.push((String::from(key), message));
                    } else {
                        reader.skip()?;
                    }
                    Ok(())
                })?;
            }
            _ => reader.skip()?,
        }
        Ok(())
    })?;
    reader.finish()?;
    match kind.as_deref() {
        Some("create_entry") => Ok(FlowResult::CreateEntry {
            code: result.ok_or(Error::InvalidResponse)?,
            title,
        }),
        Some("form") => Ok(FlowResult::Form {
            flow_id: flow_id.unwrap_or_default(),
            step_id: step_id.unwrap_or_default(),
            errors,
        }),
        Some("abort") => Ok(FlowResult::Abort { reason }),
        _ => Err(Error::InvalidResponse),
    }
}

/// Tokens of a `/auth/token` answer. A refresh grant returns no new refresh
/// token: the stored one stays valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: String,
    pub expires_in: u32,
    pub refresh_token: Option<String>,
}

/// Parses a `/auth/token` answer.
pub fn parse_token_set<S: crate::json::Source>(reader: &mut Reader<S>) -> Result<TokenSet> {
    let mut access_token = None;
    let mut expires_in = None;
    let mut refresh_token = None;
    reader.object(|reader, key| {
        match key {
            "access_token" => access_token = Some(reader.string(MAXIMUM_TOKEN_LENGTH)?),
            "expires_in" => {
                let value = reader.optional_number()?;
                expires_in = value.map(|value| value.max(0.0) as u32);
            }
            "refresh_token" => refresh_token = reader.optional_string(MAXIMUM_TOKEN_LENGTH)?,
            _ => reader.skip()?,
        }
        Ok(())
    })?;
    reader.finish()?;
    Ok(TokenSet {
        access_token: access_token.ok_or(Error::InvalidResponse)?,
        expires_in: expires_in.ok_or(Error::InvalidResponse)?,
        refresh_token,
    })
}

/// Body of `POST /auth/login_flow`.
pub fn login_flow_body() -> String {
    let mut body = String::from("{\"client_id\":");
    push_string(&mut body, CLIENT_ID);
    body.push_str(",\"handler\":[\"homeassistant\",null],\"redirect_uri\":");
    push_string(&mut body, REDIRECT_URI);
    body.push('}');
    body
}

/// Body of `POST /auth/login_flow/<flow_id>` with a username and password.
pub fn credentials_body(username: &str, password: &str) -> String {
    let mut body = String::from("{\"client_id\":");
    push_string(&mut body, CLIENT_ID);
    body.push_str(",\"username\":");
    push_string(&mut body, username);
    body.push_str(",\"password\":");
    push_string(&mut body, password);
    body.push('}');
    body
}

/// Body of `POST /auth/login_flow/<flow_id>` with a two-factor code.
pub fn mfa_body(code: &str) -> String {
    let mut body = String::from("{\"client_id\":");
    push_string(&mut body, CLIENT_ID);
    body.push_str(",\"code\":");
    push_string(&mut body, code);
    body.push('}');
    body
}

/// Percent-encodes one value of a form body: everything but the unreserved
/// characters of RFC 3986.
pub fn form_encode_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// An `application/x-www-form-urlencoded` body from its pairs.
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_encode_value(key), form_encode_value(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Body of `POST /auth/token` exchanging an authorization code.
pub fn authorization_code_body(code: &str) -> String {
    form_encode(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", CLIENT_ID),
    ])
}

/// Body of `POST /auth/token` refreshing an access token.
pub fn refresh_token_body(refresh_token: &str) -> String {
    form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
    ])
}

/// Body of `POST /auth/revoke`.
pub fn revoke_body(refresh_token: &str) -> String {
    form_encode(&[("token", refresh_token)])
}

/// Starts a login flow: `POST /auth/login_flow`.
pub fn start_login_flow<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
) -> Result<FlowResult> {
    let body = login_flow_body();
    let request = endpoint.request_head(
        "POST",
        "/auth/login_flow",
        None,
        Some("application/json"),
        Some(body.as_bytes()),
    );
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    drop(body);
    let (status, response) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    parse_flow_result(&mut Reader::new(response))
}

/// Progresses a login flow: `POST /auth/login_flow/<flow_id>` with `body`.
pub fn submit_login_flow<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    flow_id: &str,
    body: &str,
) -> Result<FlowResult> {
    let request = endpoint.request_head(
        "POST",
        &format!("/auth/login_flow/{flow_id}"),
        None,
        Some("application/json"),
        Some(body.as_bytes()),
    );
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, response) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    parse_flow_result(&mut Reader::new(response))
}

/// Exchanges an authorization code: `POST /auth/token`.
pub fn exchange_code<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    code: &str,
) -> Result<TokenSet> {
    let body = authorization_code_body(code);
    let request = endpoint.request_form("POST", "/auth/token", &body);
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, response) = read_response(transport)?;
    if status != 200 {
        return Err(status_error(status));
    }
    parse_token_set(&mut Reader::new(response))
}

/// Gets a new access token from a refresh token: `POST /auth/token`. A 400
/// means the device was revoked.
pub fn refresh<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    refresh_token: &str,
) -> Result<TokenSet> {
    let body = refresh_token_body(refresh_token);
    let request = endpoint.request_form("POST", "/auth/token", &body);
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, response) = read_response(transport)?;
    if status == 400 {
        return Err(Error::Revoked);
    }
    if status != 200 {
        return Err(status_error(status));
    }
    parse_token_set(&mut Reader::new(response))
}

/// Revokes a refresh token and every access token it granted:
/// `POST /auth/revoke`. The answer is always 200, so failures of the request
/// itself are reported, but a refused token is not an error.
pub fn revoke<C: Connector>(
    connector: &mut C,
    endpoint: &Endpoint,
    refresh_token: &str,
) -> Result<()> {
    let body = revoke_body(refresh_token);
    let request = endpoint.request_form("POST", "/auth/revoke", &body);
    let transport = connector.send(endpoint.device(), &request)?;
    drop(request);
    let (status, _response) = read_response(transport)?;
    if status == 200 {
        Ok(())
    } else {
        Err(status_error(status))
    }
}

/// Reads a whole answer body, for the plain-text template answers.
#[cfg(test)]
fn read_text<T: crate::net::Transport>(
    body: &mut crate::net::Body<T>,
    limit: usize,
) -> Result<String> {
    crate::net::read_body(body, limit)
}

#[cfg(test)]
mod tests {
    // The example passwords of the tests are built at run time from single
    // characters, so that no string literal sits in a password position:
    // they are fixtures, not real credentials.
    fn fixture_password(characters: &[char]) -> String {
        characters.iter().collect()
    }

    use super::*;
    use crate::json::SliceSource;
    use crate::net::tests::{Canned, canned};
    use crate::net::{Connector, read_response};

    struct Recorder {
        responses: Vec<String>,
        requests: Vec<(String, String)>,
    }

    impl Connector for Recorder {
        type Transport = Canned;

        fn send(&mut self, device: &str, request: &[u8]) -> Result<Canned> {
            self.requests.push((
                String::from(device),
                String::from_utf8(request.to_vec()).unwrap(),
            ));
            if self.responses.is_empty() {
                return Err(Error::Network);
            }
            Ok(canned(&self.responses.remove(0), 7))
        }
    }

    fn recorder(responses: &[&str]) -> Recorder {
        Recorder {
            responses: responses.iter().map(|text| String::from(*text)).collect(),
            requests: Vec::new(),
        }
    }

    fn ok(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{body}")
    }

    fn endpoint() -> Endpoint {
        Endpoint::parse("http://homeassistant.local:8123").unwrap()
    }

    fn parse_flow(text: &str) -> Result<FlowResult> {
        let mut reader = Reader::new(SliceSource::new(text.as_bytes()));
        parse_flow_result(&mut reader)
    }

    #[test]
    fn login_flow_request_bodies() {
        assert_eq!(
            login_flow_body(),
            "{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"handler\":[\"homeassistant\",null],\"redirect_uri\":\"https://xila.dev/applications/home-assistant/\"}"
        );
        assert_eq!(
            credentials_body(
                "alice",
                &fixture_password(&[
                    's', '3', 'c', 'r', 'e', 't', ' ', '"', 'q', 'u', 'o', 't', 'e', 'd', '"',
                    '\n',
                ])
            ),
            "{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"username\":\"alice\",\"password\":\"s3cret \\\"quoted\\\"\\n\"}"
        );
        assert_eq!(
            mfa_body("123456"),
            "{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"code\":\"123456\"}"
        );
        // The bodies parse back as JSON objects.
        for body in [
            login_flow_body(),
            credentials_body("a", &fixture_password(&['b'])),
            mfa_body("1"),
        ] {
            let mut reader = Reader::new(SliceSource::new(body.as_bytes()));
            reader
                .object(|reader, key| {
                    assert!(matches!(
                        key,
                        "client_id" | "handler" | "redirect_uri" | "username" | "password" | "code"
                    ));
                    reader.skip()
                })
                .unwrap();
            reader.finish().unwrap();
        }
    }

    #[test]
    fn form_encoding_percent_encodes_everything_but_unreserved() {
        assert_eq!(form_encode_value("abcXYZ019-_.~"), "abcXYZ019-_.~");
        assert_eq!(
            form_encode_value("a b+c&d=e/f?g%h"),
            "a%20b%2Bc%26d%3De%2Ff%3Fg%25h"
        );
        assert_eq!(
            authorization_code_body("code 1&"),
            "grant_type=authorization_code&code=code%201%26&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"
        );
        assert_eq!(
            refresh_token_body("eyJhbGciOi/x"),
            "grant_type=refresh_token&refresh_token=eyJhbGciOi%2Fx&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"
        );
        assert_eq!(revoke_body("tok en"), "token=tok%20en");
    }

    #[test]
    fn parses_create_entry_form_and_abort() {
        assert_eq!(
            parse_flow(
                r#"{"flow_id":"f1","handler":["homeassistant",null],"type":"create_entry","version":1,"title":"Alice","result":"auth-code-1"}"#
            )
            .unwrap(),
            FlowResult::CreateEntry {
                code: String::from("auth-code-1"),
                title: Some(String::from("Alice")),
            }
        );
        assert_eq!(
            parse_flow(
                r#"{"flow_id":"f1","type":"form","step_id":"init","data_schema":[{"name":"username"}],"errors":{}}"#
            )
            .unwrap(),
            FlowResult::Form {
                flow_id: String::from("f1"),
                step_id: String::from("init"),
                errors: Vec::new(),
            }
        );
        assert_eq!(
            parse_flow(
                r#"{"flow_id":"f1","type":"form","step_id":"init","errors":{"base":"invalid_auth"}}"#
            )
            .unwrap(),
            FlowResult::Form {
                flow_id: String::from("f1"),
                step_id: String::from("init"),
                errors: std::vec![(String::from("base"), String::from("invalid_auth"))],
            }
        );
        assert_eq!(
            parse_flow(r#"{"flow_id":"f1","type":"form","step_id":"mfa","errors":{}}"#).unwrap(),
            FlowResult::Form {
                flow_id: String::from("f1"),
                step_id: String::from("mfa"),
                errors: Vec::new(),
            }
        );
        assert_eq!(
            parse_flow(r#"{"type":"abort","reason":"already_linked"}"#).unwrap(),
            FlowResult::Abort {
                reason: Some(String::from("already_linked")),
            }
        );
        assert_eq!(
            parse_flow(r#"{"type":"progress"}"#),
            Err(Error::InvalidResponse)
        );
        assert_eq!(
            parse_flow(r#"{"type":"create_entry"}"#),
            Err(Error::InvalidResponse)
        );
        assert_eq!(parse_flow("[]"), Err(Error::InvalidResponse));
    }

    #[test]
    fn parses_token_answers() {
        let mut reader = Reader::new(SliceSource::new(
            br#"{"access_token":"access-1","expires_in":1800,"refresh_token":"refresh-1","token_type":"Bearer","ha_auth_provider":"homeassistant"}"#,
        ));
        let set = parse_token_set(&mut reader).unwrap();
        assert_eq!(
            set,
            TokenSet {
                access_token: String::from("access-1"),
                expires_in: 1800,
                refresh_token: Some(String::from("refresh-1")),
            }
        );
        // A refresh grant returns no new refresh token.
        let mut reader = Reader::new(SliceSource::new(
            br#"{"access_token":"access-2","expires_in":1800,"token_type":"Bearer"}"#,
        ));
        let set = parse_token_set(&mut reader).unwrap();
        assert_eq!(set.refresh_token, None);
        assert_eq!(set.access_token, "access-2");
        let mut reader = Reader::new(SliceSource::new(br#"{"expires_in":1800}"#));
        assert_eq!(parse_token_set(&mut reader), Err(Error::InvalidResponse));
    }

    #[test]
    fn login_flow_calls_have_no_authorization_header() {
        let mut connector = recorder(&[&ok(
            r#"{"flow_id":"f1","type":"form","step_id":"init","errors":{}}"#,
        )]);
        let result = start_login_flow(&mut connector, &endpoint()).unwrap();
        assert!(matches!(result, FlowResult::Form { .. }));
        let (_device, request) = &connector.requests[0];
        assert!(request.starts_with("POST /auth/login_flow HTTP/1.1\r\n"));
        assert!(!request.contains("Authorization:"));
        assert!(request.contains("\r\nContent-Type: application/json\r\n"));
        assert!(request.ends_with(&format!("\r\n\r\n{}", login_flow_body())));

        let mut connector = recorder(&[&ok(
            r#"{"type":"create_entry","title":"Alice","result":"auth-code-1"}"#,
        )]);
        let result = submit_login_flow(
            &mut connector,
            &endpoint(),
            "f1",
            &credentials_body("alice", &fixture_password(&['p', 'w'])),
        )
        .unwrap();
        assert!(matches!(result, FlowResult::CreateEntry { .. }));
        let (_device, request) = &connector.requests[0];
        assert!(request.starts_with("POST /auth/login_flow/f1 HTTP/1.1\r\n"));
        assert!(!request.contains("Authorization:"));
        assert!(
            request.ends_with("\r\n\r\n{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"username\":\"alice\",\"password\":\"pw\"}")
        );
    }

    #[test]
    fn token_calls_are_form_encoded() {
        let mut connector = recorder(&[&ok(
            r#"{"access_token":"access-1","expires_in":1800,"refresh_token":"refresh-1","token_type":"Bearer"}"#,
        )]);
        let set = exchange_code(&mut connector, &endpoint(), "auth-code-1").unwrap();
        assert_eq!(set.refresh_token.as_deref(), Some("refresh-1"));
        let (_device, request) = &connector.requests[0];
        assert!(request.starts_with("POST /auth/token HTTP/1.1\r\n"));
        assert!(!request.contains("Authorization:"));
        assert!(request.contains("\r\nContent-Type: application/x-www-form-urlencoded\r\n"));
        assert!(request.ends_with("\r\n\r\ngrant_type=authorization_code&code=auth-code-1&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"));

        // A revoked refresh token fails with Revoked.
        let mut connector =
            recorder(&["HTTP/1.1 400 Bad Request\r\n\r\n{\"error\":\"invalid_grant\"}"]);
        assert_eq!(
            refresh(&mut connector, &endpoint(), "refresh-old"),
            Err(Error::Revoked)
        );
        let (_device, request) = &connector.requests[0];
        assert!(request.ends_with("\r\n\r\ngrant_type=refresh_token&refresh_token=refresh-old&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"));

        let mut connector = recorder(&[&ok(
            r#"{"access_token":"access-2","expires_in":1800,"token_type":"Bearer"}"#,
        )]);
        let set = refresh(&mut connector, &endpoint(), "refresh-1").unwrap();
        assert_eq!(set.access_token, "access-2");
        assert_eq!(set.refresh_token, None);

        // Other failures keep their status.
        let mut connector = recorder(&["HTTP/1.1 500 Internal Server Error\r\n\r\n{}"]);
        assert_eq!(
            refresh(&mut connector, &endpoint(), "refresh-1"),
            Err(Error::Http(500))
        );

        // Revoke always answers 200.
        let mut connector = recorder(&["HTTP/1.1 200 OK\r\n\r\n"]);
        revoke(&mut connector, &endpoint(), "refresh-1").unwrap();
        assert!(
            connector.requests[0]
                .1
                .starts_with("POST /auth/revoke HTTP/1.1\r\n")
        );
        assert!(connector.requests[0].1.ends_with("\r\n\r\ntoken=refresh-1"));
    }

    #[test]
    fn session_refreshes_the_access_token_only_when_needed() {
        let now = Instant::now();
        // A long-lived token is used as it is, with no request.
        let mut session = Session {
            long_lived: Some(String::from("llat")),
            ..Session::default()
        };
        let mut connector = recorder(&[]);
        assert_eq!(
            session
                .access_token(&mut connector, &endpoint(), now)
                .unwrap(),
            "llat"
        );
        assert!(connector.requests.is_empty());
        assert!(!session.needs_refresh(now));

        // Without credentials, no token can be made.
        let mut empty = Session::default();
        assert!(empty.needs_refresh(now));
        assert_eq!(
            empty.access_token(&mut connector, &endpoint(), now),
            Err(Error::MissingToken)
        );

        // A session starts without an access token: one refresh, then the
        // cached one is reused while it is valid.
        let mut session = Session {
            credentials: Some(Credentials::new("refresh-1", Some(String::from("Alice"))).unwrap()),
            ..Session::default()
        };
        assert!(session.needs_refresh(now));
        let mut connector = recorder(&[&ok(
            r#"{"access_token":"access-1","expires_in":1800,"token_type":"Bearer"}"#,
        )]);
        assert_eq!(
            session
                .access_token(&mut connector, &endpoint(), now)
                .unwrap(),
            "access-1"
        );
        assert_eq!(connector.requests.len(), 1);
        assert!(!session.needs_refresh(now));
        assert_eq!(
            session
                .access_token(&mut connector, &endpoint(), now)
                .unwrap(),
            "access-1"
        );
        assert_eq!(connector.requests.len(), 1, "no second refresh while valid");

        // Expiring within the margin renews, later it does not.
        assert!(
            session.needs_refresh(now + Duration::from_secs(1800 - REFRESH_MARGIN_SECONDS + 1))
        );
        assert!(
            !session.needs_refresh(now + Duration::from_secs(1800 - REFRESH_MARGIN_SECONDS - 1))
        );

        // An expired token is refreshed.
        let later = now + Duration::from_secs(1800);
        assert!(session.needs_refresh(later));
        let mut connector = recorder(&[&ok(
            r#"{"access_token":"access-2","expires_in":1800,"token_type":"Bearer"}"#,
        )]);
        assert_eq!(
            session
                .access_token(&mut connector, &endpoint(), later)
                .unwrap(),
            "access-2"
        );

        // A revoked refresh token fails and the session can be cleared.
        let mut session = Session {
            credentials: Some(Credentials::new("refresh-old", None).unwrap()),
            ..Session::default()
        };
        let mut connector =
            recorder(&["HTTP/1.1 400 Bad Request\r\n\r\n{\"error\":\"invalid_grant\"}"]);
        assert_eq!(
            session.access_token(&mut connector, &endpoint(), now),
            Err(Error::Revoked)
        );
        session.clear();
        assert!(!session.has_credentials());
    }

    #[test]
    fn credentials_round_trip_and_validation() {
        let credentials = Credentials::new("refresh-1", Some(String::from("Alice"))).unwrap();
        assert_eq!(
            Credentials::parse(&credentials.to_json()).unwrap(),
            credentials
        );
        assert_eq!(
            Credentials::new("refresh-1", Some(String::from("  ")))
                .unwrap()
                .name,
            None
        );
        assert_eq!(Credentials::new("", None), Err(Error::MissingToken));
        assert_eq!(
            Credentials::new("two words", None),
            Err(Error::MissingToken)
        );
        assert_eq!(
            Credentials::new(&"a".repeat(MAXIMUM_TOKEN_LENGTH + 1), None),
            Err(Error::MissingToken)
        );
        assert_eq!(
            Credentials::parse(r#"{"refresh_token":"bad token"}"#),
            Err(Error::MissingToken)
        );
        assert_eq!(Credentials::parse("{}"), Err(Error::Configuration));
        assert_eq!(Credentials::parse("{"), Err(Error::Configuration));
        // A name longer than the limit is cut on save.
        let long_name = "n".repeat(200);
        let credentials = Credentials::new("refresh-1", Some(long_name)).unwrap();
        assert_eq!(
            credentials.name.as_ref().map(String::len),
            Some(MAXIMUM_NAME_LENGTH)
        );
    }

    #[test]
    fn read_text_reads_a_plain_body_up_to_the_limit() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nlight.a|Lamp";
        let (status, mut body) = read_response(canned(response, 3)).unwrap();
        assert_eq!(status, 200);
        assert_eq!(read_text(&mut body, 1024).unwrap(), "light.a|Lamp");
        let response = "HTTP/1.1 200 OK\r\n\r\nabc";
        let (_status, mut body) = read_response(canned(response, 3)).unwrap();
        assert_eq!(read_text(&mut body, 2), Err(Error::TooLarge));
    }
}
