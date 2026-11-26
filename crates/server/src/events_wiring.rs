//! The module events (quality-sdk decision 0017) on PostgreSQL: the store,
//! the schemas compiled with `jsonschema`, and the listener that wakes the
//! reads waiting in this replica when any replica stores an event, and the
//! daily retention.

use std::sync::Arc;

use application::events::EventsService;
use application::UpdatesService;
use auth_gateway::AuthorizationServer;
use axum::Router;
use infrastructure::module_events_repo::{PgModuleEvents, CHANNEL};
use infrastructure::PgPool;

/// The events' use case, listening for stored events from now on.
pub(crate) fn service(pool: &PgPool, updates: UpdatesService) -> EventsService {
    let events = EventsService::new(
        Arc::new(PgModuleEvents::new(pool.clone())),
        updates,
        Arc::new(infrastructure::event_schemas::JsonSchemas),
    );
    let woken = events.clone();
    infrastructure::permission_notices::spawn_channel_listener(pool.clone(), CHANNEL, move |t| {
        woken.notified(t);
    });
    spawn_retention(events.clone());
    events
}

/// Once a day (and at start), delete the events no longer kept (decision
/// 0017's retention). Every replica runs it; a second run deletes nothing.
fn spawn_retention(events: EventsService) {
    tokio::spawn(async move {
        let mut daily = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
        daily.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            daily.tick().await;
            match events.retain(chrono::Utc::now()).await {
                Ok(done) => tracing::info!(
                    events = done.events,
                    cursors = done.cursors,
                    "module events retention"
                ),
                Err(e) => tracing::warn!("module events retention: {e:?}"),
            }
        }
    });
}

/// What a module calls with its machine client: `/v1/modules`,
/// `/v1/iam/effective`, `/v1/machines` and `/v1/events…`.
pub(crate) fn module_routes(
    provider: Option<AuthorizationServer>,
    updates: UpdatesService,
    machines: application::MachinesService,
    pool: &PgPool,
) -> Router {
    api::platform_v1::routes(provider.clone(), updates.clone())
        .merge(api::machines_v1::routes(provider.clone(), machines))
        .merge(api::events_v1::routes(provider, service(pool, updates)))
}
