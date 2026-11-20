//! The work that runs in the background while the server serves: each
//! tenant's periodic round and the scheduler.

use std::sync::Arc;

use application::SettingsService;

/// The background work of every tenant in this cell, every
/// `BACKGROUND_INTERVAL_SECS` (15 by default): keep the platform mail set.
pub(crate) fn spawn_background(
    settings: SettingsService,
    directory: Option<Arc<dyn tenancy::Directory>>,
    cell: String,
) {
    let tick = std::time::Duration::from_secs(
        std::env::var("BACKGROUND_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(15),
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let tenants = match &directory {
                None => vec![(tenancy::Tenant::main(), true)],
                Some(directory) => match directory.in_cell(&cell).await {
                    Ok(records) => records
                        .into_iter()
                        .map(|r| (r.tenant, r.status == tenancy::Status::Active))
                        .collect(),
                    Err(e) => {
                        tracing::error!("tenant registry: {e}");
                        continue;
                    }
                },
            };
            for (tenant, _active) in tenants {
                let slug = tenant.slug.clone();
                tenancy::within(tenant, async {
                    // The platform mail, if there is one, stays set on the
                    // company (and up to date).
                    match settings.seed_platform_mail().await {
                        Ok(true) => tracing::info!(%slug, "platform mail set for the company"),
                        Ok(false) => {}
                        Err(e) => tracing::error!(%slug, "settings: {e}"),
                    }
                })
                .await;
            }
        }
    });
}

/// The scheduler (decision 0035): every `SCHEDULER_INTERVAL_SECS` (30 by
/// default), if this VM holds its lock, fires the due schedules of every
/// active tenant of this cell. Fires missed for longer than
/// `SCHEDULER_MISSED_MAX_HOURS` (24) are skipped.
pub(crate) fn spawn_scheduler(
    pool: infrastructure::PgPool,
    directory: Option<Arc<dyn tenancy::Directory>>,
    cell: String,
) {
    let number = |name: &str, default: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(default)
    };
    let tick = std::time::Duration::from_secs(number("SCHEDULER_INTERVAL_SECS", 30));
    let hours = u32::try_from(number("SCHEDULER_MISSED_MAX_HOURS", 24)).unwrap_or(24);
    let mut scheduler = quality::scheduler::Scheduler::new(pool, hours);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let tenants = match active_tenants(directory.as_ref(), &cell).await {
                Ok(tenants) => tenants,
                Err(e) => {
                    tracing::error!("tenant registry: {e}");
                    continue;
                }
            };
            for fired in scheduler
                .round(tenants, chrono::Utc::now())
                .await
                .unwrap_or_default()
            {
                tracing::info!(
                    schedule = %fired.schedule,
                    outcome = ?fired.outcome,
                    due = %fired.due_at,
                    "schedule fired"
                );
            }
        }
    });
}

/// The active tenants of this cell: on-premise, the main one.
pub(crate) async fn active_tenants(
    directory: Option<&Arc<dyn tenancy::Directory>>,
    cell: &str,
) -> Result<Vec<tenancy::Tenant>, String> {
    let Some(directory) = directory else {
        return Ok(vec![tenancy::Tenant::main()]);
    };
    Ok(directory
        .in_cell(cell)
        .await?
        .into_iter()
        .filter(|r| r.status == tenancy::Status::Active)
        .map(|r| r.tenant)
        .collect())
}
