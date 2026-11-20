//! AWS Signature Version 4, as RustFS checks it (decisions 0028, 0030, 0039): the
//! canonical request of the headers and the query given, signed for one
//! service in `us-east-1`.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const REGION: &str = "us-east-1";

/// What a request needs to carry once signed (the signature, never the
/// secret key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// `x-amz-date`.
    pub date: String,
    pub authorization: String,
}

/// Signs `method path` with `headers` (lowercase names, `host`,
/// `x-amz-content-sha256` and `x-amz-date` among them, in any order) for
/// `service`, with no query. `x-amz-date` is set from `now`.
pub fn sign(
    service: &str,
    access: &str,
    secret: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    now: DateTime<Utc>,
) -> Signed {
    let request = Request {
        method,
        path,
        query: &[],
        headers,
    };
    sign_request(service, access, secret, &request, now)
}

/// A request to sign: its method, its path (encoded), its query (unencoded
/// name and value pairs; it carries [`query_string`] of them) and its
/// headers (lowercase names, `host` and `x-amz-content-sha256` among them).
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a [(&'a str, &'a str)],
    pub headers: &'a [(&'a str, &'a str)],
}

/// The query string as a request carries it and as it is signed: each
/// name and value encoded, sorted.
pub fn query_string(query: &[(&str, &str)]) -> String {
    let mut pairs: Vec<String> = query
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k, false), encode(v, false)))
        .collect();
    pairs.sort();
    pairs.join("&")
}

/// Signs `request` for `service`; `x-amz-date` is set from `now`.
pub fn sign_request(
    service: &str,
    access: &str,
    secret: &str,
    request: &Request,
    now: DateTime<Utc>,
) -> Signed {
    let Request {
        method,
        path,
        query,
        headers,
    } = *request;
    let date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let day = &date[..8];
    let mut all: Vec<(String, String)> = headers
        .iter()
        .filter(|(n, _)| *n != "x-amz-date")
        .map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    all.push(("x-amz-date".into(), date.clone()));
    all.sort();
    let payload_hash = all
        .iter()
        .find(|(n, _)| n == "x-amz-content-sha256")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let canonical_headers: String = all.iter().map(|(n, v)| format!("{n}:{v}\n")).collect();
    let names = all
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let query = query_string(query);
    let canonical =
        format!("{method}\n{path}\n{query}\n{canonical_headers}\n{names}\n{payload_hash}");
    let scope = format!("{day}/{REGION}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let mut key = hmac(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    for part in [REGION, service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={names}, Signature={signature}"
        ),
        date,
    }
}

/// The SHA-256 of a payload, as `x-amz-content-sha256` carries it.
pub fn payload_hash(body: &[u8]) -> String {
    hex(&Sha256::digest(body))
}

/// RFC 3986's unreserved characters stay; everything else is `%XX`
/// (`/` too unless `keep_slash`, for an object key's path).
pub fn encode(s: &str, keep_slash: bool) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b'/' if keep_slash => "/".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `host[:port]` of an endpoint URL (`https://host[:port][/path]`), the
/// port only if it is written and not the scheme's own.
pub fn host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    let default = match scheme {
        "https" => ":443",
        "http" => ":80",
        _ => return None,
    };
    Some(
        authority
            .strip_suffix(default)
            .unwrap_or(authority)
            .to_ascii_lowercase(),
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosts_name_is_its_authority_without_the_schemes_own_port() {
        assert_eq!(
            host("https://Lake.Example/iceberg").as_deref(),
            Some("lake.example")
        );
        assert_eq!(
            host("https://localhost:443/x").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            host("http://127.0.0.1:9001").as_deref(),
            Some("127.0.0.1:9001")
        );
        assert_eq!(
            host("https://localhost:8443?q").as_deref(),
            Some("localhost:8443")
        );
        assert_eq!(host("ftp://x"), None);
        assert_eq!(host("https://"), None);
    }

    #[test]
    fn a_query_is_signed_as_botocore_signs_it() {
        use chrono::TimeZone;
        // botocore's S3SigV4Auth("s3", "us-east-1") on the same request.
        let query = [
            ("list-type", "2"),
            ("prefix", "tables/u-1/data/"),
            ("continuation-token", "a+b="),
        ];
        assert_eq!(
            query_string(&query),
            "continuation-token=a%2Bb%3D&list-type=2&prefix=tables%2Fu-1%2Fdata%2F"
        );
        let hash = payload_hash(b"");
        let request = Request {
            method: "GET",
            path: "/qd-tenant-a",
            query: &query,
            headers: &[
                ("host", "lake.example"),
                ("x-amz-content-sha256", &hash),
                ("x-amz-security-token", "TOKEN"),
            ],
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let signed = sign_request("s3", "AKID", "SECRET", &request, now);
        assert!(
            signed.authorization.ends_with(
                "Signature=13df7aabd46e01b7bc36bbea4bc817471c1eb068f445fe097f84279bbe63d481"
            ),
            "{}",
            signed.authorization
        );
    }
}
