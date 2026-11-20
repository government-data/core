//! The five queries are fixed strings, a range changing only its window;
//! Mimir's answers become lines on the window's points, a gap missing
//! points; Mimir down or silent is unavailable; an answer is kept 60 s.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

const NOW: u64 = 1_700_000_123;

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW)
}

#[test]
fn the_queries_are_fixed_strings_with_only_the_window_from_the_range() {
    let cpu = |w: &str| {
        format!(
            r#"100 * (1 - avg by (vm) (rate(node_cpu_seconds_total{{job="host",mode="idle"}}[{w}])))"#
        )
    };
    let memory = r#"100 * (1 - sum by (vm) (node_memory_MemAvailable_bytes{job="host"}) / sum by (vm) (node_memory_MemTotal_bytes{job="host"}))"#;
    let filesystems = r#"100 * (1 - max by (vm, mountpoint) (node_filesystem_avail_bytes{job="host"}) / max by (vm, mountpoint) (node_filesystem_size_bytes{job="host"}))"#;
    let network = |w: &str| {
        format!(
            r#"sum by (vm, direction) (label_replace(rate(node_network_receive_bytes_total{{job="host"}}[{w}]), "direction", "in", "", "") or label_replace(rate(node_network_transmit_bytes_total{{job="host"}}[{w}]), "direction", "out", "", ""))"#
        )
    };
    let last = |r: &str| {
        format!(
            r#"max by (vm) (max_over_time(timestamp(node_boot_time_seconds{{job="host"}})[{r}:1m]))"#
        )
    };
    for (range, w) in [
        (SeriesRange::Hour, "2m"),
        (SeriesRange::Day, "10m"),
        (SeriesRange::Week, "1h"),
    ] {
        let expected = [
            cpu(w),
            memory.to_string(),
            filesystems.to_string(),
            network(w),
        ];
        assert_eq!(queries::ranged(range), expected, "{range:?}");
        assert_eq!(queries::last_sample(range), last(range.as_str()));
        for q in queries::ranged(range) {
            assert!(!q.contains('$'), "nothing left to fill: {q}");
        }
    }
}

fn matrix(result: Value) -> Value {
    json!({ "status": "success", "data": { "resultType": "matrix", "result": result } })
}

#[test]
fn an_answer_is_lines_on_the_windows_points_a_gap_missing_points() {
    let w = SeriesRange::Day.window(now());
    let (a, b) = (w.start, w.start + 600);
    let answer = matrix(json!([
        { "metric": { "vm": "worker-1", "mountpoint": "spill" }, "values": [[a, "10"], [b, "NaN"]] },
        { "metric": { "vm": "worker-1", "mountpoint": "docker" },
          "values": [[a, "1.5"], [a + 1200, "2.5"], [a + 7, "9"], [w.end + 600, "9"], [w.end, "+Inf"]] },
        { "metric": { "mountpoint": "no-vm" }, "values": [[a, "1"]] },
    ]));
    let lines = queries::lines(&answer, &w, Some("mountpoint")).unwrap();
    assert_eq!(lines.len(), 2, "a series with no vm is not a machine's");
    assert_eq!(
        (lines[0].vm.as_str(), lines[0].label.as_str()),
        ("worker-1", "docker")
    );
    assert_eq!(lines[0].values.len(), 144);
    assert_eq!(&lines[0].values[..4], &[Some(1.5), None, Some(2.5), None]);
    assert_eq!(lines[0].values[143], None, "not a finite number");
    assert_eq!(
        lines[0].values.iter().flatten().count(),
        2,
        "off-step and outside points dropped"
    );
    assert_eq!(&lines[1].values[..2], &[Some(10.0), None]);

    let error = json!({ "status": "error", "error": "bad" });
    assert_eq!(queries::lines(&error, &w, None), None);
    let vector = json!({ "status": "success", "data": { "resultType": "vector", "result": [] } });
    assert_eq!(queries::lines(&vector, &w, None), None);
    let unlabelled = matrix(json!([{ "metric": { "vm": "x" }, "values": [] }]));
    assert_eq!(queries::lines(&unlabelled, &w, Some("direction")), None);
    let last = json!({ "status": "success", "data": { "resultType": "vector", "result": [
        { "metric": { "vm": "worker-2" }, "value": [NOW, "1699999000.25"] },
        { "metric": { "vm": "worker-1" }, "value": [NOW, "1699998000"] },
    ] } });
    let got = queries::last_samples(&last).unwrap();
    assert_eq!(
        got,
        [
            ("worker-1".into(), 1_699_998_000),
            ("worker-2".into(), 1_699_999_000)
        ]
    );
}

/// A Mimir that answers `query_range` with `ranged` and `query` with a
/// vector naming `worker-1`, and counts the requests; or, when `silent`,
/// accepts and never answers.
async fn mimir(ranged: Value, silent: bool) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let count = asked.clone();
    let instant = json!({ "status": "success", "data": { "resultType": "vector", "result": [
        { "metric": { "vm": "worker-1" }, "value": [NOW, "1699999990"] } ] } });
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (ranged, instant, count) = (ranged.to_string(), instant.to_string(), count.clone());
            tokio::spawn(async move {
                let mut buf = vec![0; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                count.fetch_add(1, Ordering::SeqCst);
                if silent {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = "GET /prometheus/api/v1/query_range?";
                let body = if request.starts_with(path) {
                    ranged
                } else {
                    instant
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(body.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}/prometheus/"), asked)
}

fn series(base: &str) -> MimirSeries {
    MimirSeries::new(base, LakeClient::from(reqwest::Client::new()))
}

#[tokio::test]
async fn five_queries_make_the_charts_kept_sixty_seconds_per_range() {
    let w = SeriesRange::Hour.window(now());
    let ranged = matrix(json!([
        { "metric": { "vm": "worker-1", "mountpoint": "docker", "direction": "in" },
          "values": [[w.start, "12.5"]] },
    ]));
    let (base, asked) = mimir(ranged, false).await;
    let s = series(&base);
    let charts = s.read(SeriesRange::Hour, now()).await.unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 5, "five queries a page");
    assert_eq!(charts.window, w);
    for lines in [
        &charts.cpu,
        &charts.memory,
        &charts.filesystems,
        &charts.network,
    ] {
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (lines[0].values.len(), lines[0].values[0]),
            (60, Some(12.5))
        );
    }
    assert_eq!(charts.filesystems[0].label, "docker");
    assert_eq!(charts.network[0].label, "in");
    assert_eq!(charts.last_sample, [("worker-1".into(), 1_699_999_990)]);

    let later = now() + Duration::from_secs(59);
    assert_eq!(s.read(SeriesRange::Hour, later).await, Ok(charts));
    assert_eq!(asked.load(Ordering::SeqCst), 5, "kept: Mimir not asked");
    s.read(SeriesRange::Hour, now() + SERIES_CACHE)
        .await
        .unwrap();
    assert_eq!(
        asked.load(Ordering::SeqCst),
        10,
        "after 60 s Mimir is asked again"
    );
    s.read(SeriesRange::Day, later).await.unwrap();
    assert_eq!(
        asked.load(Ordering::SeqCst),
        15,
        "another range, another answer"
    );

    // An answer that is not Prometheus's is unavailable, and not kept.
    let (base, _) = mimir(json!({ "status": "error" }), false).await;
    let s = series(&base);
    assert!(s.read(SeriesRange::Day, now()).await.is_err());
    assert!(s.cache.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mimir_down_or_silent_is_unavailable_and_not_kept() {
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/prometheus", closed.local_addr().unwrap());
    drop(closed);
    let s = series(&base);
    assert!(s.read(SeriesRange::Day, now()).await.is_err());
    assert!(s.cache.lock().unwrap().is_empty(), "a failure is not kept");

    let (base, asked) = mimir(json!({}), true).await;
    let s = series(&base).with_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let got = s.read(SeriesRange::Day, now()).await;
    assert_eq!(got, Err(SeriesUnavailable("no answer in time".into())));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "the first query used the whole time"
    );
}
