//! `lake-admin/1` at the wire: a client that presents a machine
//! certificate and trusts the installation's CA alone, and the cases every
//! adapter passes: each operation run twice answers the same, a caller
//! refused by its certificate, and the error shape.

use contracts::lake_admin::{self as wire, CredentialKind};
use contracts::LakeGrant;
use serde_json::{json, Value};

/// A caller of one adapter, as one machine.
#[derive(Debug, Clone)]
pub struct AdminClient {
    base: String,
    http: reqwest::Client,
}

/// An answer: its status and its JSON body (`Null` when empty).
pub type Answer = (u16, Value);

impl AdminClient {
    /// The adapter at `base` (`https://host:port`), trusting `ca_pem`
    /// alone, presenting `identity_pem` (the certificate then the key).
    pub fn new(base: &str, ca_pem: &str, identity_pem: &str) -> Self {
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(
                reqwest::Certificate::from_pem(ca_pem.as_bytes()).expect("the CA's PEM"),
            )
            .identity(reqwest::Identity::from_pem(identity_pem.as_bytes()).expect("an identity"))
            .build()
            .expect("an HTTPS client");
        Self {
            base: base.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// `method` on `path` (one of `wire::paths`) with `body` if any.
    pub async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Answer {
        self.call_with(method, path, body, &[]).await
    }

    /// The same, with `headers` (the keys an operation needs).
    pub async fn call_with(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, String)],
    ) -> Answer {
        let url = format!("{}{path}", self.base);
        let method = reqwest::Method::from_bytes(method.as_bytes()).expect("a method");
        let mut request = self.http.request(method, &url);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.expect("the adapter answers");
        let status = response.status().as_u16();
        let text = response.text().await.expect("its body");
        let value = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).expect("a JSON body")
        };
        (status, value)
    }
}

/// Every operation of `lake-admin/1`, each run twice: both answers alike
/// and successful. `warehouse` is made by the case: `201` and `made` the
/// first time, then `200` and `present`.
pub async fn operations_twice(client: &AdminClient, warehouse: &str) {
    let path = wire::paths::warehouse(warehouse);
    let rules = json!({"versioning": true});
    let made = client.call("PUT", &path, Some(&rules)).await;
    assert_eq!(made, (201, json!({"state": "made"})), "PUT {path}");
    let grant = serde_json::to_value(LakeGrant::read_only(warehouse)).expect("a grant");
    let calls: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", wire::paths::about(), None),
        ("GET", wire::paths::health(), None),
        (
            "PUT",
            wire::paths::warehouse(warehouse),
            Some(json!({"versioning": true})),
        ),
        (
            "PUT",
            wire::paths::replication(warehouse),
            Some(json!({"endpoint": "https://replica.lake.test", "source_pool": false})),
        ),
        ("GET", wire::paths::replication(warehouse), None),
        (
            "PUT",
            wire::paths::managed_policies(),
            Some(json!({"policies": [{"name": "quality-admin", "access": "administer"}]})),
        ),
        ("POST", wire::paths::session_policy(), Some(grant)),
        ("GET", wire::paths::pools(), None),
        (
            "PUT",
            wire::paths::pool("contract"),
            Some(json!({"nodes": [{"name": "lake-1", "drives": ["/srv/d1"]}]})),
        ),
    ];
    for (method, path, body) in &calls {
        let first = client.call(method, path, body.as_ref()).await;
        if path == &wire::paths::warehouse(warehouse) {
            assert_eq!(first.1, json!({"state": "present"}), "PUT {path} again");
        }
        let second = client.call(method, path, body.as_ref()).await;
        assert!((200..300).contains(&first.0), "{method} {path}: {first:?}");
        assert_eq!(first, second, "{method} {path} twice answers the same");
    }
    let (_, about) = client.call("GET", &wire::paths::about(), None).await;
    let about: wire::About = serde_json::from_value(about).expect("an about");
    assert!(
        about.protocols.iter().any(|p| p == wire::PROTOCOL),
        "{about:?}"
    );

    let path = wire::paths::credentials(CredentialKind::Admin);
    let (status, made) = client.call("PUT", &path, Some(&json!({}))).await;
    assert_eq!(status, 200, "keys made: {made}");
    let held = json!({ "held": made.clone() });
    let (status, kept) = client.call("PUT", &path, Some(&held)).await;
    assert_eq!(
        (status, kept),
        (200, made),
        "the keys held are kept while taken"
    );
}

/// A caller whose certificate is of the CA but not the core server's:
/// refused with `403`, the error shape, and nothing changed.
pub async fn refused_by_its_certificate(stranger: &AdminClient, warehouse: &str) {
    let path = wire::paths::warehouse(warehouse);
    let (status, body) = stranger.call("PUT", &path, Some(&json!({}))).await;
    assert_eq!(status, 403, "{body}");
    error_shape(&body, wire::ErrorCode::Refused);
}

/// The error shape: an unknown warehouse (`404 unknown`, asked with
/// `keys`, the headers that carry the lake's admin keys) and a request the
/// adapter cannot read (`400 invalid`), neither retryable.
pub async fn errors(client: &AdminClient, unknown_warehouse: &str, keys: &[(&str, String)]) {
    let path = wire::paths::replication(unknown_warehouse);
    let (status, body) = client.call_with("GET", &path, None, keys).await;
    assert_eq!(status, 404, "{body}");
    error_shape(&body, wire::ErrorCode::Unknown);
    let path = wire::paths::session_policy();
    let (status, body) = client
        .call("POST", &path, Some(&json!({"no": "grant"})))
        .await;
    assert_eq!(status, 400, "{body}");
    error_shape(&body, wire::ErrorCode::Invalid);
}

fn error_shape(body: &Value, code: wire::ErrorCode) {
    let error: wire::ErrorBody = serde_json::from_value(body.clone()).expect("the error shape");
    assert_eq!(error.code, code, "{body}");
    assert_eq!(error.retryable, code.retryable(), "{body}");
    assert!(!error.error.is_empty(), "the message names the operation");
}
