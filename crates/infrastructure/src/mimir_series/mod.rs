//! The machines' series from Mimir (decision 0072, point 13, step 7): the
//! five fixed queries of [`queries`], each for every machine at once, on
//! Mimir's Prometheus API (`MIMIR_QUERY_URL`, e.g.
//! `https://<INTERNAL_DOMAIN>:9009/prometheus`: the application VM's
//! Traefik, which lets only role `app` read, 0071). The API presents its
//! VM's `machine.pem` with the lake's client ([`LakeClient`]: `LAKE_CA_FILE`,
//! `LAKE_CLIENT_CERT_FILE`, read again when renewed); in a development
//! without TLS the URL is plain HTTP and no certificate is needed.
//!
//! All five within 5 seconds, or none: a page gets every chart or "metrics
//! not available". An answer is kept 60 seconds per range in this process;
//! a failure is not kept.

mod queries;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use auth_gateway::{MachineCharts, MachineSeries, SeriesRange, SeriesUnavailable, SeriesWindow};
use serde_json::Value;

use crate::lake_http::LakeClient;

/// How long the five queries may take together.
pub const MIMIR_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an answer is kept per range.
pub const SERIES_CACHE: Duration = Duration::from_secs(60);

/// The machines' series, read from Mimir.
pub struct MimirSeries {
    /// Mimir's Prometheus API, without a trailing `/`.
    base: String,
    http: LakeClient,
    timeout: Duration,
    cache: Mutex<HashMap<SeriesRange, (SystemTime, MachineCharts)>>,
}

impl std::fmt::Debug for MimirSeries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MimirSeries")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl MimirSeries {
    pub fn new(base: &str, http: LakeClient) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http,
            timeout: MIMIR_TIMEOUT,
            cache: Mutex::default(),
        }
    }

    /// Over plain HTTP or the system's roots, with no certificate: for a
    /// development without TLS, and tests.
    pub fn plain(base: &str) -> Self {
        Self::new(base, LakeClient::from(reqwest::Client::new()))
    }

    /// From `MIMIR_QUERY_URL`, with the lake's client of the environment;
    /// `Ok(None)` when the variable is unset or empty.
    pub fn from_env() -> Result<Option<Self>, String> {
        let url = std::env::var("MIMIR_QUERY_URL").ok();
        let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
            return Ok(None);
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("MIMIR_QUERY_URL: not an http(s) URL".into());
        }
        let http = LakeClient::from_env(MIMIR_TIMEOUT)?;
        Ok(Some(Self::new(url.trim(), http)))
    }

    /// Another time limit, for tests.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn cached(&self, range: SeriesRange, now: SystemTime) -> Option<MachineCharts> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let (at, charts) = cache.get(&range)?;
        let age = now.duration_since(*at).ok()?;
        (age < SERIES_CACHE).then(|| charts.clone())
    }

    async fn ask(
        &self,
        deadline: Instant,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<Value, SeriesUnavailable> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SeriesUnavailable("no answer in time".into()));
        }
        let url = format!("{}{path}", self.base);
        let sent = self.http.get().get(&url).query(params).timeout(left).send();
        let answer = sent.await.map_err(|e| unavailable(&e))?;
        let status = answer.status();
        if !status.is_success() {
            return Err(SeriesUnavailable(format!("Mimir answered {status}")));
        }
        answer.json::<Value>().await.map_err(|e| unavailable(&e))
    }

    async fn fetch(&self, window: SeriesWindow) -> Result<MachineCharts, SeriesUnavailable> {
        let deadline = Instant::now() + self.timeout;
        let range = window.range;
        let span = [
            ("start", window.start.to_string()),
            ("end", window.end.to_string()),
            ("step", range.step().to_string()),
        ];
        let labels = [None, None, Some("mountpoint"), Some("direction")];
        let mut charts = Vec::with_capacity(4);
        for (query, label) in queries::ranged(range).into_iter().zip(labels) {
            let mut params = vec![("query", query)];
            params.extend(span.iter().cloned());
            let answer = self.ask(deadline, "/api/v1/query_range", &params).await?;
            charts.push(queries::lines(&answer, &window, label).ok_or_else(not_prometheus)?);
        }
        let params = [
            ("query", queries::last_sample(range)),
            ("time", window.end.to_string()),
        ];
        let answer = self.ask(deadline, "/api/v1/query", &params).await?;
        let last_sample = queries::last_samples(&answer).ok_or_else(not_prometheus)?;
        let [cpu, memory, filesystems, network]: [_; 4] =
            charts.try_into().map_err(|_| not_prometheus())?;
        Ok(MachineCharts {
            window,
            cpu,
            memory,
            filesystems,
            network,
            last_sample,
        })
    }
}

#[async_trait]
impl MachineSeries for MimirSeries {
    async fn read(
        &self,
        range: SeriesRange,
        now: SystemTime,
    ) -> Result<MachineCharts, SeriesUnavailable> {
        if let Some(charts) = self.cached(range, now) {
            return Ok(charts);
        }
        let charts = self.fetch(range.window(now)).await?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(range, (now, charts.clone()));
        Ok(charts)
    }
}

/// The error without the request: its URL holds nothing secret, but the
/// kind is what a log needs.
fn unavailable(e: &reqwest::Error) -> SeriesUnavailable {
    let kind = if e.is_timeout() {
        "no answer in time"
    } else if e.is_connect() {
        "not reached"
    } else if e.is_decode() {
        "not Prometheus's answer"
    } else {
        "request failed"
    };
    SeriesUnavailable(kind.into())
}

fn not_prometheus() -> SeriesUnavailable {
    SeriesUnavailable("not Prometheus's answer".into())
}

#[cfg(test)]
mod tests;
