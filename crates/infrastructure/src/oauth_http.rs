//! The HTTP transport seam for OAuth provider adapters.
//!
//! [`HttpClient`] is an infrastructure-internal port (not a domain port — the
//! domain knows nothing of HTTP): [`OidcProvider`](crate::oauth_provider) does
//! its token/userinfo calls through it, so its request-building and
//! response-parsing are unit-testable against a fake transport, while
//! production wires the real [`ReqwestHttpClient`].

use async_trait::async_trait;

/// A minimal HTTP response: status code and raw body text.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body, as text (JSON, for the endpoints we call).
    pub body: String,
}

impl HttpResponse {
    /// Whether the status is in the 2xx range.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A transport error (connection failure, TLS, unreadable body).
#[derive(Debug)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "http transport error: {}", self.0)
    }
}

impl std::error::Error for HttpError {}

/// Infrastructure-internal port: the small set of HTTP shapes the OAuth and
/// payment provider adapters need. `bearer` is optional on `post_form` because
/// the OIDC token endpoint takes no auth header while Stripe's API takes a
/// `Bearer` secret key on every call.
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// POST an `application/x-www-form-urlencoded` body, optionally with a
    /// `Bearer` token.
    async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
        bearer: Option<&str>,
    ) -> Result<HttpResponse, HttpError>;

    /// GET with a `Bearer` token (the userinfo endpoint).
    async fn get_bearer(&self, url: &str, bearer: &str) -> Result<HttpResponse, HttpError>;

    /// GET a binary body (a profile picture) of at most `max` bytes, with its
    /// `Content-Type`. A bigger body, or a non-2xx answer, is an error.
    async fn get_bytes(
        &self,
        _url: &str,
        _bearer: Option<&str>,
        _max: usize,
    ) -> Result<(String, Vec<u8>), HttpError> {
        Err(HttpError("binary GET not supported".into()))
    }
}

/// A [`HttpClient`] backed by `reqwest` (rustls). The production transport.
#[derive(Debug, Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHttpClient {
    /// Build the client with sensible timeouts.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("building a reqwest client with default TLS cannot fail");
        Self { client }
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
        bearer: Option<&str>,
    ) -> Result<HttpResponse, HttpError> {
        let mut request = self.client.post(url).form(form);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().await.map_err(|e| HttpError(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| HttpError(e.to_string()))?;
        Ok(HttpResponse { status, body })
    }

    async fn get_bearer(&self, url: &str, bearer: &str) -> Result<HttpResponse, HttpError> {
        let response = self
            .client
            .get(url)
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| HttpError(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| HttpError(e.to_string()))?;
        Ok(HttpResponse { status, body })
    }

    async fn get_bytes(
        &self,
        url: &str,
        bearer: Option<&str>,
        max: usize,
    ) -> Result<(String, Vec<u8>), HttpError> {
        let mut request = self.client.get(url);
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let mut response = request.send().await.map_err(|e| HttpError(e.to_string()))?;
        if !response.status().is_success() {
            return Err(HttpError(format!("answered {}", response.status())));
        }
        if response.content_length().is_some_and(|n| n > max as u64) {
            return Err(HttpError(format!("bigger than {max} bytes")));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        // Read in parts: without Content-Length no more than `max` is
        // downloaded.
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| HttpError(e.to_string()))?
        {
            if body.len() + chunk.len() > max {
                return Err(HttpError(format!("bigger than {max} bytes")));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((content_type, body))
    }
}
