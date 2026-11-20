//! The client's credentials in HTTP Basic (RFC 6749 §2.3.1), each part
//! form-urlencoded.

use axum::http::{header, HeaderMap};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// `client_id:client_secret` from `Authorization: Basic`, form-urlencoded
/// each (RFC 6749 §2.3.1).
pub(super) fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let encoded = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((form_decode(id)?, form_decode(secret)?))
}

/// `application/x-www-form-urlencoded` decoding of one value.
fn form_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn basic_credentials_are_form_decoded() {
        let mut headers = HeaderMap::new();
        let value = format!("Basic {}", STANDARD.encode("gestion:a%2Bb+c"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&value).unwrap(),
        );
        assert_eq!(
            basic_credentials(&headers),
            Some(("gestion".into(), "a+b c".into()))
        );
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic !!"));
        assert_eq!(basic_credentials(&headers), None);
    }
}
