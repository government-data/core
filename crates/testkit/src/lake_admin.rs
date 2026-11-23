//! `lake-admin/1` served in process over the in-memory lake (decision
//! 0078, step 4, the user's ruling of 2026-11-23): HTTPS with mTLS of a
//! test CA both ways, the caller accepted only with the core server's
//! machine certificate (role `app`), anything else of the CA refused with
//! `403`. One request per connection; enough HTTP/1.1 for a client of the
//! protocol, nothing more.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use application::lake::{LakeAdmin, LakeCredentialKind, LakeError, LakeKeys};
use contracts::lake_admin::{self as wire, CredentialKind};
use contracts::LakeGrant;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::lake::InMemoryLake;
use crate::tls::TestCa;

/// The role whose certificate the adapter accepts: the core server's.
pub const CALLER_ROLE: &str = "app";

/// A running server: where it answers, and what it was asked.
#[derive(Debug, Clone)]
pub struct LakeAdminServer {
    /// `https://localhost:<port>`, the base `/lake-admin/v1` goes under.
    pub url: String,
    pub lake: Arc<InMemoryLake>,
    pools: Arc<Mutex<BTreeMap<String, wire::PoolLayout>>>,
}

/// Serves `lake` with a server certificate of `ca` for `localhost`, of
/// the adapter's role (`lake`).
pub async fn serve(lake: Arc<InMemoryLake>, ca: &TestCa) -> LakeAdminServer {
    serve_as(lake, ca, "lake").await
}

/// [`serve`], its own certificate of `role`: a server that is no adapter.
pub async fn serve_as(lake: Arc<InMemoryLake>, ca: &TestCa, role: &str) -> LakeAdminServer {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca.pem.as_bytes()).expect("the CA's PEM"))
        .expect("the CA as a root");
    let verifier =
        rustls::server::WebPkiClientVerifier::builder_with_provider(roots.into(), provider.clone())
            .build()
            .expect("a client verifier");
    let own = ca.machine(role, "localhost");
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(own.der.clone())],
            PrivateKeyDer::from_pem_slice(own.key_pem.as_bytes()).expect("the server's key"),
        )
        .expect("the server's certificate");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its address").port();
    let server = LakeAdminServer {
        url: format!("https://localhost:{port}"),
        lake,
        pools: Arc::default(),
    };
    let state = server.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, state) = (acceptor.clone(), state.clone());
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    state.answer(tls).await;
                }
            });
        }
    });
    server
}

/// What an answer carries: its status and its JSON body, if any.
type Answer = (u16, Option<String>);

impl LakeAdminServer {
    async fn answer(&self, mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>) {
        let role = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .and_then(|c| role_of(c));
        let Some((method, path, body)) = read_request(&mut tls).await else {
            return;
        };
        let (status, body) = if role.as_deref() == Some(CALLER_ROLE) {
            self.route(&method, &path, &body).await
        } else {
            let what = format!("{method} {path}: not the core server's certificate");
            error(&LakeError::Refused(what))
        };
        let body = body.unwrap_or_default();
        let head = format!(
            "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            reason(status),
            body.len()
        );
        let _ = tls.write_all(head.as_bytes()).await;
        let _ = tls.write_all(body.as_bytes()).await;
        let _ = tls.shutdown().await;
    }

    async fn route(&self, method: &str, path: &str, body: &[u8]) -> Answer {
        let Some(rest) = path.strip_prefix(wire::BASE) else {
            return error(&LakeError::Unknown(format!("{method} {path}")));
        };
        let parts: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        let admin: &dyn LakeAdmin = self.lake.as_ref();
        let result = match (method, parts.as_slice()) {
            ("GET", ["about"]) => ok(&wire::About {
                protocols: vec![wire::PROTOCOL.into()],
                implementation: "testkit".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            }),
            ("GET", ["health"]) => admin.health().await.map(|h| json(&wire::Health::from(h))),
            ("PUT", ["warehouses", name]) => match parse::<wire::RetentionRules>(body) {
                Ok(rules) => admin
                    .ensure_warehouse(name, &rules.into())
                    .await
                    .map(|()| (204, None)),
                Err(e) => Err(e),
            },
            ("PUT", ["warehouses", name, "replication"]) => {
                match parse::<wire::ReplicaTarget>(body) {
                    Ok(target) => admin
                        .replicate(name, &target.into())
                        .await
                        .map(|s| json(&wire::ReplicationState::from(s))),
                    Err(e) => Err(e),
                }
            }
            ("GET", ["warehouses", name, "replication"]) => admin
                .replication(name)
                .await
                .map(|s| json(&wire::ReplicationState::from(s))),
            ("PUT", ["policies", "managed"]) => match parse::<wire::ManagedPolicies>(body) {
                Ok(p) => {
                    let policies: Vec<_> = p.policies.into_iter().map(Into::into).collect();
                    admin
                        .set_managed_policies(&policies)
                        .await
                        .map(|()| (204, None))
                }
                Err(e) => Err(e),
            },
            ("POST", ["policies", "session"]) => match parse::<LakeGrant>(body) {
                Ok(grant) => admin
                    .session_policy(&grant)
                    .await
                    .map(|p| json(&wire::SessionPolicy::from(p))),
                Err(e) => Err(e),
            },
            ("PUT", ["credentials", kind]) => self.credentials(kind, body).await,
            ("GET", ["pools"]) => Ok(self.pools()),
            ("PUT", ["pools", id]) => match parse::<wire::PoolLayout>(body) {
                Ok(layout) => {
                    let mut pools = self.pools.lock().expect("the pools");
                    pools.insert((*id).to_string(), layout);
                    Ok((204, None))
                }
                Err(e) => Err(e),
            },
            _ => Err(LakeError::Unknown(format!("{method} {path}"))),
        };
        result.unwrap_or_else(|e| error(&e))
    }

    async fn credentials(&self, kind: &str, body: &[u8]) -> Result<Answer, LakeError> {
        let kind = match kind {
            "root" => CredentialKind::Root,
            "admin" => CredentialKind::Admin,
            "sts-issuer" => CredentialKind::StsIssuer,
            other => return Err(LakeError::Unknown(format!("credentials {other}"))),
        };
        let request = parse::<wire::CredentialsRequest>(body)?;
        let held = request.held.map(LakeKeys::from);
        let admin: &dyn LakeAdmin = self.lake.as_ref();
        let keys = admin
            .ensure_credentials(LakeCredentialKind::from(kind), held.as_ref())
            .await?;
        Ok(json(&wire::Credentials::from(&keys)))
    }

    fn pools(&self) -> Answer {
        let pools = self.pools.lock().expect("the pools");
        json(&wire::Pools {
            pools: pools
                .iter()
                .map(|(id, layout)| wire::Pool {
                    id: id.clone(),
                    layout: layout.clone(),
                })
                .collect(),
        })
    }
}

fn ok<T: Serialize>(value: &T) -> Result<Answer, LakeError> {
    Ok(json(value))
}

fn json<T: Serialize>(value: &T) -> Answer {
    (
        200,
        Some(serde_json::to_string(value).expect("a JSON answer")),
    )
}

fn error(e: &LakeError) -> Answer {
    let body = wire::ErrorBody::from(e);
    (
        body.code.status(),
        Some(serde_json::to_string(&body).expect("an error body")),
    )
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, LakeError> {
    serde_json::from_slice(body).map_err(|e| LakeError::Invalid(format!("the request: {e}")))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        502 => "Bad Gateway",
        _ => "Service Unavailable",
    }
}

/// The `OU` of a certificate of the CA: the machine's role.
fn role_of(der: &CertificateDer<'_>) -> Option<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der.as_ref()).ok()?;
    let ou = cert.subject().iter_organizational_unit().next()?;
    ou.as_str().ok().map(str::to_string)
}

/// The request line's method and path, and its body by `content-length`.
async fn read_request(
    tls: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Option<(String, String, Vec<u8>)> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = tls.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    while raw.len() < head_end + length {
        let n = tls.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let body = raw[head_end..].to_vec();
    Some((method, path, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::Leaf;

    fn client(ca: &TestCa, me: &Leaf) -> reqwest::Client {
        reqwest::Client::builder()
            .use_rustls_tls()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(ca.pem.as_bytes()).unwrap())
            .identity(reqwest::Identity::from_pem(me.bundle().as_bytes()).unwrap())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn the_core_servers_certificate_is_answered_and_another_role_refused() {
        let ca = TestCa::new();
        let server = serve(Arc::default(), &ca).await;
        let about = format!("{}{}", server.url, wire::paths::about());
        let core = client(&ca, &ca.machine(CALLER_ROLE, "app-1"));
        let answer = core.get(&about).send().await.unwrap();
        assert_eq!(answer.status(), 200);
        let about_body: wire::About = answer.json().await.unwrap();
        assert_eq!(about_body.protocols, [wire::PROTOCOL]);

        let worker = client(&ca, &ca.machine("worker", "worker-1"));
        let refused = worker.get(&about).send().await.unwrap();
        assert_eq!(refused.status(), 403);
        let body: wire::ErrorBody = refused.json().await.unwrap();
        assert_eq!(
            (body.code, body.retryable),
            (wire::ErrorCode::Refused, false)
        );
    }

    #[tokio::test]
    async fn a_warehouse_put_twice_and_an_unknown_one_asked() {
        let ca = TestCa::new();
        let server = serve(Arc::default(), &ca).await;
        let core = client(&ca, &ca.machine(CALLER_ROLE, "app-1"));
        let put = format!("{}{}", server.url, wire::paths::warehouse("qd-tenant-a1"));
        for _ in 0..2 {
            let answer = core
                .put(&put)
                .json(&wire::RetentionRules::default())
                .send()
                .await;
            assert_eq!(answer.unwrap().status(), 204);
        }
        assert_eq!(server.lake.snapshot().warehouses.len(), 1);
        let unknown = format!("{}{}", server.url, wire::paths::replication("qd-tenant-b2"));
        let answer = core.get(&unknown).send().await.unwrap();
        assert_eq!(answer.status(), 404);
        let body: wire::ErrorBody = answer.json().await.unwrap();
        assert_eq!(body.code, wire::ErrorCode::Unknown);
    }
}
