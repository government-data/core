//! The module events (quality-sdk decision 0017) on PostgreSQL: the store,
//! the schemas compiled with `jsonschema`, and the listener that wakes the
//! reads waiting in this replica when any replica stores an event.

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
    events
}

/// What a module calls with its machine client: `/v1/modules`,
/// `/v1/iam/effective` and `/v1/events…`.
pub(crate) fn module_routes(
    provider: Option<AuthorizationServer>,
    updates: UpdatesService,
    pool: &PgPool,
) -> Router {
    api::platform_v1::routes(provider.clone(), updates.clone())
        .merge(api::events_v1::routes(provider, service(pool, updates)))
}
