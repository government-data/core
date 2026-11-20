//! [`MachineApi`], [`TrinoGate`] and Dolt's gate ([`DoltApi`]) over HTTPS,
//! with rustls, and Trino's [`TrinoInfo`]. Never an invalid certificate accepted: the join trusts the
//! public roots and the CA file it is given (`QD_CA_FILE`, which `join.sh`
//! writes after checking it); the renewal and Trino's hold trust the
//! installation's CA alone and present the machine's certificate.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Certificate, Client, Identity, Response};
use serde_json::json;

use crate::api::{answer, classify, CallError, Issued, MachineApi};
use crate::cert::Credentials;
use crate::dolt::{self, DoltGate};
use crate::restart::Release;
use crate::trino::{read_info, HoldAnswer, Info, TrinoGate, TrinoInfo};

const TIMEOUT: Duration = Duration::from_secs(30);

/// The API at its two addresses.
#[derive(Debug)]
pub struct HttpApi {
    /// The public entry point's API, `https://<domain>/api`.
    public_url: String,
    /// The internal one's, `https://<internal domain>:9443/api`.
    internal_url: String,
    /// The CA file trusted for the join, PEM.
    join_ca: Option<Vec<u8>>,
}

impl HttpApi {
    pub fn new(public_url: &str, internal_url: &str, join_ca: Option<Vec<u8>>) -> Self {
        Self {
            public_url: public_url.trim_end_matches('/').to_string(),
            internal_url: internal_url.trim_end_matches('/').to_string(),
            join_ca,
        }
    }

    fn join_client(&self) -> Result<Client, CallError> {
        let mut client = Client::builder().timeout(TIMEOUT).https_only(true);
        if let Some(ca) = &self.join_ca {
            client = client.add_root_certificate(Certificate::from_pem(ca).map_err(local)?);
        }
        client.build().map_err(local)
    }
}

#[async_trait]
impl MachineApi for HttpApi {
    async fn join(&self, token: &str, name: &str, csr: &str) -> Result<Issued, CallError> {
        let url = format!("{}/install/join", self.public_url);
        let body = json!({ "token": token, "name": name, "csr": csr });
        let sent = self.join_client()?.post(url).json(&body).send().await;
        let (status, retry_after, body) = read(sent).await?;
        answer(status, retry_after, &body)
    }

    async fn renew(
        &self,
        identity: &Credentials,
        ca: &str,
        csr: &str,
    ) -> Result<Issued, CallError> {
        let url = format!("{}/internal/machines/renew", self.internal_url);
        let sent = machine_client(identity, ca)?
            .post(url)
            .json(&json!({ "csr": csr }))
            .send()
            .await;
        let (status, retry_after, body) = read(sent).await?;
        answer(status, retry_after, &body)
    }
}

#[async_trait]
impl TrinoGate for HttpApi {
    async fn hold(&self, identity: &Credentials, ca: &str) -> Result<HoldAnswer, CallError> {
        self.hold_of("trino", identity, ca).await
    }

    async fn release(&self, identity: &Credentials, ca: &str) -> Result<bool, CallError> {
        self.release_of("trino", identity, ca).await
    }
}

impl HttpApi {
    /// `POST /internal/machines/<service>/hold`, its answer read as `T`.
    async fn hold_of<T: serde::de::DeserializeOwned>(
        &self,
        service: &str,
        identity: &Credentials,
        ca: &str,
    ) -> Result<T, CallError> {
        let url = format!("{}/internal/machines/{service}/hold", self.internal_url);
        let sent = machine_client(identity, ca)?.post(url).send().await;
        let (status, retry_after, body) = read(sent).await?;
        success(status, retry_after)?;
        serde_json::from_slice(&body).map_err(|_| CallError::Unavailable(status))
    }

    /// `POST /internal/machines/<service>/release`: whether one was in force.
    async fn release_of(
        &self,
        service: &str,
        identity: &Credentials,
        ca: &str,
    ) -> Result<bool, CallError> {
        let url = format!("{}/internal/machines/{service}/release", self.internal_url);
        let sent = machine_client(identity, ca)?.post(url).send().await;
        let (status, retry_after, body) = read(sent).await?;
        success(status, retry_after)?;
        let released: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| CallError::Unavailable(status))?;
        Ok(released["released"].as_bool().unwrap_or(false))
    }
}

/// Dolt's hold and release on the API (0066, step 10).
#[derive(Debug)]
pub struct DoltApi<'a>(pub &'a HttpApi);

#[async_trait]
impl Release for DoltApi<'_> {
    async fn release(&self, identity: &Credentials, ca: &str) -> Result<bool, CallError> {
        self.0.release_of("dolt", identity, ca).await
    }
}

#[async_trait]
impl DoltGate for DoltApi<'_> {
    async fn hold(&self, identity: &Credentials, ca: &str) -> Result<dolt::HoldAnswer, CallError> {
        self.0.hold_of("dolt", identity, ca).await
    }
}

/// A client presenting the machine's certificate and trusting the
/// installation's CA alone, as renewing does.
fn machine_client(identity: &Credentials, ca: &str) -> Result<Client, CallError> {
    let identity = Identity::from_pem(identity.bundle().as_bytes()).map_err(local)?;
    Client::builder()
        .timeout(TIMEOUT)
        .https_only(true)
        .tls_built_in_root_certs(false)
        .add_root_certificate(Certificate::from_pem(ca.as_bytes()).map_err(local)?)
        .identity(identity)
        .build()
        .map_err(local)
}

/// Trino's `/v1/info` at `url`, which needs no token: over HTTPS trusting
/// the installation's CA (the coordinator serves the machine's certificate),
/// or plain HTTP inside its host.
#[derive(Debug)]
pub struct HttpTrinoInfo {
    url: String,
    client: Client,
}

impl HttpTrinoInfo {
    pub fn new(url: &str, ca: Option<&str>) -> Result<Self, String> {
        let mut client = Client::builder().timeout(Duration::from_secs(5));
        if let Some(ca) = ca {
            let ca = Certificate::from_pem(ca.as_bytes()).map_err(|e| e.to_string())?;
            client = client
                .tls_built_in_root_certs(false)
                .add_root_certificate(ca);
        }
        Ok(Self {
            url: url.to_string(),
            client: client.build().map_err(|e| e.to_string())?,
        })
    }
}

#[async_trait]
impl TrinoInfo for HttpTrinoInfo {
    async fn info(&self) -> Result<Info, String> {
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        read_info(&body).ok_or_else(|| "not Trino's /v1/info".into())
    }
}

/// A success status, or what the other means.
fn success(status: u16, retry_after: Option<u64>) -> Result<(), CallError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(classify(status, retry_after))
    }
}

async fn read(sent: reqwest::Result<Response>) -> Result<(u16, Option<u64>, Vec<u8>), CallError> {
    let response = sent.map_err(network)?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let body = response.bytes().await.map_err(network)?;
    Ok((status, retry_after, body.to_vec()))
}

/// The error and its causes, as a TLS refusal hides in them.
fn network(e: reqwest::Error) -> CallError {
    let mut text = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        text = format!("{text}: {c}");
        cause = c.source();
    }
    CallError::Network(text)
}

/// A client that cannot be built: the files it was given are bad.
fn local(e: reqwest::Error) -> CallError {
    CallError::Local(e.to_string())
}
