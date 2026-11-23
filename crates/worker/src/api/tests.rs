//! The worker's calls to the API with the machine's certificate, against a
//! real HTTPS server that asks for a client certificate of a test CA, as
//! Traefik's internal entry point does: the token asked with no secret,
//! the certificate the agent renews presented without a restart, and a
//! token refused once asked again with the files read again. Synthetic
//! values only.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use super::{Api, Auth, Client};
use crate::machine::{MachineFiles, MachineHttp};
use crate::testing::{write_machine, Leaf, TempDir, TestCa};

/// One request as the server saw it.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    authorization: bool,
    body: String,
    /// The client certificate presented, DER.
    cert: Option<Vec<u8>>,
}

/// What the server accepts and what it saw.
#[derive(Default)]
struct State {
    /// The certificates that get a token (the serial in use and the next).
    accepted: Vec<Vec<u8>>,
    seen: Vec<Seen>,
    /// Done at the first token request, before answering it: the agent
    /// promoting a renewal meanwhile.
    on_first_token: Option<Box<dyn FnOnce() + Send>>,
}

type Shared = Arc<Mutex<State>>;

/// An HTTPS server on `localhost` with a certificate of `ca`, asking for a
/// client certificate of it (and taking requests without one).
async fn server(ca: &TestCa, state: Shared) -> String {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca.pem.as_bytes()).unwrap())
        .unwrap();
    let verifier =
        rustls::server::WebPkiClientVerifier::builder_with_provider(roots.into(), provider.clone())
            .allow_unauthenticated()
            .build()
            .unwrap();
    let own = ca.server();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(own.der.clone())],
            PrivateKeyDer::from_pem_slice(own.key_pem.as_bytes()).unwrap(),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, state) = (acceptor.clone(), state.clone());
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    answer(tls, state).await;
                }
            });
        }
    });
    format!("https://localhost:{port}/api")
}

/// Reads one request, answers it and closes.
async fn answer(mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>, state: Shared) {
    let cert = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| c.to_vec());
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = tls.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let length: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while raw.len() < head_end + length {
        let n = tls.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let seen = Seen {
        line: head.lines().next().unwrap_or_default().to_string(),
        authorization: head
            .to_ascii_lowercase()
            .contains("\r\nauthorization: basic"),
        body: String::from_utf8_lossy(&raw[head_end..]).to_string(),
        cert: cert.clone(),
    };
    let token = seen.line.starts_with("POST /api/oauth/token ");
    let hook = {
        let mut s = state.lock().unwrap();
        s.seen.push(seen.clone());
        let first = token
            && s.seen
                .iter()
                .filter(|r| r.line.starts_with("POST /api/oauth/token "))
                .count()
                == 1;
        if first {
            s.on_first_token.take()
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        hook();
    }
    let given = seen.authorization
        || cert
            .as_ref()
            .is_some_and(|c| state.lock().unwrap().accepted.contains(c));
    let (status, body) = match (token, given) {
        (true, true) => ("200 OK", r#"{"access_token":"t","expires_in":900}"#),
        (true, false) => ("401 Unauthorized", r#"{"error":"invalid_client"}"#),
        (false, _) => ("200 OK", "{}"),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tls.write_all(response.as_bytes()).await;
    let _ = tls.shutdown().await;
}

fn machine_api(dir: PathBuf, url: &str) -> Api {
    let files = MachineFiles::new(dir);
    let machine = files.machine_name().unwrap();
    let auth = Auth::Certificate {
        http: MachineHttp::new(files),
        machine,
    };
    let client = Client {
        id: "qd-worker".into(),
        auth,
    };
    Api::new(reqwest::Client::new(), url, client)
}

fn tokens(state: &Shared) -> Vec<Seen> {
    let s = state.lock().unwrap();
    s.seen
        .iter()
        .filter(|r| r.line.starts_with("POST /api/oauth/token "))
        .cloned()
        .collect()
}

#[tokio::test]
async fn with_its_certificate_the_worker_asks_its_token_without_a_secret() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    let leaf = ca.machine("worker-01");
    write_machine(dir.path(), &ca.pem, &leaf);
    let state = Shared::default();
    state.lock().unwrap().accepted.push(leaf.der.clone());
    let url = server(&ca, state.clone()).await;
    let api = machine_api(dir.path().into(), &url);
    assert_eq!(api.worker(), "qd-worker/worker-01");
    api.ready().await.unwrap();
    let asked = tokens(&state);
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].cert.as_ref(),
        Some(&leaf.der),
        "its certificate presented"
    );
    assert!(!asked[0].authorization, "no Basic secret");
    assert!(
        asked[0].body.contains("client_id=qd-worker"),
        "{}",
        asked[0].body
    );
    assert!(asked[0].body.contains("grant_type=client_credentials"));
    assert!(asked[0].body.contains("audience=quality-api"));
    assert!(!asked[0].body.contains("secret"));
}

#[tokio::test]
async fn a_renewed_certificate_is_presented_without_a_restart() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    let (old, new) = (ca.machine("worker-01"), ca.machine("worker-01"));
    write_machine(dir.path(), &ca.pem, &old);
    let state = Shared::default();
    state.lock().unwrap().accepted = vec![old.der.clone(), new.der.clone()];
    let url = server(&ca, state.clone()).await;
    let api = machine_api(dir.path().into(), &url);
    let id = uuid::Uuid::nil();
    api.report(id, &serde_json::json!({})).await.unwrap();
    write_machine(dir.path(), &ca.pem, &new);
    api.report(id, &serde_json::json!({})).await.unwrap();
    let reports: Vec<_> = state
        .lock()
        .unwrap()
        .seen
        .iter()
        .filter(|r| r.line.contains("/report "))
        .map(|r| r.cert.clone())
        .collect();
    assert_eq!(reports, vec![Some(old.der.clone()), Some(new.der.clone())]);
    assert_eq!(
        tokens(&state).len(),
        1,
        "the token, still live, is not asked again"
    );
}

#[tokio::test]
async fn a_token_refused_is_asked_once_more_with_the_files_read_again() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    let (old, new) = (ca.machine("worker-01"), ca.machine("worker-01"));
    write_machine(dir.path(), &ca.pem, &old);
    let state = Shared::default();
    // The new one was presented elsewhere: the old one is refused, and the
    // agent renames the new files in while the request is on its way.
    state.lock().unwrap().accepted = vec![new.der.clone()];
    let (path, ca_pem) = (dir.path().to_path_buf(), ca.pem.clone());
    let promote: Leaf = new.clone();
    state.lock().unwrap().on_first_token =
        Some(Box::new(move || write_machine(&path, &ca_pem, &promote)));
    let url = server(&ca, state.clone()).await;
    let api = machine_api(dir.path().into(), &url);
    api.ready().await.unwrap();
    let asked: Vec<_> = tokens(&state).into_iter().map(|r| r.cert).collect();
    assert_eq!(asked, vec![Some(old.der.clone()), Some(new.der.clone())]);
}

#[tokio::test]
async fn refused_again_the_worker_gives_up_and_takes_no_job() {
    let (ca, dir) = (TestCa::new(), TempDir::new());
    write_machine(dir.path(), &ca.pem, &ca.machine("worker-01"));
    let state = Shared::default();
    let url = server(&ca, state.clone()).await;
    let api = machine_api(dir.path().into(), &url);
    let refused = api.ready().await.unwrap_err();
    assert!(refused.to_string().contains("401"), "{refused}");
    assert_eq!(tokens(&state).len(), 2, "asked once more, not more");
}

#[tokio::test]
async fn with_a_secret_the_worker_is_as_before() {
    let ca = TestCa::new();
    let state = Shared::default();
    let url = server(&ca, state.clone()).await;
    let http = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.pem.as_bytes()).unwrap())
        .build()
        .unwrap();
    let client = Client {
        id: "qd-worker-dev".into(),
        auth: Auth::Secret("s3cret".into()),
    };
    let api = Api::new(http, &url, client);
    assert_eq!(api.worker(), "qd-worker-dev");
    assert!(!format!("{api:?}").contains("s3cret"));
    api.ready().await.unwrap();
    let asked = tokens(&state);
    assert_eq!(asked.len(), 1);
    assert!(asked[0].authorization && asked[0].cert.is_none());
}

fn job_lake() -> quality::lake::LakeAccess {
    quality::lake::LakeAccess {
        endpoint: "https://lake.example:9000".into(),
        warehouse: "qd-tenant-1".into(),
        access_key: "k".into(),
        secret_key: "s".into(),
        session_token: "t".into(),
        expires_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn with_a_proxy_every_jobs_lake_is_reached_there_its_keys_as_they_came() {
    let there = super::reached_at(Some("https://worker-01:9000"), job_lake());
    assert_eq!(there.endpoint, "https://worker-01:9000");
    let mut expected = job_lake();
    expected.endpoint = there.endpoint.clone();
    assert_eq!(there, expected);
    assert_eq!(super::reached_at(None, job_lake()), job_lake());
}

#[tokio::test]
async fn the_worker_waits_for_its_lake_proxy_and_refuses_an_address_without_a_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    crate::config::wait_for_lake(&url).await.unwrap();
    assert!(crate::config::wait_for_lake("not an address")
        .await
        .is_err());
}

#[test]
fn a_jobs_calls_carry_its_id_and_one_of_their_own() {
    let job = uuid::Uuid::from_u128(0x10);
    let (a, b) = (super::request_id(job), super::request_id(job));
    assert_ne!(a, b);
    for id in [&a, &b] {
        assert!(id.starts_with(&format!("{job}.")), "{id}");
        // What http-kit keeps as the request's id, and the Loki link reads whole.
        assert!(id.len() <= 128 && id.is_ascii(), "{id}");
        assert!(!id.contains(['"', '}', ' ']), "{id}");
    }
}
