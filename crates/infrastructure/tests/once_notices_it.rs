//! The once-notices (decision 0090, ruling 78) on an ephemeral Postgres,
//! as the application's role: a notice is recorded once. Needs Docker.

use application::once_notices::{OnceNotices, TRINO_RESULTS};
use infrastructure::once_notices::PgOnceNotices;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn a_notice_is_recorded_once() {
    let db = testkit::spawn_test_db().await;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(testkit::app_role(&db).await)
        .await
        .unwrap();
    let store = PgOnceNotices::new(pool);
    assert!(!store.sent(TRINO_RESULTS).await.unwrap());
    assert!(store.mark_sent(TRINO_RESULTS).await.unwrap());
    assert!(
        !store.mark_sent(TRINO_RESULTS).await.unwrap(),
        "another start"
    );
    assert!(store.sent(TRINO_RESULTS).await.unwrap());
}
