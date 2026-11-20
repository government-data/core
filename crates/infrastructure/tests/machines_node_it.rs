//! A lake pool node's join token on Postgres (0066 step 11, the user's
//! ruling 1 of 2026-11-14): the token names its node, and joining with
//! another name is refused and leaves the token unused. Connected as the
//! application's role so row-level security applies. Needs Docker.

use std::time::{Duration, SystemTime};

use auth_gateway::{JoinTokenMint, MachineRegistry, MachineRole, NewJoinToken, RedeemRefusal};
use domain::AdminId;
use infrastructure::{PgMachines, RingJoinTokenMint};
use sqlx::postgres::PgPoolOptions;

const MAIN: &str = "00000000-0000-0000-0000-000000000001";

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

#[tokio::test]
async fn a_node_token_makes_only_the_machine_it_names() {
    let db = testkit::spawn_test_db().await;
    let pool = infrastructure::db::with_tenancy(
        PgPoolOptions::new().max_connections(4),
        tenancy::Mode::Multiple,
    )
    .connect_with(testkit::app_role(&db).await)
    .await
    .unwrap();
    let reg = PgMachines::new(pool);
    let mint = RingJoinTokenMint::new();
    let token = NewJoinToken {
        token_hash: mint.hash(&mint.mint().unwrap()),
        role: MachineRole::Lake,
        pool: Some("main".into()),
        node: Some("lake-b".into()),
        created_by: AdminId::new("00000000-0000-0000-0000-0000000000ad"),
        expires_at: t0() + Duration::from_secs(3600),
    };
    let main = tenancy::Tenant {
        id: MAIN.into(),
        slug: "main".into(),
    };
    tenancy::within(main, async {
        let id = reg.issue_token(&token).await.unwrap();
        let node: Option<String> =
            sqlx::query_scalar("SELECT node FROM join_tokens WHERE id = $1::uuid")
                .bind(&id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(node.as_deref(), Some("lake-b"));

        let other = reg.redeem(&token.token_hash, "lake-a", t0()).await;
        assert_eq!(other.unwrap(), Err(RedeemRefusal::NodeName));
        assert!(reg.list().await.unwrap().is_empty(), "no machine made");

        let made = reg.redeem(&token.token_hash, "lake-b", t0()).await;
        let made = made.unwrap().expect("the token was left unused");
        assert_eq!(
            (made.name.as_str(), made.role, made.pool.as_deref()),
            ("lake-b", MachineRole::Lake, Some("main"))
        );
        let again = reg.redeem(&token.token_hash, "lake-b", t0()).await;
        assert_eq!(again.unwrap(), Err(RedeemRefusal::Token));
    })
    .await;
}
