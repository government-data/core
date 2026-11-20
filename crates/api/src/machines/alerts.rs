//! `POST /internal/machines/alerts` (decision 0072, points 10 and 11, step
//! 5): Grafana's webhook, on Traefik's internal entry point, which requires
//! a certificate of the installation's CA and passes it in
//! `X-Forwarded-Tls-Client-Cert`. The certificate is the proof: only the
//! application VM's machine (role `app`) is taken, any other 403; no
//! session, no CSRF, no shared secret. It also works while the license is
//! read-only (`read_only.rs`), as the machines' other routes.
//!
//! 200 once the delivery's rows are committed; 4xx only for a body that is
//! not Grafana's (Grafana retries on 5xx). Neither the request nor the
//! certificate is logged. The alerts it leaves out (an unknown or retired
//! machine) are counted by [`UnknownAlerts`], which the server makes
//! `qd_machine_alerts_unknown_total`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use application::{AlertsError, DeliveredAlert, MachinesService};
use auth_gateway::http::client_cert::{ClientCertificateError, ClientCertificates};
use auth_gateway::MachineCertificate;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;

use super::{invalid, origin};
use crate::auth::ClientIp;
use crate::error::ApiError;

/// The path, as the API sees it (Traefik strips `/api`).
pub const ALERTS_PATH: &str = "/internal/machines/alerts";

/// A delivery's body, at most: Grafana sends one group, its alerts each a
/// few hundred bytes.
const ALERTS_BODY_LIMIT: usize = 1 << 20;

/// Counts the alerts a delivery left out; the server's metrics file holds
/// the counter.
#[derive(Clone)]
pub struct UnknownAlerts(Arc<dyn Fn(u64) + Send + Sync>);

impl std::fmt::Debug for UnknownAlerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnknownAlerts").finish_non_exhaustive()
    }
}

impl UnknownAlerts {
    pub fn new(count: impl Fn(u64) + Send + Sync + 'static) -> Self {
        Self(Arc::new(count))
    }

    /// Counted nowhere.
    pub fn uncounted() -> Self {
        Self::new(|_| {})
    }
}

/// Mount the route; `certificates` says whether the client certificate
/// header is trusted.
pub fn routes(
    machines: MachinesService,
    certificates: ClientCertificates,
    unknown: UnknownAlerts,
) -> Router {
    Router::new()
        .route(ALERTS_PATH, post(alerts_handler))
        .with_state(machines)
        .layer(Extension(certificates))
        .layer(Extension(unknown))
        .layer(DefaultBodyLimit::max(ALERTS_BODY_LIMIT))
}

/// Grafana's webhook body; only `alerts` is read.
#[derive(Debug, Deserialize, ToSchema)]
pub struct GrafanaDeliveryIn {
    alerts: Vec<GrafanaAlertIn>,
}

/// One alert, as Grafana's webhook sends it; its other fields are ignored.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrafanaAlertIn {
    /// `firing` or `resolved`.
    #[schema(example = "firing")]
    status: String,
    /// `vm`, `qd_alert`, `severity`, `alertname`, `rulename`, `mountpoint`,
    /// `device` are read.
    #[serde(default)]
    labels: BTreeMap<String, String>,
    /// Letters and digits, up to 64: Grafana's id of the alert.
    #[schema(example = "0a1b2c3d4e5f6789")]
    fingerprint: String,
    /// RFC 3339.
    #[schema(example = "2026-11-17T10:00:00Z")]
    starts_at: String,
    /// Each query's number (`A`, `C`); anything else is not kept.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    values: Option<Map<String, Value>>,
}

/// What a delivery did.
#[derive(Debug, Serialize, ToSchema)]
pub struct AlertsOut {
    /// Alerts kept, changed or removed, the heartbeat's included.
    applied: usize,
    /// Alerts left out: of a machine unknown or retired, or naming none.
    unknown: usize,
}

/// `POST /internal/machines/alerts`: Grafana's delivery, from the
/// application VM's machine.
#[utoipa::path(
    post,
    path = "/internal/machines/alerts",
    request_body = GrafanaDeliveryIn,
    responses(
        (status = 200, description = "The delivery's rows committed", body = AlertsOut),
        (status = 401, description = "No certificate, or one that cannot be read"),
        (status = 403, description = "Not an approved or draining app machine's certificate"),
        (status = 413, description = "The body is too large"),
        (status = 422, description = "Not Grafana's body"),
        (status = 503, description = "No alerts kept here, or the client certificate header is not trusted"),
    ),
    tag = "machines",
)]
pub async fn alerts_handler(
    State(machines): State<MachinesService>,
    Extension(unknown): Extension<UnknownAlerts>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    presented: Result<MachineCertificate, ClientCertificateError>,
    body: Bytes,
) -> Response {
    let presented = match presented {
        Ok(presented) => presented,
        Err(refused) => return refused.into_response(),
    };
    let Some(alerts) = delivered(&body) else {
        return invalid("alerts", texts::tr!("api.machines.invalidAlert"));
    };
    let origin = origin(ip, &headers);
    match machines.receive_alerts(&presented, &alerts, &origin).await {
        Ok(done) => {
            if done.unknown > 0 {
                (unknown.0)(done.unknown as u64);
            }
            Json(AlertsOut {
                applied: done.applied,
                unknown: done.unknown,
            })
            .into_response()
        }
        Err(e) => error(e),
    }
}

/// The body's alerts, or `None` if it is not Grafana's.
fn delivered(body: &[u8]) -> Option<Vec<DeliveredAlert>> {
    let body: GrafanaDeliveryIn = serde_json::from_slice(body).ok()?;
    body.alerts.into_iter().map(alert).collect()
}

fn alert(a: GrafanaAlertIn) -> Option<DeliveredAlert> {
    let firing = match a.status.as_str() {
        "firing" => true,
        "resolved" => false,
        _ => return None,
    };
    let fingerprint_ok = !a.fingerprint.is_empty()
        && a.fingerprint.len() <= 64
        && a.fingerprint.chars().all(|c| c.is_ascii_alphanumeric());
    let starts_at = chrono::DateTime::parse_from_rfc3339(&a.starts_at).ok()?;
    fingerprint_ok.then(|| DeliveredAlert {
        firing,
        labels: a.labels,
        fingerprint: a.fingerprint,
        starts_at: SystemTime::from(starts_at),
        values: a.values.unwrap_or_default(),
    })
}

fn error(e: AlertsError) -> Response {
    match e {
        AlertsError::Refused => ApiError::new(
            StatusCode::FORBIDDEN,
            "certificate_refused",
            texts::tr!("api.machines.alertsNotApp"),
        )
        .into_response(),
        AlertsError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "alerts_unavailable",
            texts::tr!("api.machines.alertsUnavailable"),
        )
        .into_response(),
        AlertsError::Backend(e) => ApiError::internal(format!("alerts: {e}")).into_response(),
    }
}
