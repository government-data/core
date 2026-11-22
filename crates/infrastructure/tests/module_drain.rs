//! A module's drain path asked over HTTP, against a fake module on a local
//! socket: it answers its counts for the horizon asked; one that is slow,
//! answers an error status or an answer that does not read is an error.

use std::time::Duration;

use infrastructure::module_drain::{DrainEndpoint, HttpModuleDrains, ModuleDrain};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fake module: `/busy` counts, `/slow` does not answer in time,
/// `/broken` fails, `/odd` answers without the counts. Each request line
/// must carry the horizon asked for.
async fn fake_module() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = request.lines().next().unwrap_or_default().to_string();
                let (status, body) = if !line.contains("horizon_secs=60 ") {
                    ("400 Bad Request", "{}")
                } else if line.starts_with("GET /busy?") {
                    (
                        "200 OK",
                        r#"{"running": 1, "queued": 4, "scheduled_soon": 2}"#,
                    )
                } else if line.starts_with("GET /idle?") {
                    ("200 OK", r#"{"running": 0, "scheduled_soon": 0}"#)
                } else if line.starts_with("GET /slow?") {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    ("200 OK", r#"{"running": 0, "scheduled_soon": 0}"#)
                } else if line.starts_with("GET /odd?") {
                    ("200 OK", r#"{"busy": true}"#)
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
async fn each_module_answers_its_counts_or_is_an_error() {
    let base = fake_module().await;
    let endpoint = |module: &str, path: &str| DrainEndpoint {
        module: module.into(),
        url: format!("{base}{path}"),
    };
    let drains = HttpModuleDrains::new(
        vec![
            endpoint("busy", "/busy"),
            endpoint("idle", "/idle"),
            endpoint("slow", "/slow"),
            endpoint("broken", "/broken"),
            endpoint("odd", "/odd"),
        ],
        Duration::from_millis(500),
    )
    .unwrap();
    let answers = drains.work(Duration::from_secs(60)).await;
    let names: Vec<_> = answers.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(names, ["busy", "idle", "slow", "broken", "odd"]);
    assert_eq!(
        answers[0].1,
        Ok(ModuleDrain {
            running: 1,
            queued: 4,
            scheduled_soon: 2,
        })
    );
    assert_eq!(
        answers[1].1,
        Ok(ModuleDrain::default()),
        "queued is optional"
    );
    for (module, answer) in &answers[2..] {
        assert!(answer.is_err(), "{module}: {answer:?}");
    }
}

#[tokio::test]
async fn without_modules_nothing_is_asked() {
    let drains = HttpModuleDrains::new(Vec::new(), Duration::from_secs(1)).unwrap();
    assert!(drains.work(Duration::from_secs(60)).await.is_empty());
}
