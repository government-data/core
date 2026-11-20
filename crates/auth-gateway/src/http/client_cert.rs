//! The client certificate a machine presented on Traefik's internal entry
//! point (decision 0066, point 5), as an extractor: [`MachineCertificate`].
//!
//! Traefik verifies the certificate against the installation's CA and writes
//! it in `X-Forwarded-Tls-Client-Cert` (`passTLSClientCert` with `pem: true`):
//! URL-encoded, the base64 DER of every certificate the client sent, comma
//! separated, the verified one first. Only that first entry is believed: what
//! follows it was sent by the client and verified by no one. Traefik removes
//! the header when a client sends it, but the API cannot tell a request that
//! came through Traefik from one that did not, so the header is read only when
//! the deployment says it can be trusted ([`ClientCertificates::trusting`],
//! `INTERNAL_CLIENT_CERT_HEADER=trusted`): where the API cannot be reached but
//! through Traefik. Otherwise it is never read and the extractor refuses with
//! 503. Neither the header nor the certificate is logged.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use http_kit::error::ApiError;

use crate::{CertificateReader, MachineCertificate};

/// The header Traefik writes the client's certificates in.
pub const CLIENT_CERT_HEADER: &str = "x-forwarded-tls-client-cert";

/// Whether, and how, the client certificate header is read: put in the
/// request's extensions by the routes that need a certificate.
#[derive(Clone)]
pub struct ClientCertificates(Option<Arc<dyn CertificateReader>>);

impl ClientCertificates {
    /// The header is Traefik's: the API is reachable only through it.
    pub fn trusting(reader: Arc<dyn CertificateReader>) -> Self {
        Self(Some(reader))
    }

    /// The header is never read.
    pub fn not_trusted() -> Self {
        Self(None)
    }

    pub fn is_trusted(&self) -> bool {
        self.0.is_some()
    }
}

impl std::fmt::Debug for ClientCertificates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ClientCertificates")
            .field(&self.is_trusted())
            .finish()
    }
}

/// Why no certificate was taken from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCertificateError {
    /// The deployment does not trust the header: it was not read.
    NotTrusted,
    /// No header, or more than one.
    Missing,
    /// Its first entry is not a certificate.
    Unreadable,
}

impl IntoResponse for ClientCertificateError {
    fn into_response(self) -> Response {
        match self {
            Self::NotTrusted => ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "client_certificate_not_trusted",
                texts::tr!("api.machines.certificateNotTrusted"),
            ),
            Self::Missing | Self::Unreadable => certificate_refused(),
        }
        .into_response()
    }
}

/// The one answer to a machine whose certificate is missing, unknown,
/// replaced or retired.
pub fn certificate_refused() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "certificate_refused",
        texts::tr!("api.machines.certificateRefused"),
    )
}

impl<S: Send + Sync> FromRequestParts<S> for MachineCertificate {
    type Rejection = ClientCertificateError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<ClientCertificates>()
            .ok_or(ClientCertificateError::NotTrusted)?
            .read(&parts.headers)
    }
}

impl ClientCertificates {
    /// The certificate of a request's `headers`, as the extractor reads it:
    /// for a route that is not given the request's parts (a module's, through
    /// a port the server implements).
    pub fn read(&self, headers: &HeaderMap) -> Result<MachineCertificate, ClientCertificateError> {
        let reader = self.0.as_ref().ok_or(ClientCertificateError::NotTrusted)?;
        let mut values = headers.get_all(CLIENT_CERT_HEADER).iter();
        let (Some(value), None) = (values.next(), values.next()) else {
            return Err(ClientCertificateError::Missing);
        };
        let der = value
            .to_str()
            .ok()
            .and_then(first_certificate)
            .ok_or(ClientCertificateError::Unreadable)?;
        reader.read(&der).ok_or(ClientCertificateError::Unreadable)
    }
}

/// The DER of the first certificate in the header's value: URL-decoded,
/// up to the first comma, base64.
fn first_certificate(value: &str) -> Option<Vec<u8>> {
    let decoded = percent_decode(value.trim())?;
    let first = decoded.split(',').next()?.trim();
    if first.is_empty() {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(first).ok()
}

/// `%XX` to its byte; anything else as it is. `None` for a `%` without two
/// hex digits after it, or a result that is not UTF-8.
pub(super) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use axum::http::Request;

    use super::*;

    /// "Reads" a certificate whose DER is `cert:<OU>:<serial>`.
    struct Fake;

    impl CertificateReader for Fake {
        fn read(&self, der: &[u8]) -> Option<MachineCertificate> {
            let text = std::str::from_utf8(der).ok()?;
            let mut parts = text.strip_prefix("cert:")?.split(':');
            let (unit, serial) = (parts.next()?, parts.next()?);
            Some(MachineCertificate {
                organization: Some("qd".into()),
                unit: Some(unit.into()),
                common_name: Some("worker-01".into()),
                serial: serial.into(),
                not_after: SystemTime::UNIX_EPOCH,
            })
        }
    }

    /// As Traefik writes it: base64 of each DER, comma separated, URL-encoded.
    fn header(ders: &[&str]) -> String {
        let b64 = |d: &&str| base64::engine::general_purpose::STANDARD.encode(d.as_bytes());
        let joined = ders.iter().map(b64).collect::<Vec<_>>().join(",");
        joined
            .replace('+', "%2B")
            .replace('/', "%2F")
            .replace('=', "%3D")
            .replace(',', "%2C")
    }

    async fn extract(
        certs: Option<ClientCertificates>,
        values: &[&str],
    ) -> Result<MachineCertificate, ClientCertificateError> {
        let mut request = Request::builder().uri("/internal/machines/renew");
        for v in values {
            request = request.header(CLIENT_CERT_HEADER, *v);
        }
        let (mut parts, ()) = request.body(()).unwrap().into_parts();
        if let Some(c) = certs {
            parts.extensions.insert(c);
        }
        MachineCertificate::from_request_parts(&mut parts, &()).await
    }

    fn trusted() -> Option<ClientCertificates> {
        Some(ClientCertificates::trusting(Arc::new(Fake)))
    }

    #[tokio::test]
    async fn client_cert_believes_only_the_first_certificate() {
        // A real one followed by one the client made itself, as admin.
        let value = header(&["cert:worker:ab12", "cert:admin:ffff"]);
        assert!(value.contains("%2C"), "{value}");
        let cert = extract(trusted(), &[&value]).await.unwrap();
        assert_eq!(cert.unit.as_deref(), Some("worker"));
        assert_eq!(cert.serial, "ab12");
        let one = extract(trusted(), &[&header(&["cert:worker:ab12"])]).await;
        assert_eq!(one.unwrap().serial, "ab12");
    }

    #[tokio::test]
    async fn client_cert_refuses_garbage_and_a_missing_header() {
        let unreadable = Err(ClientCertificateError::Unreadable);
        for bad in ["%ZZ", "%2", "not base64!", "%2Ccert", ""] {
            assert_eq!(extract(trusted(), &[bad]).await, unreadable, "{bad:?}");
        }
        // Base64, but not a certificate first; a certificate after it.
        let value = header(&["garbage", "cert:worker:ab12"]);
        assert_eq!(extract(trusted(), &[&value]).await, unreadable);
        let missing = Err(ClientCertificateError::Missing);
        assert_eq!(extract(trusted(), &[]).await, missing);
        let good = header(&["cert:worker:ab12"]);
        let twice = extract(trusted(), &[&good, &good]).await;
        assert_eq!(twice, missing, "two headers: one was not Traefik's");
    }

    #[tokio::test]
    async fn client_cert_is_not_read_unless_trusted() {
        let good = header(&["cert:worker:ab12"]);
        let not_trusted = Err(ClientCertificateError::NotTrusted);
        let off = Some(ClientCertificates::not_trusted());
        assert_eq!(extract(off, &[&good]).await, not_trusted);
        assert_eq!(extract(None, &[&good]).await, not_trusted);
        assert_eq!(extract(None, &["%ZZ"]).await, not_trusted);
        let status = |e: ClientCertificateError| e.into_response().status();
        assert_eq!(
            status(ClientCertificateError::NotTrusted),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status(ClientCertificateError::Missing),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(ClientCertificateError::Unreadable),
            status(ClientCertificateError::Missing)
        );
    }
}
