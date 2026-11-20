//! The two calls the agent makes, as a port: `POST /install/join` on the
//! public entry point and `POST /internal/machines/renew` on the internal
//! one (0066, steps 3 and 4). [`crate::http`] is the real one.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::cert::Credentials;

/// What both routes answer.
#[derive(Clone, Deserialize)]
pub struct Issued {
    /// The machine's certificate, PEM.
    pub certificate: String,
    /// The installation's CA, PEM.
    pub ca: String,
    /// The certificate's serial, lowercase hex.
    pub serial: String,
    /// When it ends, Unix seconds.
    pub not_after: i64,
    /// The machine's state: `pending`, `approved` or `draining`.
    pub state: String,
}

impl std::fmt::Debug for Issued {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Issued")
            .field("serial", &self.serial)
            .field("not_after", &self.not_after)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Why a call gave no certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// 401: the token is unknown, used or expired (join); the certificate
    /// is unknown, replaced, retired or of another role (renew).
    Refused,
    /// 409: another machine has the name.
    NameTaken,
    /// Another 4xx (422 a bad request, 413): asking again would not help.
    Rejected(u16),
    /// 429: try again after this long.
    TooMany(Duration),
    /// 5xx (503: no CA yet, or the header not trusted), or an answer that
    /// cannot be read: try again later.
    Unavailable(u16),
    /// No answer: the connection, TLS or a timeout. The request may have
    /// arrived, so the server may have issued a certificate that was lost.
    Network(String),
    /// The call could not be made from the files it was given (a CA or a
    /// certificate that cannot be read): asking again would not help.
    Local(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused => write!(f, "refused (401)"),
            Self::NameTaken => write!(f, "the name is taken (409)"),
            Self::Rejected(status) => write!(f, "rejected ({status})"),
            Self::TooMany(after) => write!(f, "too many attempts, {}s to wait", after.as_secs()),
            Self::Unavailable(status) => write!(f, "unavailable ({status})"),
            Self::Network(e) => write!(f, "no answer: {e}"),
            Self::Local(e) => write!(f, "the client cannot be built: {e}"),
        }
    }
}

impl CallError {
    /// Whether asking again later may succeed.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::TooMany(_) | Self::Unavailable(_) | Self::Network(_)
        )
    }
}

/// What a status other than success means; `retry_after` is the header's
/// seconds.
pub fn classify(status: u16, retry_after: Option<u64>) -> CallError {
    match status {
        401 => CallError::Refused,
        409 => CallError::NameTaken,
        429 => CallError::TooMany(Duration::from_secs(retry_after.unwrap_or(60))),
        400..=499 => CallError::Rejected(status),
        _ => CallError::Unavailable(status),
    }
}

/// The answer of a call: a certificate for a success status, else
/// [`classify`]'s.
pub fn answer(status: u16, retry_after: Option<u64>, body: &[u8]) -> Result<Issued, CallError> {
    if !(200..300).contains(&status) {
        return Err(classify(status, retry_after));
    }
    serde_json::from_slice(body).map_err(|_| CallError::Unavailable(status))
}

/// The API, as the agent calls it.
#[async_trait]
pub trait MachineApi: Send + Sync {
    /// Exchanges the join token and a request for the first certificate.
    async fn join(&self, token: &str, name: &str, csr: &str) -> Result<Issued, CallError>;

    /// A new certificate for the request, presenting `identity`, trusting
    /// `ca` for the server.
    async fn renew(&self, identity: &Credentials, ca: &str, csr: &str)
        -> Result<Issued, CallError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_status_says_what_to_do() {
        assert_eq!(classify(401, None), CallError::Refused);
        assert_eq!(classify(409, None), CallError::NameTaken);
        assert_eq!(classify(422, None), CallError::Rejected(422));
        assert_eq!(classify(413, None), CallError::Rejected(413));
        let too_many = classify(429, Some(7));
        assert_eq!(too_many, CallError::TooMany(Duration::from_secs(7)));
        assert_eq!(classify(503, None), CallError::Unavailable(503));
        assert_eq!(classify(500, None), CallError::Unavailable(500));
        let retryable: Vec<bool> = [401, 409, 422, 429, 500, 503]
            .iter()
            .map(|s| classify(*s, None).retryable())
            .collect();
        assert_eq!(retryable, [false, false, false, true, true, true]);
        assert!(CallError::Network("reset".into()).retryable());
    }

    #[test]
    fn a_success_is_read_and_never_printed_whole() {
        let body = br#"{"certificate":"-----BEGIN CERTIFICATE-----","ca":"x","serial":"0a","not_after":1,"state":"pending"}"#;
        let issued = answer(201, None, body).unwrap();
        assert_eq!(
            (issued.serial.as_str(), issued.state.as_str()),
            ("0a", "pending")
        );
        assert!(!format!("{issued:?}").contains("BEGIN"));
        assert_eq!(
            answer(200, None, b"{}").unwrap_err(),
            CallError::Unavailable(200)
        );
        assert_eq!(answer(401, None, b"{}").unwrap_err(), CallError::Refused);
    }
}
