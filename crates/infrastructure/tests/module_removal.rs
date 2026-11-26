//! A module's removal path asked over HTTP, against two fake modules on a
//! local socket: one refuses with a reason and counts, one does not answer
//! in time, one answers an error status.

use std::time::Duration;

use application::{RemovalChecks, RemovalEndpoint};
use infrastructure::module_removal::HttpModuleRemovals;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn fake_modules() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = request.lines().next().unwrap_or_default().to_string();
                let (status, body) = if !line.contains("?module=engine-trino ") {
                    ("400 Bad Request", "{}")
                } else if line.starts_with("GET /modules/orchestrator/internal/removal?") {
                    (
                        "200 OK",
                        r#"{"refuse": true, "reason": "flows on its line", "counts": {"flows": 2}}"#,
                    )
                } else if line.starts_with("GET /modules/slow/r?") {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    ("200 OK", r#"{"refuse": false}"#)
                } else {
                    ("500 Internal Server Error", "{}")
                };
                let answer = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            });
        }
    });
    base
}

#[tokio::test]
async fn each_module_is_asked_with_the_removed_module() {
    let base = fake_modules().await;
    let removals = HttpModuleRemovals::plain(&base, Duration::from_millis(500));
    let endpoint = |module: &str, path: &str| RemovalEndpoint {
        module: module.into(),
        path: path.into(),
    };
    let endpoints = [
        endpoint("orchestrator", "/internal/removal"),
        endpoint("slow", "/r"),
        endpoint("broken", "/r"),
    ];
    let answers = removals.ask(&endpoints, "engine-trino").await;
    let names: Vec<_> = answers.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(names, ["orchestrator", "slow", "broken"]);
    let refused = answers[0].1.as_ref().unwrap();
    assert!(refused.refuse);
    assert_eq!(refused.reason.as_deref(), Some("flows on its line"));
    assert_eq!(refused.counts["flows"], 2);
    assert!(answers[1].1.is_err(), "too slow");
    assert!(answers[2].1.is_err(), "an error status");
}
