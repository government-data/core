//! Licencias: el permiso firmado de la nube, su verificación y dónde se guarda.
//!
//! Formato del permiso firmado (lo que responde la nube y se guarda tal cual):
//! `{"permiso": "<JSON del Entitlement en base64>", "firma": "<ed25519 en base64>"}`.
//! La firma cubre los bytes exactos del JSON. La llave pública de verificación
//! se compila dentro del binario (`ORQ_LICENSE_PUBKEY` al compilar); la privada
//! solo la tiene quien emite licencias.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;

use license_model::{Entitlement, FetchError, LicenseAuthority, LicenseStore, StoredLicense};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::SecretBox;

/// The **development** public key (its private key is not in this
/// repository; licenses are being removed). Customer builds carry their own
/// with `ORQ_LICENSE_PUBKEY`.
pub const DEV_PUBLIC_KEY: &str = "Td3r3l+dQNCiSjNfQyPnGI2tXzmJo0GRbtLzB8Hxtr4=";

/// La llave pública con la que este binario verifica los permisos.
pub fn compiled_public_key() -> &'static str {
    option_env!("ORQ_LICENSE_PUBKEY")
        .filter(|k| !k.trim().is_empty())
        .unwrap_or(DEV_PUBLIC_KEY)
}

/// Si este binario confía en la llave de desarrollo (no apto para clientes).
pub fn uses_dev_key() -> bool {
    compiled_public_key() == DEV_PUBLIC_KEY
}

#[derive(Serialize, Deserialize)]
struct Signed {
    permiso: String,
    firma: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Verificar un permiso firmado con `public_key` (32 bytes en base64).
pub fn verify_permit(signed: &str, public_key: &str) -> Result<Entitlement, String> {
    let signed: Signed =
        serde_json::from_str(signed).map_err(|e| format!("permiso ilegible: {e}"))?;
    let payload = b64()
        .decode(signed.permiso.as_bytes())
        .map_err(|_| "permiso ilegible (base64)".to_string())?;
    let signature = b64()
        .decode(signed.firma.as_bytes())
        .map_err(|_| "firma ilegible (base64)".to_string())?;
    let key = b64()
        .decode(public_key.as_bytes())
        .map_err(|_| "llave pública de licencias inválida".to_string())?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&payload, &signature)
        .map_err(|_| "la firma del permiso no es válida".to_string())?;
    serde_json::from_slice(&payload).map_err(|e| format!("permiso ilegible: {e}"))
}

/// Firmar un permiso con la llave privada (PKCS#8 en base64). Lo usa quien
/// emite licencias (la nube, o `server license-sign`).
pub fn sign_permit(entitlement: &Entitlement, private_key: &str) -> Result<String, String> {
    let pkcs8 = b64()
        .decode(private_key.trim().as_bytes())
        .map_err(|_| "llave privada ilegible (base64)".to_string())?;
    let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&pkcs8)
        .map_err(|_| "llave privada inválida".to_string())?;
    let payload = serde_json::to_vec(entitlement).map_err(|e| e.to_string())?;
    let signature = pair.sign(&payload);
    serde_json::to_string(&Signed {
        permiso: b64().encode(&payload),
        firma: b64().encode(signature.as_ref()),
    })
    .map_err(|e| e.to_string())
}

/// Un par de llaves nuevo: (privada PKCS#8, pública), ambas en base64.
pub fn generate_keypair() -> Result<(String, String), String> {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "no se pudo generar la llave".to_string())?;
    let pair =
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "llave inválida".to_string())?;
    Ok((
        b64().encode(pkcs8.as_ref()),
        b64().encode(pair.public_key().as_ref()),
    ))
}

/// La nube de licencias por HTTPS.
pub struct HttpLicenseAuthority {
    base: String,
    public_key: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for HttpLicenseAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpLicenseAuthority")
            .field("base", &self.base)
            .finish()
    }
}

impl HttpLicenseAuthority {
    pub fn new(base: &str, public_key: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .user_agent("orquestador-de-flujos")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            public_key: public_key.to_string(),
            http,
        })
    }
}

/// The platform's mail server, as the licensing cloud hands it out
/// (`POST /v1/mail`).
#[derive(Clone, serde::Deserialize)]
pub struct PlatformMail {
    pub host: String,
    pub port: u16,
    /// `starttls`, `tls` or `none`.
    pub tls: String,
    pub user: String,
    pub password: String,
    pub sender: String,
    pub name: String,
}

impl std::fmt::Debug for PlatformMail {
    // Without the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformMail")
            .field("host", &self.host)
            .field("sender", &self.sender)
            .finish_non_exhaustive()
    }
}

impl HttpLicenseAuthority {
    /// The platform mail for this online cell or managed instance; `None` if
    /// the cloud has none or it is not for this key (a client's installation
    /// uses its own).
    pub async fn platform_mail(
        &self,
        key: &str,
        installation: &str,
    ) -> Result<Option<PlatformMail>, String> {
        let response = self
            .http
            .post(format!("{}/v1/mail", self.base))
            .json(&serde_json::json!({ "key": key, "installation": installation }))
            .send()
            .await
            .map_err(|e| format!("no connection to the license cloud: {e}"))?;
        match response.status() {
            s if s.is_success() => response.json().await.map(Some).map_err(|e| e.to_string()),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(None),
            s => {
                let message = response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_default();
                Err(format!("the license cloud answered {s}: {message}"))
            }
        }
    }
}

/// Signing in with the common account, as the licensing cloud hands it to an
/// instance we administer (`POST /v1/account-oidc`): its `orquestador`
/// provider.
#[derive(Clone, serde::Deserialize)]
pub struct PlatformOidc {
    pub client_id: String,
    pub client_secret: String,
    pub issuer: String,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

impl std::fmt::Debug for PlatformOidc {
    // Without the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformOidc")
            .field("client_id", &self.client_id)
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl HttpLicenseAuthority {
    /// Signing in with the common account (a managed instance); `None` if
    /// the cloud does not have it or it is not for this key.
    pub async fn platform_oidc(
        &self,
        key: &str,
        installation: &str,
    ) -> Result<Option<PlatformOidc>, String> {
        let response = self
            .http
            .post(format!("{}/v1/account-oidc", self.base))
            .json(&serde_json::json!({ "key": key, "installation": installation }))
            .send()
            .await
            .map_err(|e| format!("no connection to the license cloud: {e}"))?;
        match response.status() {
            s if s.is_success() => response.json().await.map(Some).map_err(|e| e.to_string()),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(None),
            s => Err(format!("the license cloud answered {s}")),
        }
    }
}

impl HttpLicenseAuthority {
    /// The Turnstile keys (public and secret) for this online cell or managed
    /// instance; `None` if the cloud has none or they are not for it.
    pub async fn platform_captcha(
        &self,
        key: &str,
        installation: &str,
    ) -> Result<Option<(String, String)>, String> {
        let response = self
            .http
            .post(format!("{}/v1/captcha", self.base))
            .json(&serde_json::json!({ "key": key, "installation": installation }))
            .send()
            .await
            .map_err(|e| format!("no connection to the license cloud: {e}"))?;
        match response.status() {
            s if s.is_success() => {
                let v: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
                match (v["site"].as_str(), v["secret"].as_str()) {
                    (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
                        Ok(Some((a.to_string(), b.to_string())))
                    }
                    _ => Ok(None),
                }
            }
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::FORBIDDEN => Ok(None),
            s => Err(format!("the license cloud answered {s}")),
        }
    }
}

#[async_trait]
impl LicenseAuthority for HttpLicenseAuthority {
    async fn fetch(
        &self,
        key: &str,
        installation: &str,
        usage: Option<&license_model::Usage>,
    ) -> Result<String, FetchError> {
        let mut body = serde_json::json!({ "key": key, "installation": installation });
        if let Some(usage) = usage {
            body["usage"] = serde_json::json!({
                "month": chrono::Utc::now().format("%Y-%m").to_string(),
                "executions": usage.executions_month,
                "seconds": usage.execution_secs_month,
                "servers": usage.servers,
                "workflows": usage.workflows,
                "users": usage.users,
                "deployments": usage.deployments_month,
            });
        }
        let response = self
            .http
            .post(format!("{}/v1/permits", self.base))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                FetchError::Unreachable(format!("no hay conexión con la nube de licencias: {e}"))
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| FetchError::Unreachable(e.to_string()))?;
        if !status.is_success() {
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("respondió {status}"));
            // 403/409: the cloud said no. The rest (5xx, proxies) is a
            // failure on the way, not a decision about the key.
            return Err(match status.as_u16() {
                401 | 403 | 404 | 409 => {
                    FetchError::Rejected(format!("la nube rechazó la clave: {message}"))
                }
                _ => FetchError::Unreachable(format!("la nube de licencias falló: {message}")),
            });
        }
        Ok(body)
    }

    fn verify(&self, signed: &str) -> Result<Entitlement, String> {
        verify_permit(signed, &self.public_key)
    }

    async fn billing(
        &self,
        key: &str,
        installation: &str,
    ) -> Result<license_model::Billing, String> {
        let response = self
            .http
            .post(format!("{}/v1/billing", self.base))
            .json(&serde_json::json!({ "key": key, "installation": installation }))
            .send()
            .await
            .map_err(|e| format!("no hay conexión con la nube de licencias: {e}"))?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("la nube respondió {status}: {e}"))?;
        if !status.is_success() {
            return Err(body["message"]
                .as_str()
                .map_or_else(|| format!("la nube respondió {status}"), str::to_string));
        }
        serde_json::from_value::<CloudBilling>(body)
            .map(Into::into)
            .map_err(|e| format!("la nube respondió algo que no se entiende: {e}"))
    }

    async fn heartbeat(
        &self,
        key: &str,
        installation: &str,
        notify: &[String],
    ) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/v1/heartbeats", self.base))
            .json(&serde_json::json!({
                "key": key,
                "installation": installation,
                "notify": notify,
            }))
            .send()
            .await
            .map_err(|e| format!("no hay conexión con la nube de licencias: {e}"))?;
        match response.status() {
            s if s.is_success() => Ok(()),
            s => Err(format!("la nube respondió {s} al latido")),
        }
    }
}

/// `POST /v1/billing`, as the cloud answers it.
#[derive(Deserialize)]
struct CloudBilling {
    period: Option<String>,
    #[serde(default)]
    packages: std::collections::BTreeMap<String, u32>,
    renewal_cop: Option<i64>,
    #[serde(default)]
    payments: Vec<CloudPayment>,
}

#[derive(Deserialize)]
struct CloudPayment {
    invoice: String,
    paid_on: chrono::NaiveDate,
    amount_cop: i64,
    period: String,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
}

impl From<CloudBilling> for license_model::Billing {
    fn from(f: CloudBilling) -> Self {
        Self {
            period: f.period,
            packages: f.packages,
            renewal_cop: f.renewal_cop,
            payments: f
                .payments
                .into_iter()
                .map(|p| license_model::BillingPayment {
                    invoice: p.invoice,
                    paid_on: p.paid_on,
                    amount_cop: p.amount_cop,
                    period: p.period,
                    from: p.from,
                    to: p.to,
                })
                .collect(),
        }
    }
}

/// La licencia en Postgres (una sola fila), con la clave cifrada.
#[derive(Clone)]
pub struct PgLicenseStore {
    pool: PgPool,
    secrets: Arc<SecretBox>,
}

impl std::fmt::Debug for PgLicenseStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgLicenseStore").finish_non_exhaustive()
    }
}

impl PgLicenseStore {
    pub fn new(pool: PgPool, secrets: Arc<SecretBox>) -> Self {
        Self { pool, secrets }
    }
}

#[async_trait]
impl LicenseStore for PgLicenseStore {
    async fn load(&self) -> Result<StoredLicense, String> {
        let row = sqlx::query(
            "SELECT key_sealed, permit, fetched_at, last_attempt_at, last_error FROM orq_license",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(StoredLicense::default());
        };
        let sealed: Option<String> = row.try_get("key_sealed").map_err(|e| e.to_string())?;
        Ok(StoredLicense {
            key: sealed.map(|s| self.secrets.decrypt(&s)).transpose()?,
            permit: row.try_get("permit").map_err(|e| e.to_string())?,
            fetched_at: row.try_get("fetched_at").map_err(|e| e.to_string())?,
            last_attempt_at: row.try_get("last_attempt_at").map_err(|e| e.to_string())?,
            last_error: row.try_get("last_error").map_err(|e| e.to_string())?,
        })
    }

    async fn save_key(&self, key: &str) -> Result<(), String> {
        let sealed = self.secrets.encrypt(key)?;
        sqlx::query(
            "INSERT INTO orq_license (key_sealed) VALUES ($1)
             ON CONFLICT (tenant_id) DO UPDATE SET key_sealed = $1, permit = NULL, fetched_at = NULL",
        )
        .bind(sealed)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn save_permit(&self, permit: &str) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_license (permit, fetched_at, last_attempt_at, last_error)
             VALUES ($1, now(), now(), NULL)
             ON CONFLICT (tenant_id) DO UPDATE
                SET permit = $1, fetched_at = now(), last_attempt_at = now(), last_error = NULL",
        )
        .bind(permit)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn clear_permit(&self, error: &str) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_license (last_attempt_at, last_error) VALUES (now(), $1)
             ON CONFLICT (tenant_id) DO UPDATE
                SET permit = NULL, fetched_at = NULL, last_attempt_at = now(), last_error = $1",
        )
        .bind(error)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn reminders_sent(&self) -> Result<Vec<String>, String> {
        let sent: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT reminders FROM orq_license")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        Ok(sent
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default())
    }

    async fn mark_reminders(&self, sent: &[String]) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_license (reminders) VALUES ($1)
             ON CONFLICT (tenant_id) DO UPDATE SET reminders = $1",
        )
        .bind(serde_json::json!(sent))
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn record_error(&self, error: &str) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO orq_license (last_attempt_at, last_error) VALUES (now(), $1)
             ON CONFLICT (tenant_id) DO UPDATE SET last_attempt_at = now(), last_error = $1",
        )
        .bind(error)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn entitlement() -> Entitlement {
        Entitlement {
            customer: "Empresa".into(),
            plan: "Profesional".into(),
            features: vec!["ssh".into()],
            limits: license_model::Limits::default(),
            installation: "i-1".into(),
            issued_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(30),
            contract_ends_at: None,
            suspended: None,
        }
    }

    #[test]
    fn signs_and_verifies_and_rejects_tampering() {
        let (private, public) = generate_keypair().unwrap();
        let signed = sign_permit(&entitlement(), &private).unwrap();
        assert_eq!(
            verify_permit(&signed, &public).unwrap(),
            entitlement_like(&signed, &public)
        );

        // Darse más: cambiar el plan dentro del permiso rompe la firma.
        let mut forged: Signed = serde_json::from_str(&signed).unwrap();
        let payload = String::from_utf8(b64().decode(&forged.permiso).unwrap()).unwrap();
        forged.permiso = b64().encode(payload.replace("Profesional", "Empresa"));
        let forged = serde_json::to_string(&forged).unwrap();
        assert!(verify_permit(&forged, &public).is_err());

        // Firmado con otra llave: no vale.
        let (other, _) = generate_keypair().unwrap();
        let foreign = sign_permit(&entitlement(), &other).unwrap();
        assert!(verify_permit(&foreign, &public).is_err());
    }

    fn entitlement_like(signed: &str, public: &str) -> Entitlement {
        verify_permit(signed, public).unwrap()
    }
}
