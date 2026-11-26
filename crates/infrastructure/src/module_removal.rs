//! Whether a module may be removed, asked over HTTP (quality-sdk decision
//! 0015): a module whose manifest declares `removal: { path }` answers `GET
//! /modules/<name><path>?module=<removed>` on Traefik's internal entry
//! point, asked with the machine's certificate, as its drain is asked
//! ([`crate::module_drain`]). Every module is asked at once, within one
//! timeout; what does not answer, answers an error status or does not read
//! is an error, which the use case counts as a refusal.

use std::time::Duration;

use application::{RemovalAnswer, RemovalChecks, RemovalEndpoint};
use async_trait::async_trait;

use crate::lake_http::LakeClient;

/// Asks the modules' removal paths at `<base>/modules/<name><path>`.
#[derive(Debug, Clone)]
pub struct HttpModuleRemovals {
    http: LakeClient,
    base: String,
    within: Duration,
}

impl HttpModuleRemovals {
    pub fn new(http: LakeClient, base: &str, within: Duration) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            within,
        }
    }

    /// With the machine's certificate of the environment
    /// ([`LakeClient::from_env`]), at `https://<INTERNAL_DOMAIN>:<TRAEFIK_INTERNAL_PORT>`
    /// (`internal.localhost` and 443 by default, as Traefik's).
    pub fn from_env(within: Duration) -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let domain = var("INTERNAL_DOMAIN").unwrap_or_else(|| "internal.localhost".into());
        let port = var("TRAEFIK_INTERNAL_PORT").unwrap_or_else(|| "443".into());
        let base = format!("https://{}:{}", domain.trim(), port.trim());
        Ok(Self::new(LakeClient::from_env(within)?, &base, within))
    }

    /// Over plain HTTP or the system's roots, with no certificate: tests.
    pub fn plain(base: &str, within: Duration) -> Self {
        Self::new(LakeClient::from(reqwest::Client::new()), base, within)
    }
}

#[async_trait]
impl RemovalChecks for HttpModuleRemovals {
    async fn ask(
        &self,
        endpoints: &[RemovalEndpoint],
        removed: &str,
    ) -> Vec<(String, Result<RemovalAnswer, String>)> {
        let mut asks = tokio::task::JoinSet::new();
        let client = self.http.get();
        for (i, e) in endpoints.iter().enumerate() {
            let url = format!("{}/modules/{}{}", self.base, e.module, e.path);
            let (client, removed, within) = (client.clone(), removed.to_string(), self.within);
            asks.spawn(async move { (i, ask(&client, &url, &removed, within).await) });
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
    removed: &str,
    within: Duration,
) -> Result<RemovalAnswer, String> {
    let answer = client
        .get(url)
        .query(&[("module", removed)])
        .timeout(within)
        .send()
        .await
        .map_err(|e| format!("no answer: {e}"))?;
    let status = answer.status();
    if !status.is_success() {
        return Err(format!("answered {status}"));
    }
    answer
        .json::<RemovalAnswer>()
        .await
        .map_err(|e| format!("an answer that does not read: {e}"))
}
