//! An application module's work in flight, asked over HTTP (decision 0003):
//! a module whose manifest declares `drain: { path }` answers `GET
//! <path>?horizon_secs=N` on its internal service inside the Compose
//! network with `{"running": n, "scheduled_soon": n}` (`queued` optional).
//! Each module is asked at once, within one timeout; one that does not
//! answer, answers an error status or an answer that does not read is an
//! error, which the caller counts as not drained.

use std::time::Duration;

use serde::Deserialize;

/// Where a module answers: its name and the URL of its drain path on its
/// internal service (`http://<service>:<port><path>`).
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

/// Asks the modules' drain paths.
#[derive(Debug, Clone)]
pub struct HttpModuleDrains {
    client: reqwest::Client,
    endpoints: Vec<DrainEndpoint>,
}

impl HttpModuleDrains {
    /// Each module answers within `within`, or it did not answer.
    pub fn new(endpoints: Vec<DrainEndpoint>, within: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(within)
            .build()
            .map_err(|e| format!("drain client: {e}"))?;
        Ok(Self { client, endpoints })
    }

    /// Every module's answer, in the endpoints' order.
    pub async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleDrain, String>)> {
        let mut asks = tokio::task::JoinSet::new();
        for (i, e) in self.endpoints.iter().enumerate() {
            let (client, url) = (self.client.clone(), e.url.clone());
            asks.spawn(async move { (i, ask(&client, &url, horizon).await) });
        }
        let mut answers: Vec<_> = self
            .endpoints
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
) -> Result<ModuleDrain, String> {
    let secs = horizon.as_secs().to_string();
    let answer = client
        .get(url)
        .query(&[("horizon_secs", secs.as_str())])
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
