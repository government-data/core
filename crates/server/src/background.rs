//! The work that runs in the background while the server serves: the
//! license's renewal and heartbeat, what management sends (platform mail,
//! sign-in and captcha), each tenant's periodic round, and the scheduler.

use std::sync::Arc;

use application::{LicenseService, SettingsService};

use crate::settings_wiring;

/// Renew the permit with the cloud at start and once a day; if it fails, it
/// retries every hour (meanwhile the last good permit holds). After each
/// attempt, whatever expiry notice is due.
pub(crate) fn spawn_license_renewal(license: LicenseService) {
    // The heartbeat, so the cloud tells the administrators if the
    // installation goes down. A lost heartbeat is not serious: the cloud waits
    // for several.
    let heartbeat = license.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(application::HEARTBEAT_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            if let Err(e) = heartbeat.heartbeat().await {
                tracing::warn!("heartbeat to the cloud: {e}");
            }
        }
    });
    tokio::spawn(async move {
        loop {
            let wait = match license.refresh().await {
                Ok(()) => {
                    tracing::info!("license renewed with the cloud");
                    application::RENEW_EVERY
                }
                Err(e) => {
                    tracing::warn!("could not renew the license: {e}");
                    application::RETRY_EVERY
                }
            };
            match license.send_reminders(chrono::Utc::now()).await {
                Ok(Some(notice)) => tracing::info!(%notice, "license notice sent"),
                Ok(None) => {}
                Err(e) => tracing::warn!("could not send the license notice: {e}"),
            }
            tokio::time::sleep(wait).await;
        }
    });
}

/// The platform mail from the licensing cloud: at start and every five
/// minutes, so a change in management arrives on its own. If the cloud does
/// not answer, the last one that arrived stays.
pub(crate) fn spawn_platform_mail(
    settings: SettingsService,
    cloud: infrastructure::HttpLicenseAuthority,
    key: String,
    installation: String,
    captcha: Option<api::captcha::Captcha>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut had = None;
        let mut had_oidc = None;
        loop {
            interval.tick().await;
            match cloud.platform_mail(&key, &installation).await {
                Ok(mail) => {
                    let found = mail.is_some();
                    if let Err(e) =
                        settings.set_platform_mail(mail.map(settings_wiring::platform_mail))
                    {
                        tracing::error!("platform mail: {e}");
                        continue;
                    }
                    if had != Some(found) {
                        tracing::info!(found, "platform mail from the license cloud");
                        had = Some(found);
                    }
                }
                Err(e) => tracing::warn!("platform mail: {e}"),
            }
            // And signing in with the shared account (the instances we manage).
            match cloud.platform_oidc(&key, &installation).await {
                Ok(oidc) => {
                    let found = oidc.is_some();
                    if let Err(e) = settings
                        .set_platform_oauth(oidc.map(settings_wiring::platform_oidc))
                        .await
                    {
                        tracing::error!("platform sign-in: {e}");
                    } else if had_oidc != Some(found) {
                        tracing::info!(found, "sign-in with the cloud account");
                        had_oidc = Some(found);
                    }
                }
                Err(e) => tracing::warn!("platform sign-in: {e}"),
            }
            // And the platform's captcha, if it did not come from the environment.
            if let Some(captcha) = &captcha {
                match cloud.platform_captcha(&key, &installation).await {
                    Ok(keys) => captcha.configure(keys),
                    Err(e) => tracing::warn!("platform captcha: {e}"),
                }
            }
        }
    });
}

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
