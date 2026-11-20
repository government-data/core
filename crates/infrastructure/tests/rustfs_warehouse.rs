//! The warehouse's adapter against a stand-in of the main lake over plain
//! HTTP on the loopback: the calls it signs, in order, with the main lake's
//! kept keys, and what it makes of the answers. RustFS itself is in
//! `tools/verify/rustfs_warehouse.sh`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeys, LakeWarehouses, ReplicationStep, WarehouseOutcome};
use domain::RepositoryError;
use infrastructure::rustfs_admin::{AdminKeySource, RustfsLakeAdmin};

const SECRET: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";
const W: &str = "qd-tenant-0000000000000000000000000000000a";

/// The main lake's keys, kept or not; asked for which lake.
struct Keys {
    kept: bool,
    asked: Mutex<Vec<Lake>>,
}

#[async_trait]
impl AdminKeySource for Keys {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        self.asked.lock().unwrap().push(lake.clone());
        Ok(self.kept.then(|| LakeAdminKeys {
            access_key: "qd-platform-admin".into(),
            secret_key: SECRET.into(),
        }))
    }
}

/// A stand-in that answers each request with the next of `answers`, and
/// keeps each request's head.
fn lake(answers: Vec<(&'static str, &'static str)>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for (status, body) in answers {
            let Ok((mut conn, _)) = listener.accept() else {
                return;
            };
            let (mut raw, mut buf) = (Vec::new(), [0u8; 4096]);
            while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = conn.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
            }
            let head = String::from_utf8_lossy(&raw).to_ascii_lowercase();
            kept.lock().unwrap().push(head);
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            conn.write_all(reply.as_bytes()).unwrap();
        }
    });
    (port, seen)
}

fn adapter(kept: bool) -> (RustfsLakeAdmin, Arc<Keys>) {
    let keys = Arc::new(Keys {
        kept,
        asked: Mutex::default(),
    });
    let admin = RustfsLakeAdmin::new(reqwest::Client::new(), keys.clone());
    (admin, keys)
}

const OK: (&str, &str) = ("200 OK", "");
const MISSING: (&str, &str) = ("404 Not Found", "");

fn lines(seen: &Mutex<Vec<String>>) -> Vec<String> {
    let seen = seen.lock().unwrap();
    seen.iter()
        .map(|h| h.lines().next().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_missing_warehouse_is_made_then_its_table_bucket_with_the_main_lakes_keys() {
    let (port, seen) = lake(vec![MISSING, OK, OK]);
    let (admin, keys) = adapter(true);
    let source = format!("http://127.0.0.1:{port}");
    let outcome = admin.ensure(&source, W).await.unwrap();
    assert_eq!(outcome, WarehouseOutcome::Made);
    assert_eq!(
        lines(&seen),
        [
            format!("head /{W} http/1.1"),
            format!("put /{W} http/1.1"),
            format!("put /iceberg/v1/buckets/{W} http/1.1"),
        ]
    );
    assert_eq!(*keys.asked.lock().unwrap(), [Lake::main()]);
    for head in seen.lock().unwrap().iter() {
        assert!(head.contains("authorization: aws4-hmac-sha256 credential=qd-platform-admin/"));
        assert!(!head.contains(SECRET));
    }
}

#[tokio::test]
async fn a_warehouse_there_is_not_made_again_and_its_table_bucket_is_asked_again() {
    let (port, seen) = lake(vec![OK, OK]);
    let (admin, _) = adapter(true);
    let source = format!("http://127.0.0.1:{port}");
    let outcome = admin.ensure(&source, W).await.unwrap();
    assert_eq!(outcome, WarehouseOutcome::Present);
    assert_eq!(
        lines(&seen),
        [
            format!("head /{W} http/1.1"),
            format!("put /iceberg/v1/buckets/{W} http/1.1"),
        ]
    );
}

#[tokio::test]
async fn without_the_keys_nothing_is_asked_and_a_refusal_says_its_step() {
    let (admin, _) = adapter(false);
    let outcome = admin.ensure("http://127.0.0.1:9", W).await.unwrap();
    assert_eq!(outcome, WarehouseOutcome::NoKeys);
    let refusal = "<Error><Code>AccessDenied</Code><Message>m</Message></Error>";
    let (port, seen) = lake(vec![MISSING, ("403 Forbidden", refusal)]);
    let (admin, _) = adapter(true);
    let source = format!("http://127.0.0.1:{port}");
    let outcome = admin.ensure(&source, W).await.unwrap();
    let failed = WarehouseOutcome::Failed {
        step: ReplicationStep::WarehouseBucket,
        detail: "HTTP 403 AccessDenied".into(),
    };
    assert_eq!(outcome, failed);
    assert_eq!(lines(&seen).len(), 2, "the table bucket is not asked");
    let bad = r#"{"error":{"message":"Bucket not found","code":400}}"#;
    let (port, _) = lake(vec![OK, ("400 Bad Request", bad)]);
    let source = format!("http://127.0.0.1:{port}");
    let outcome = admin.ensure(&source, W).await.unwrap();
    let failed = WarehouseOutcome::Failed {
        step: ReplicationStep::WarehouseTables,
        detail: "HTTP 400".into(),
    };
    assert_eq!(outcome, failed);
}
