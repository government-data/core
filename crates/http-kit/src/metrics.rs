//! The API's own request metrics (decision 0071, point 8: "Which routes are
//! slow or failing?"): `qd_http_requests_total` by route template and status
//! class, `qd_http_request_duration_seconds` by route template.
//!
//! A route is labelled by its **template** (`/datasets/{id}`), the router's
//! own text, never by the path asked for; a request no route matched is
//! [`UNMATCHED`] (point 10). So the label values are the router's templates
//! and nothing a client sends: an unknown path or an id adds no series. The
//! duration is by route alone, not by class too, so that every route the API
//! has, each with every class, stays under the scrape's 5,000 samples
//! (point 11).
//!
//! [`track`] must run where the route is known: added with `Router::layer`
//! on the router that holds every route, which runs it for each route (and
//! for the fallback, which has no template).

use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

/// The route of a request no route matched.
pub const UNMATCHED: &str = "unmatched";

/// Every status class a request is counted under.
pub const STATUS_CLASSES: [&str; 6] = ["1xx", "2xx", "3xx", "4xx", "5xx", "other"];

/// The request durations' buckets, in seconds.
const BUCKETS: [f64; 10] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0];

type Requests = Family<[(&'static str, String); 2], Counter>;
type Durations = Family<[(&'static str, String); 1], Histogram, fn() -> Histogram>;

/// The two families, cheap to clone (each is shared).
#[derive(Debug, Clone)]
pub struct HttpMetrics {
    requests: Requests,
    durations: Durations,
}

fn histogram() -> Histogram {
    Histogram::new(BUCKETS)
}

impl HttpMetrics {
    /// The families, registered in the API's `registry`.
    pub fn register(registry: &mut Registry) -> Self {
        let metrics = Self {
            requests: Requests::default(),
            durations: Durations::new_with_constructor(histogram),
        };
        registry.register(
            "qd_http_requests",
            "Requests the API answered, by route template and status class.",
            metrics.requests.clone(),
        );
        registry.register(
            "qd_http_request_duration_seconds",
            "How long the API took to answer, by route template, in seconds.",
            metrics.durations.clone(),
        );
        metrics
    }

    fn record(&self, route: String, status: u16, seconds: f64) {
        let class = status_class(status).to_string();
        self.requests
            .get_or_create(&[("route", route.clone()), ("status_class", class)])
            .inc();
        self.durations
            .get_or_create(&[("route", route)])
            .observe(seconds);
    }
}

/// `2xx` for 204: one of [`STATUS_CLASSES`].
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => STATUS_CLASSES[0],
        200..=299 => STATUS_CLASSES[1],
        300..=399 => STATUS_CLASSES[2],
        400..=499 => STATUS_CLASSES[3],
        500..=599 => STATUS_CLASSES[4],
        _ => STATUS_CLASSES[5],
    }
}

/// The middleware: counts and times the request under its route's template.
pub async fn track(State(metrics): State<HttpMetrics>, request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| UNMATCHED.to_string(), |m| m.as_str().to_string());
    let start = Instant::now();
    let response = next.run(request).await;
    metrics.record(
        route,
        response.status().as_u16(),
        start.elapsed().as_secs_f64(),
    );
    response
}

#[cfg(test)]
mod tests;
