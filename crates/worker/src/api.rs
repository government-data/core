//! The API as a worker sees it (decisions 0018, 0023): a machine token of
//! the installation's OIDC provider for the audience `quality-api`, then a
//! job's details and its report.
//!
//! The token is asked with the client's secret (development, until the dev
//! stack moves to certificates), or with the machine's certificate and no
//! secret (decision 0066, step 7): then every call to the API presents it,
//! on the internal entry point, through [`MachineHttp`], which follows the
//! files the node agent renews.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use quality::lake::LakeAccess;
use quality::worker_api::{JobDetails, AUDIENCE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::machine::MachineHttp;

type Failure = Box<dyn std::error::Error + Send + Sync>;

/// The client and how it proves who it is.
#[derive(Debug)]
pub(crate) struct Client {
    pub(crate) id: String,
    pub(crate) auth: Auth,
}

/// A secret (`client_secret_basic`), or the machine's certificate, with the
/// machine's name. The secret never shows in `Debug`.
pub(crate) enum Auth {
    Secret(String),
    Certificate { http: MachineHttp, machine: String },
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Secret(_) => f.write_str("Secret(..)"),
            Self::Certificate { machine, .. } => f
                .debug_struct("Certificate")
                .field("machine", machine)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Api {
    /// The lake's client, and the API's with a secret.
    http: reqwest::Client,
    base: String,
    client: Client,
    /// A live token per audience, and when to ask again.
    tokens: Mutex<HashMap<&'static str, (String, Instant)>>,
    /// Where this worker reaches the lake, when not where a job says.
    lake_url: Option<String>,
}

/// A job's lake as this worker reaches it: at `url` when it has one, its
/// keys and warehouse as they came.
fn reached_at(url: Option<&str>, mut lake: LakeAccess) -> LakeAccess {
    if let Some(url) = url {
        lake.endpoint = url.to_string();
    }
    lake
}

/// The header the API takes as its request's id (`http-kit`'s telemetry:
/// up to 128 ASCII characters, kept as given).
const REQUEST_ID: &str = "x-request-id";

/// The request id of a call for job `id`: the job's id, so one search for
/// it finds the API's lines of every call of the job, then `.` and a part
/// of this call's own (the time in nanoseconds mixed with a counter, as
/// `http-kit` makes its ids), so each call keeps one of its own. 53
/// characters, none that ends the id in the Loki datasource's link.
fn request_id(id: Uuid) -> String {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    format!("{id}.{:016x}", nanos ^ n.rotate_left(32))
}

/// The API gave no details and said to come back (503): something the API
/// needs did not answer, and the job went back to the queue.
#[derive(Debug)]
pub(crate) struct NotNow;

impl std::fmt::Display for NotNow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the API cannot give the job now (503)")
    }
}

impl std::error::Error for NotNow {}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl Api {
    /// `base`: the API's address, `https://<domain>/api` with a secret, the
    /// internal entry point's with a certificate.
    pub(crate) fn new(http: reqwest::Client, base: &str, client: Client) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            client,
            tokens: Mutex::new(HashMap::new()),
            lake_url: None,
        }
    }

    /// With the lake reached at `url` (`QD_LAKE_URL`; decision 0066, point
    /// 10): the proxy on this worker's loopback, which presents the
    /// machine's certificate to the lake's gate, as DuckDB cannot. Every
    /// job's lake is reached there, whatever address came with it.
    pub(crate) fn with_lake(mut self, url: String) -> Self {
        self.lake_url = Some(url);
        self
    }

    /// The lake's HTTP client, which trusts `QD_CA_FILE` and presents no
    /// certificate: the proxy of [`Api::with_lake`] does.
    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// What this worker's slots are named after: `<client id>` with a
    /// secret, `<client id>/<machine>` with a certificate, as a role's
    /// client is shared by its machines. The API gives a job only to the
    /// worker whose name starts with it and a `/`.
    pub(crate) fn worker(&self) -> String {
        match &self.client.auth {
            Auth::Secret(_) => self.client.id.clone(),
            Auth::Certificate { machine, .. } => format!("{}/{machine}", self.client.id),
        }
    }

    /// The client for the API: with a certificate, the machine's as its
    /// files are now.
    fn api(&self) -> Result<reqwest::Client, Failure> {
        match &self.client.auth {
            Auth::Secret(_) => Ok(self.http.clone()),
            Auth::Certificate { http, .. } => http.client(),
        }
    }

    /// Whether a token for the API can be had now (the last one if live):
    /// a machine not approved, or retired, gets none, and takes no job.
    pub(crate) async fn ready(&self) -> Result<(), Failure> {
        self.token().await.map(drop)
    }

    /// A live token for the API.
    async fn token(&self) -> Result<String, Failure> {
        self.token_for(AUDIENCE).await
    }

    /// A live token for `audience`, asked again a minute before it expires.
    async fn token_for(&self, audience: &'static str) -> Result<String, Failure> {
        let mut cached = self.tokens.lock().await;
        if let Some((token, until)) = cached.get(audience) {
            if Instant::now() < *until {
                return Ok(token.clone());
            }
        }
        let mut sent = self.ask_token(audience).await?;
        if sent.status() == StatusCode::UNAUTHORIZED
            && matches!(self.client.auth, Auth::Certificate { .. })
        {
            // Renewed, and presented elsewhere, since it was read: the
            // files are read again and it is asked once more.
            tracing::info!("token refused: asking again with the certificate's files read again");
            sent = self.ask_token(audience).await?;
        }
        let res: TokenResponse = sent.error_for_status()?.json().await?;
        let until = Instant::now() + Duration::from_secs(res.expires_in.saturating_sub(60));
        cached.insert(audience, (res.access_token.clone(), until));
        Ok(res.access_token)
    }

    /// `POST /oauth/token`, `client_credentials`: the secret in Basic, or
    /// no secret and the machine's certificate (RFC 8705).
    async fn ask_token(&self, audience: &str) -> Result<reqwest::Response, Failure> {
        let url = format!("{}/oauth/token", self.base);
        let grant = ("grant_type", "client_credentials");
        let request = match &self.client.auth {
            Auth::Secret(secret) => self
                .http
                .post(url)
                .basic_auth(&self.client.id, Some(secret))
                .form(&[grant, ("audience", audience)]),
            Auth::Certificate { http, .. } => http.client()?.post(url).form(&[
                grant,
                ("client_id", self.client.id.as_str()),
                ("audience", audience),
            ]),
        };
        Ok(request.send().await?)
    }

    /// `GET` a route of a job this worker holds: `/internal/jobs/<id><tail>`.
    async fn job_get<T: serde::de::DeserializeOwned>(
        &self,
        id: Uuid,
        tail: &str,
    ) -> Result<T, Failure> {
        let token = self.token().await?;
        Ok(self
            .api()?
            .get(format!("{}/internal/jobs/{id}{tail}", self.base))
            .header(REQUEST_ID, request_id(id))
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub(crate) async fn details(&self, id: Uuid) -> Result<JobDetails, Failure> {
        let got: Result<JobDetails, Failure> = self.job_get(id, "").await;
        let mut details = got.map_err(|e| match e.downcast_ref::<reqwest::Error>() {
            Some(http) if http.status() == Some(StatusCode::SERVICE_UNAVAILABLE) => NotNow.into(),
            _ => e,
        })?;
        details.lake = details
            .lake
            .map(|lake| reached_at(self.lake_url.as_deref(), lake));
        Ok(details)
    }

    /// An extraction's credentials, asked again once it created its table.
    pub(crate) async fn lake_again(&self, id: Uuid) -> Result<LakeAccess, Failure> {
        let lake = self.job_get(id, "/lake").await?;
        Ok(reached_at(self.lake_url.as_deref(), lake))
    }

    /// A job's report: an evaluation's or an extraction's.
    pub(crate) async fn report(&self, id: Uuid, report: &impl Serialize) -> Result<(), Failure> {
        let token = self.token().await?;
        self.api()?
            .post(format!("{}/internal/jobs/{id}/report", self.base))
            .header(REQUEST_ID, request_id(id))
            .bearer_auth(token)
            .json(report)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
