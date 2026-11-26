//! Postgres's `permissions` notice (ADR 0013, migration 0077): when what
//! someone can do changes in a tenant, whoever keeps Actors in memory is told.
//! It reconnects by itself; on reconnecting it notifies "everyone", because
//! whatever happened while it was cut did not arrive. [`spawn_channel_listener`]
//! does the same on another channel (the module events', quality-sdk
//! decision 0017).

use std::time::Duration;

use sqlx::postgres::PgListener;
use sqlx::PgPool;

const CHANNEL: &str = "permissions";

/// Start listening: `notify(Some(tenant))` on each change, `notify(None)` on
/// (re)connecting. It lives as long as the process.
pub fn spawn_listener<F>(pool: PgPool, notify: F)
where
    F: Fn(Option<&str>) + Send + Sync + 'static,
{
    spawn_channel_listener(pool, CHANNEL, notify);
}

/// Listen to `channel`: `notify(Some(payload))` on each notice,
/// `notify(None)` on (re)connecting. It lives as long as the process.
pub fn spawn_channel_listener<F>(pool: PgPool, channel: &'static str, notify: F)
where
    F: Fn(Option<&str>) + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(1);
        loop {
            match listen(&pool, channel).await {
                Ok(mut listener) => {
                    delay = Duration::from_secs(1);
                    notify(None);
                    loop {
                        match listener.try_recv().await {
                            Ok(Some(notice)) => {
                                let payload = notice.payload();
                                notify((!payload.is_empty()).then_some(payload));
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(channel, "notices: {e}");
                                break;
                            }
                        }
                    }
                    tracing::warn!(
                        channel,
                        "the Postgres notice listener was cut, reconnecting"
                    );
                }
                Err(e) => {
                    tracing::warn!(channel, "could not listen to notices: {e}");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    });
}

async fn listen(pool: &PgPool, channel: &str) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(channel).await?;
    Ok(listener)
}
