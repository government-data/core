//! An application module's work in flight, asked over HTTP (decision 0003):
//! a module whose manifest declares `drain: { path }` answers `GET
//! /modules/<name><path>?horizon_secs=N` on Traefik's internal entry point,
//! asked with the machine's certificate, with `{"running": n,
//! "scheduled_soon": n}` (`queued` optional).
//! Each module is asked at once, within one timeout; one that does not
//! answer, answers an error status or an answer that does not read is an
//! error, which the caller counts as not drained.

use std::time::Duration;

use serde::Deserialize;

use crate::lake_http::LakeClient;

/// Where a module answers: its name and the URL of its drain path
/// (`https://<INTERNAL_DOMAIN>:<port>/modules/<name><path>`, Traefik's
/// internal entry point).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainEndpoint {
    pub module: String,
    pub url: String,
}

/// A module's answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct ModuleDrain {
    pub running: u64,
    #[serde(default)]
    pub queued: u64,
    pub scheduled_soon: u64,
}

/// Asks the modules' drain paths, presenting the client's certificate
/// (the machine's, through Traefik's internal entry point).
#[derive(Debug, Clone)]
pub struct HttpModuleDrains {
    http: LakeClient,
    within: Duration,
}

impl HttpModuleDrains {
    /// Each module answers within `within`, or it did not answer.
    pub fn new(http: LakeClient, within: Duration) -> Self {
        Self { http, within }
    }

    /// With the machine's certificate of the environment
    /// ([`LakeClient::from_env`]: `LAKE_CA_FILE`, `LAKE_CLIENT_CERT_FILE`).
    pub fn from_env(within: Duration) -> Result<Self, String> {
        LakeClient::from_env(within).map(|http| Self::new(http, within))
    }

    /// Over plain HTTP or the system's roots, with no certificate: tests.
    pub fn plain(within: Duration) -> Self {
        Self::new(LakeClient::from(reqwest::Client::new()), within)
    }

    /// Every module's answer, in the endpoints' order.
    pub async fn work(
        &self,
        endpoints: &[DrainEndpoint],
        horizon: Duration,
    ) -> Vec<(String, Result<ModuleDrain, String>)> {
        let mut asks = tokio::task::JoinSet::new();
        let client = self.http.get();
        for (i, e) in endpoints.iter().enumerate() {
            let (client, url, within) = (client.clone(), e.url.clone(), self.within);
            asks.spawn(async move { (i, ask(&client, &url, horizon, within).await) });
        }
        let mut answers: Vec<_> = endpoints
            .iter()
            .map(|e| (e.module.clone(), Err("no answer".to_string())))
            .collect();
        while let Some(done) = asks.join_next().await {
            if let Ok((i, answer)) = done {
                answers[i].1 = answer;
            }
        }
        answers
    }
}

async fn ask(
    client: &reqwest::Client,
    url: &str,
    horizon: Duration,
    within: Duration,
) -> Result<ModuleDrain, String> {
    let secs = horizon.as_secs().to_string();
    let answer = client
        .get(url)
        .query(&[("horizon_secs", secs.as_str())])
        .timeout(within)
        .send()
        .await
        .map_err(|e| format!("no answer: {e}"))?;
    let status = answer.status();
    if !status.is_success() {
        return Err(format!("answered {status}"));
    }
    answer
        .json::<ModuleDrain>()
        .await
        .map_err(|e| format!("an answer that does not read: {e}"))
}
