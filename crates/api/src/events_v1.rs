//! `/v1/events` (quality-sdk decision 0017): a module publishes the events
//! of the topics it provides, reads with a cursor those it consumes
//! (waiting up to 30 s when there are none) and confirms what it handled.
//! The caller is its machine client's module, with a token for the core
//! ([`crate::platform_v1::require_module_client`]). `data` is never logged,
//! and a refusal of `data` names the schema paths, never the values.
//!
//! | Call                                | Does                              |
//! | ----------------------------------- | --------------------------------- |
//! | `POST /v1/events`                   | publish: `201`, `200` again       |
//! | `GET /v1/events?topic&version&after&wait` | `{"events", "cursor"}`      |
//! | `PUT /v1/events/cursors/{topic}`    | `{"version", "cursor"}` confirmed |

use std::time::Duration;

use application::events::{EventsError, EventsService, NewEvent, Published};
use auth_gateway::AuthorizationServer;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::platform_v1::{require_module_client, ModuleCaller};
use crate::updater_v1::refusal;

/// The largest request: `data` at its most and the event around it.
const BODY_LIMIT: usize = application::events::MAX_DATA_BYTES + 16 * 1024;
/// The longest a read waits for an event.
const MAX_WAIT: Duration = Duration::from_secs(30);

/// What the routes call.
#[derive(Clone)]
struct Events {
    provider: Option<AuthorizationServer>,
    events: EventsService,
}

/// Mount `/v1/events…`; `provider` is `None` without `OIDC_SIGNING_KEY`,
/// and then every call is a 503.
pub fn routes(provider: Option<AuthorizationServer>, events: EventsService) -> Router {
    let state = Events {
        provider: provider.clone(),
        events,
    };
    Router::new()
        .route("/v1/events", get(read).post(publish))
        .route("/v1/events/cursors/{topic}", put(confirm))
        .with_state(state)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn_with_state(
            provider,
            require_module_client,
        ))
}

/// The module that registered the calling client.
async fn module(e: &Events, caller: &ModuleCaller) -> Result<String, Box<Response>> {
    let Some(provider) = &e.provider else {
        let why = texts::tr!("api.updater.noProvider");
        return Err(Box::new(refusal(StatusCode::SERVICE_UNAVAILABLE, why)));
    };
    let module = provider.client_module(&caller.client).await;
    module.ok_or_else(|| {
        let why = texts::tr!("api.platform.notAModule");
        Box::new(refusal(StatusCode::FORBIDDEN, why))
    })
}

fn refused(call: &str, e: EventsError) -> Response {
    match e {
        EventsError::NotDeclared => {
            refusal(StatusCode::FORBIDDEN, texts::tr!("api.events.notDeclared"))
        }
        EventsError::TooBig => refusal(StatusCode::BAD_REQUEST, texts::tr!("api.events.tooBig")),
        EventsError::Invalid(paths) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": texts::tr!("api.events.invalid"), "schema_paths": paths })),
        )
            .into_response(),
        EventsError::BadSchema => crate::updater_v1::unavailable(),
        EventsError::Unavailable(why) => {
            tracing::error!("v1 events {call}: {why}");
            crate::updater_v1::unavailable()
        }
    }
}

/// Whether `id` is a UUID, and its canonical (lower-case) form.
fn uuid(id: &str) -> Option<String> {
    let bytes = id.as_bytes();
    let well_formed = bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    well_formed.then(|| id.to_ascii_lowercase())
}

#[derive(Deserialize)]
struct Publish {
    id: String,
    topic: String,
    version: u32,
    occurred_at: DateTime<Utc>,
    data: Value,
}

/// `POST /v1/events`: `201` stored, `200` the same id already stored.
async fn publish(
    State(e): State<Events>,
    Extension(caller): Extension<ModuleCaller>,
    body: Result<Json<Publish>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let module = match module(&e, &caller).await {
        Ok(module) => module,
        Err(refused) => return *refused,
    };
    let Ok(Json(body)) = body else {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.events.badEvent"));
    };
    let Some(id) = uuid(&body.id) else {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.events.badEvent"));
    };
    let event = NewEvent {
        id,
        topic: body.topic,
        version: body.version,
        occurred_at: body.occurred_at,
        data: body.data,
    };
    match e.events.publish(&module, event).await {
        Ok(Published::Stored) => StatusCode::CREATED.into_response(),
        Ok(Published::Again) => StatusCode::OK.into_response(),
        Err(why) => refused("publish", why),
    }
}

#[derive(Deserialize)]
struct ReadQuery {
    topic: String,
    version: u32,
    after: Option<i64>,
    /// Seconds, at most 30; none does not wait.
    wait: Option<u64>,
}

/// `GET /v1/events?topic=<t>&version=<v>&after=<cursor>&wait=<s>`.
async fn read(
    State(e): State<Events>,
    Extension(caller): Extension<ModuleCaller>,
    query: Result<Query<ReadQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let module = match module(&e, &caller).await {
        Ok(module) => module,
        Err(refused) => return *refused,
    };
    let Ok(Query(q)) = query else {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.events.badRead"));
    };
    let wait = Duration::from_secs(q.wait.unwrap_or(0)).min(MAX_WAIT);
    let deadline = tokio::time::sleep(wait);
    match e
        .events
        .read(&module, &q.topic, q.version, q.after, deadline)
        .await
    {
        Ok(batch) => {
            Json(json!({ "events": batch.events, "cursor": batch.cursor })).into_response()
        }
        Err(why) => refused("read", why),
    }
}

#[derive(Deserialize)]
struct Confirm {
    version: u32,
    cursor: i64,
}

/// `PUT /v1/events/cursors/{topic}`: `204`, handled up to `cursor`.
async fn confirm(
    State(e): State<Events>,
    Extension(caller): Extension<ModuleCaller>,
    Path(topic): Path<String>,
    body: Result<Json<Confirm>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let module = match module(&e, &caller).await {
        Ok(module) => module,
        Err(refused) => return *refused,
    };
    let Ok(Json(body)) = body else {
        return refusal(StatusCode::BAD_REQUEST, texts::tr!("api.events.badCursor"));
    };
    match e
        .events
        .confirm(&module, &topic, body.version, body.cursor)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(why) => refused("confirm", why),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_id_is_a_uuid_in_its_canonical_form() {
        let id = "0193A1B2-C3D4-7E5F-8A9B-0C1D2E3F4A5B";
        assert_eq!(super::uuid(id).as_deref(), Some(&*id.to_ascii_lowercase()));
        for bad in [
            "",
            "not-a-uuid",
            "0193a1b2c3d47e5f8a9b0c1d2e3f4a5b",
            "0193a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5g",
        ] {
            assert_eq!(super::uuid(bad), None, "{bad}");
        }
    }
}
