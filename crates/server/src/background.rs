//! The work that runs in the background while the server serves: the
//! scheduler.

/// The scheduler (decision 0035): every `SCHEDULER_INTERVAL_SECS` (30 by
/// default), if this VM holds its lock, fires the due schedules. Fires missed for longer than
/// `SCHEDULER_MISSED_MAX_HOURS` (24) are skipped.
pub(crate) fn spawn_scheduler(pool: infrastructure::PgPool) {
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
            for fired in scheduler
                .round(chrono::Utc::now())
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
