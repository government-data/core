//! The lakes' admin API's adapter against a stand-in of a node's gate over
//! plain HTTP on the loopback: the request it signs with the kept keys, and
//! what it makes of each answer. RustFS itself is in
//! `tools/verify/rustfs_replication_gate.sh`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdmin, LakeAdminKeys, LakeProbe};
use domain::RepositoryError;
use infrastructure::rustfs_admin::{AdminKeySource, RustfsLakeAdmin};

struct Keys(Option<LakeAdminKeys>);

#[async_trait]
impl AdminKeySource for Keys {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        assert!(matches!(lake, Lake::Replica) || *lake == Lake::main());
        Ok(self.0.clone())
    }
}

fn kept() -> Arc<Keys> {
    Arc::new(Keys(Some(LakeAdminKeys {
        access_key: "qd-platform-admin".into(),
        secret_key: "ab".repeat(32),
    })))
}

/// A gate that answers one request with `status` and `body`; the request's
/// head is kept.
fn gate(status: &str, body: &str) -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let (kept, status, body) = (seen.clone(), status.to_string(), body.to_string());
    std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut buf = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = conn.read(&mut buf).unwrap();
            head.extend_from_slice(&buf[..n]);
        }
        *kept.lock().unwrap() = String::from_utf8_lossy(&head).into_owned();
        let reply = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(reply.as_bytes()).unwrap();
    });
    (port, seen)
}

fn admin(keys: Arc<Keys>, port: u16) -> RustfsLakeAdmin {
    RustfsLakeAdmin::new(reqwest::Client::new(), keys).at("http", port)
}

#[tokio::test]
async fn info_is_asked_signed_with_the_kept_keys_and_its_drives_read() {
    let info = r#"{"info":{"backend":{"backendType":"Erasure","onlineDisks":4,"offlineDisks":0}}}"#;
    let (port, seen) = gate("200 OK", info);
    let probe = admin(kept(), port).probe(&Lake::Replica, "127.0.0.1").await;
    let drives = LakeProbe::Answered {
        drives_online: 4,
        drives_offline: 0,
    };
    assert_eq!(probe.unwrap(), drives);
    let head = seen.lock().unwrap().to_ascii_lowercase();
    assert!(
        head.starts_with("get /rustfs/admin/v3/info http/1.1\r\n"),
        "{head}"
    );
    assert!(
        head.contains(&format!("host: 127.0.0.1:{port}\r\n")),
        "{head}"
    );
    assert!(head.contains("content-length: 0\r\n"), "{head}");
    assert!(head.contains("authorization: aws4-hmac-sha256 credential=qd-platform-admin/"));
    assert!(head.contains("signedheaders=content-length;host;x-amz-content-sha256;x-amz-date"));
    assert!(!head.contains(&"ab".repeat(32)), "the secret travelled");
}

#[tokio::test]
async fn refused_keys_and_other_answers_are_told_apart() {
    let refusal = "<Error><Code>SignatureDoesNotMatch</Code><Message>m</Message></Error>";
    let (port, _) = gate("403 Forbidden", refusal);
    let probe = admin(kept(), port).probe(&Lake::Replica, "127.0.0.1").await;
    let refused = LakeProbe::KeysRefused {
        code: "SignatureDoesNotMatch".into(),
    };
    assert_eq!(probe.unwrap(), refused);
    let (port, _) = gate("502 Bad Gateway", "");
    let probe = admin(kept(), port).probe(&Lake::Replica, "127.0.0.1").await;
    let detail = "HTTP 502".to_string();
    assert_eq!(probe.unwrap(), LakeProbe::Unreachable { detail });
}

#[tokio::test]
async fn nothing_is_asked_without_keys_and_a_closed_port_is_unreachable() {
    let none = admin(Arc::new(Keys(None)), 9);
    let probe = none.probe(&Lake::Replica, "127.0.0.1").await;
    assert_eq!(probe.unwrap(), LakeProbe::NoKeys);
    // A port nothing listens on: bound, then let go.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let probe = admin(kept(), port).probe(&Lake::Replica, "127.0.0.1").await;
    match probe.unwrap() {
        LakeProbe::Unreachable { detail } => {
            assert!(detail.contains("127.0.0.1"), "{detail}");
            assert!(!detail.contains(&"ab".repeat(32)));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_main_lake_is_asked_at_its_base_url() {
    let info = r#"{"info":{"backend":{"onlineDisks":1,"offlineDisks":0}}}"#;
    let (port, seen) = gate("200 OK", info);
    // Its own scheme and port, not the gate's default.
    let api = RustfsLakeAdmin::new(reqwest::Client::new(), kept()).at("https", 9);
    let probe = api
        .probe(&Lake::main(), &format!("http://127.0.0.1:{port}/"))
        .await;
    let drives = LakeProbe::Answered {
        drives_online: 1,
        drives_offline: 0,
    };
    assert_eq!(probe.unwrap(), drives);
    let head = seen.lock().unwrap().to_ascii_lowercase();
    assert!(
        head.starts_with("get /rustfs/admin/v3/info http/1.1\r\n"),
        "{head}"
    );
    assert!(
        head.contains(&format!("host: 127.0.0.1:{port}\r\n")),
        "{head}"
    );
}
