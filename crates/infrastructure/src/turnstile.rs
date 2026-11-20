//! Checking a Cloudflare Turnstile token (`siteverify`).

use std::time::Duration;

/// Asks Turnstile whether a token is good.
#[derive(Debug, Clone)]
pub struct Turnstile {
    http: reqwest::Client,
}

const VERIFICAR: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

impl Turnstile {
    pub fn new() -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { http })
    }

    pub async fn verify(&self, secret: &str, token: &str, ip: &str) -> Result<bool, String> {
        let respuesta = self
            .http
            .post(VERIFICAR)
            .form(&[("secret", secret), ("response", token), ("remoteip", ip)])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let data: serde_json::Value = respuesta.json().await.map_err(|e| e.to_string())?;
        Ok(data["success"].as_bool().unwrap_or(false))
    }
}
