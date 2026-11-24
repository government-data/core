//! The replication's adapter against a stand-in of both lakes over plain
//! HTTP on the loopback: the calls it signs, in order, with each lake's
//! kept keys, their bodies, and what it makes of the answers. RustFS
//! itself is in `tools/verify/rustfs_replication_app.sh`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use auth_gateway::{Lake, LakeAdminKeys, ReplicationOutcome, ReplicationStep};
use domain::RepositoryError;
use infrastructure::rustfs_admin::{AdminKeySource, RustfsLakeAdmin};
use infrastructure::rustfs_replication::WarehouseReplication;

const SOURCE_SECRET: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";
const REPLICA_SECRET: &str = "7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b7b";
const ARN: &str = "arn:minio:replication:us-east-1:d1:qd-tenant-a";

/// Both lakes' keys, or one missing.
struct Keys {
    main: bool,
    replica: bool,
}

#[async_trait]
impl AdminKeySource for Keys {
    async fn open(&self, lake: &Lake) -> Result<Option<LakeAdminKeys>, RepositoryError> {
        let (kept, secret) = match lake {
            Lake::Replica => (self.replica, REPLICA_SECRET),
            _ => (self.main, SOURCE_SECRET),
        };
        Ok(kept.then(|| LakeAdminKeys {
            access_key: "qd-platform-admin".into(),
            secret_key: secret.into(),
        }))
    }
}

/// A request as the stand-in saw it: its first line, its headers, its body.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    head: String,
    body: String,
}

/// A stand-in that answers each request with the next of `answers`.
fn lakes(answers: Vec<(&'static str, &'static str)>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
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
            let end = loop {
                let n = conn.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
                if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
            };
            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .map_or(0, |v| v.trim().parse().unwrap());
            while raw.len() < end + length {
                let n = conn.read(&mut buf).unwrap();
                raw.extend_from_slice(&buf[..n]);
            }
            let line = head.lines().next().unwrap_or_default().to_string();
            let got = String::from_utf8_lossy(&raw[end..end + length]).into_owned();
            kept.lock().unwrap().push(Seen {
                line,
                head,
                body: got,
            });
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            conn.write_all(reply.as_bytes()).unwrap();
        }
    });
    (port, seen)
}

fn warehouse(port: u16) -> WarehouseReplication {
    WarehouseReplication {
        warehouse: "qd-tenant-a".into(),
        source: format!("http://127.0.0.1:{port}"),
        replica: "127.0.0.1".into(),
        proxy: "lake-1:9100".into(),
    }
}

fn adapter(port: u16, main: bool, replica: bool) -> RustfsLakeAdmin {
    let keys = Arc::new(Keys { main, replica });
    RustfsLakeAdmin::new(reqwest::Client::new(), keys).at("http", port)
}

const OK: (&str, &str) = ("200 OK", "");

#[tokio::test]
async fn every_step_is_signed_by_its_lake_in_order_with_what_was_ruled() {
    let target = format!("\"{ARN}\"");
    let target: &'static str = Box::leak(target.into_boxed_str());
    let (port, seen) = lakes(vec![OK, OK, OK, OK, OK, ("200 OK", target), OK]);
    let outcome = adapter(port, true, true).configure(&warehouse(port)).await;
    assert_eq!(outcome.unwrap(), ReplicationOutcome::Configured);
    let seen = seen.lock().unwrap().clone();
    let lines: Vec<&str> = seen.iter().map(|s| s.line.as_str()).collect();
    assert_eq!(
        lines,
        [
            "head /qd-tenant-a http/1.1",
            "put /qd-tenant-a http/1.1",
            "put /qd-tenant-a?object-lock= http/1.1",
            "put /qd-tenant-a?versioning= http/1.1",
            "put /qd-tenant-a?lifecycle= http/1.1",
            "put /rustfs/admin/v3/set-remote-target?bucket=qd-tenant-a http/1.1",
            "put /qd-tenant-a?replication= http/1.1",
        ]
    );
    assert!(seen[1]
        .head
        .contains("x-amz-bucket-object-lock-enabled: true"));
    assert!(seen[2]
        .body
        .contains("<Mode>COMPLIANCE</Mode><Days>30</Days>"));
    assert!(seen[4].body.contains("<NoncurrentDays>7</NoncurrentDays>"));
    let target: serde_json::Value = serde_json::from_str(&seen[5].body).unwrap();
    assert_eq!(target["endpoint"], "lake-1:9100");
    assert_eq!(target["secure"], true);
    assert_eq!(target["credentials"]["secretKey"], REPLICA_SECRET);
    assert!(seen[6].body.contains(&format!("<Role>{ARN}</Role>")));
    assert!(seen[6]
        .body
        .contains("<DeleteMarkerReplication><Status>Disabled</Status>"));
    for s in &seen {
        assert!(s
            .head
            .contains("authorization: aws4-hmac-sha256 credential=qd-platform-admin/"));
        assert!(!s.head.contains(SOURCE_SECRET) && !s.head.contains(REPLICA_SECRET));
        assert!(!s.body.contains(SOURCE_SECRET), "{}", s.line);
    }
    // The replica's secret goes to the source in the target's body alone.
    let carrying: Vec<usize> = (0..seen.len())
        .filter(|i| seen[*i].body.contains(REPLICA_SECRET))
        .collect();
    assert_eq!(carrying, [5]);
}

#[tokio::test]
async fn a_missing_warehouse_or_missing_keys_ask_nothing_more() {
    let (port, seen) = lakes(vec![("404 Not Found", "")]);
    let outcome = adapter(port, true, true).configure(&warehouse(port)).await;
    assert_eq!(outcome.unwrap(), ReplicationOutcome::NoWarehouse);
    assert_eq!(seen.lock().unwrap().len(), 1);
    for (main, replica, lake) in [(false, true, Lake::main()), (true, false, Lake::Replica)] {
        let outcome = adapter(9, main, replica).configure(&warehouse(9)).await;
        assert_eq!(outcome.unwrap(), ReplicationOutcome::NoKeys { lake });
    }
}

#[tokio::test]
async fn a_refused_step_stops_the_run_and_says_which() {
    let refusal = "<Error><Code>AccessDenied</Code><Message>m</Message></Error>";
    let (port, seen) = lakes(vec![OK, OK, OK, OK, ("403 Forbidden", refusal)]);
    let outcome = adapter(port, true, true).configure(&warehouse(port)).await;
    let failed = ReplicationOutcome::Failed {
        step: ReplicationStep::SourceLifecycle,
        detail: "HTTP 403 AccessDenied".into(),
    };
    assert_eq!(outcome.unwrap(), failed);
    assert_eq!(seen.lock().unwrap().len(), 5);
    let (port, _) = lakes(vec![OK, OK, OK, OK, OK, ("200 OK", "not an arn")]);
    let outcome = adapter(port, true, true).configure(&warehouse(port)).await;
    let no_arn = ReplicationOutcome::Failed {
        step: ReplicationStep::RemoteTarget,
        detail: "no ARN in the answer".into(),
    };
    assert_eq!(outcome.unwrap(), no_arn);
}
