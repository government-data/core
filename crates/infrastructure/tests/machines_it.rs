//! The machines and their join tokens on Postgres (decisions 0029, 0066),
//! connected as the application's role so row-level security applies. The
//! rules of the use cases are tested over memory in `application`; here, what
//! the SQL holds: a token used once and within its hour, of two redemptions
//! one, names, the transitions, the serial in use, and tenants. Needs Docker.

mod machines {
    use std::time::{Duration, SystemTime};

    use auth_gateway::{
        is_join_token, JoinTokenMint, Machine, MachineChange, MachineId, MachineRegistry,
        MachineRole, MachineState, NewJoinToken, RedeemRefusal,
    };
    use domain::AdminId;
    use infrastructure::{PgMachines, RingJoinTokenMint};
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;
    use MachineState::{Approved, Draining, Pending, Retired};

    const MAIN: &str = "00000000-0000-0000-0000-000000000001";
    const HOUR: Duration = Duration::from_secs(3600);

    fn admin() -> AdminId {
        AdminId::new("00000000-0000-0000-0000-0000000000ad")
    }

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    async fn app_pool(db: &testkit::TestDb, mode: tenancy::Mode) -> PgPool {
        infrastructure::db::with_tenancy(PgPoolOptions::new().max_connections(4), mode)
            .connect_with(testkit::app_role(db).await)
            .await
            .unwrap()
    }

    /// A token for a worker of `pool`; returns its hash, which is what
    /// `redeem` takes.
    async fn issue(reg: &PgMachines, pool: Option<&str>, expires_at: SystemTime) -> String {
        let mint = RingJoinTokenMint::new();
        let token = mint.mint().unwrap();
        reg.issue_token(&NewJoinToken {
            token_hash: mint.hash(&token),
            role: MachineRole::Worker,
            pool: pool.map(str::to_string),
            node: None,
            created_by: admin(),
            expires_at,
        })
        .await
        .unwrap();
        mint.hash(&token)
    }

    async fn join(reg: &PgMachines, name: &str) -> Machine {
        let hash = issue(reg, Some("default"), t0() + HOUR).await;
        reg.redeem(&hash, name, t0()).await.unwrap().unwrap()
    }

    async fn to(reg: &PgMachines, m: &Machine, state: MachineState) -> MachineChange {
        reg.transition(&m.id, state, &admin(), t0()).await.unwrap()
    }

    #[tokio::test]
    async fn machines_join_and_live_their_whole_life() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        let mint = RingJoinTokenMint::new();
        let token = mint.mint().unwrap();
        assert!(is_join_token(&token));
        let new = NewJoinToken {
            token_hash: mint.hash(&token),
            role: MachineRole::TrinoWorker,
            pool: None,
            node: None,
            created_by: admin(),
            expires_at: t0() + HOUR,
        };
        let token_id = reg.issue_token(&new).await.unwrap();

        let m = reg.redeem(&new.token_hash, "trino-1", t0()).await.unwrap();
        let m = m.unwrap();
        assert_eq!(
            (m.state, m.role, m.pool.as_deref()),
            (Pending, MachineRole::TrinoWorker, None)
        );
        let (stored, used_by): (String, Option<String>) = sqlx::query_as(
            "SELECT token_hash, machine_id::text FROM join_tokens WHERE id = $1::uuid",
        )
        .bind(&token_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(stored, new.token_hash);
        assert_ne!(stored, token);
        assert_eq!(used_by.as_deref(), Some(m.id.as_str()));

        let MachineChange::Done(a) = to(&reg, &m, Approved).await else {
            panic!("pending → approved");
        };
        assert_eq!(
            (a.approved_at, a.approved_by.clone()),
            (Some(t0()), Some(admin()))
        );
        let MachineChange::Done(r) = to(&reg, &m, Retired).await else {
            panic!("approved → retired");
        };
        assert_eq!(
            (r.retired_at, r.retired_by.clone()),
            (Some(t0()), Some(admin()))
        );
        assert_eq!(
            reg.find(&m.id).await.unwrap().map(|m| m.state),
            Some(Retired)
        );
        assert_eq!(reg.find_live("trino-1").await.unwrap(), None);
        assert_eq!(reg.list().await.unwrap(), [*r]);
    }

    #[tokio::test]
    async fn machines_tokens_work_once_and_within_their_hour() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        let refused = Err(RedeemRefusal::Token);

        let hash = issue(&reg, None, t0() + HOUR).await;
        assert!(reg.redeem(&hash, "w-1", t0()).await.unwrap().is_ok());
        assert_eq!(reg.redeem(&hash, "w-2", t0()).await.unwrap(), refused);

        let hash = issue(&reg, None, t0() + HOUR).await;
        assert_eq!(
            reg.redeem(&hash, "w-3", t0() + HOUR).await.unwrap(),
            refused
        );
        let ok = reg.redeem(&hash, "w-3", t0() + HOUR - Duration::from_secs(1));
        assert!(ok.await.unwrap().is_ok());

        let hash = issue(&reg, None, t0() - Duration::from_secs(1)).await;
        assert_eq!(reg.redeem(&hash, "w-4", t0()).await.unwrap(), refused);
        assert_eq!(
            reg.redeem(&"0".repeat(64), "w-5", t0()).await.unwrap(),
            refused
        );
        assert_eq!(reg.list().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn machines_of_two_redemptions_of_one_token_one_wins() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        for round in 0..10 {
            let hash = issue(&reg, None, t0() + HOUR).await;
            let (a, b) = (format!("a-{round}"), format!("b-{round}"));
            let (one, two) = tokio::join!(reg.redeem(&hash, &a, t0()), reg.redeem(&hash, &b, t0()));
            let outcomes = [one.unwrap(), two.unwrap()];
            let won = outcomes.iter().filter(|o| o.is_ok()).count();
            assert_eq!(won, 1, "round {round}: {outcomes:?}");
            assert!(outcomes.contains(&Err(RedeemRefusal::Token)));
        }
        assert_eq!(reg.list().await.unwrap().len(), 10);
    }

    #[tokio::test]
    async fn machines_names_belong_to_one_live_machine() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        let first = join(&reg, "worker-01").await;

        let hash = issue(&reg, None, t0() + HOUR).await;
        let taken = reg.redeem(&hash, "worker-01", t0()).await;
        assert_eq!(taken.unwrap(), Err(RedeemRefusal::NameTaken));
        // The refusal left the token unused.
        let other = reg.redeem(&hash, "worker-02", t0()).await.unwrap();
        assert_eq!(other.unwrap().name, "worker-02");

        // Two tokens racing for one name: one machine.
        let (h1, h2) = (
            issue(&reg, None, t0() + HOUR).await,
            issue(&reg, None, t0() + HOUR).await,
        );
        let (one, two) = tokio::join!(
            reg.redeem(&h1, "worker-03", t0()),
            reg.redeem(&h2, "worker-03", t0())
        );
        let outcomes = [one.unwrap(), two.unwrap()];
        assert_eq!(outcomes.iter().filter(|o| o.is_ok()).count(), 1);
        assert!(outcomes.contains(&Err(RedeemRefusal::NameTaken)));

        // A retired machine's name joins again.
        to(&reg, &first, Retired).await;
        let again = join(&reg, "worker-01").await;
        assert_ne!(again.id, first.id);
        assert_eq!(reg.find_live("worker-01").await.unwrap(), Some(again));
    }

    /// A machine brought to `state` along the allowed path.
    async fn machine_in(reg: &PgMachines, name: &str, state: MachineState) -> Machine {
        let m = join(reg, name).await;
        let path: &[MachineState] = match state {
            Pending => &[],
            Approved => &[Approved],
            Draining => &[Approved, Draining],
            Retired => &[Retired],
        };
        for step in path {
            assert!(matches!(to(reg, &m, *step).await, MachineChange::Done(_)));
        }
        m
    }

    #[tokio::test]
    async fn machines_move_only_as_allowed() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        let mut n = 0;
        for from in MachineState::ALL {
            for target in MachineState::ALL {
                n += 1;
                let m = machine_in(&reg, &format!("m-{n}"), from).await;
                let change = to(&reg, &m, target).await;
                if from.may_become(target) {
                    let MachineChange::Done(moved) = change else {
                        panic!("{from:?} → {target:?}: {change:?}");
                    };
                    assert_eq!(moved.state, target);
                } else {
                    assert_eq!(
                        change,
                        MachineChange::NotAllowed(from),
                        "{from:?} → {target:?}"
                    );
                }
            }
        }
        for unknown in ["00000000-0000-0000-0000-00000000dead", "not-a-uuid"] {
            let id = MachineId::new(unknown);
            let change = reg.transition(&id, Approved, &admin(), t0()).await;
            assert_eq!(change.unwrap(), MachineChange::NotFound);
        }
    }

    #[tokio::test]
    async fn machines_keep_the_serial_in_use_until_retired() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        let m = join(&reg, "worker-01").await;
        let serial = |reg: &PgMachines| {
            let reg = reg.clone();
            async move {
                reg.find_live("worker-01")
                    .await
                    .unwrap()
                    .and_then(|m| m.cert_serial)
            }
        };
        assert_eq!(serial(&reg).await, None);

        let end = t0() + Duration::from_secs(86_400);
        let change = reg.record_certificate(&m.id, "0a1b", end).await.unwrap();
        let MachineChange::Done(with) = change else {
            panic!("a pending machine takes a certificate");
        };
        assert_eq!(with.cert_not_after, Some(end));
        assert_eq!(serial(&reg).await.as_deref(), Some("0a1b"));
        reg.record_certificate(&m.id, "ff", end).await.unwrap();
        assert_eq!(serial(&reg).await.as_deref(), Some("ff"));

        to(&reg, &m, Retired).await;
        assert_eq!(serial(&reg).await, None);
        let refused = reg.record_certificate(&m.id, "ee", end).await;
        assert_eq!(refused.unwrap(), MachineChange::NotAllowed(Retired));
    }

    #[tokio::test]
    async fn machines_of_another_tenant_are_not_seen() {
        let db = testkit::spawn_test_db().await;
        let acme = testkit::new_tenant(&db, "acme").await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Multiple).await);
        let in_tenant = |id: &str| tenancy::Tenant {
            id: id.into(),
            slug: "t".into(),
        };

        let (theirs, unused) = tenancy::within(in_tenant(&acme), async {
            let m = join(&reg, "worker-01").await;
            (m, issue(&reg, None, t0() + HOUR).await)
        })
        .await;

        tenancy::within(in_tenant(MAIN), async {
            assert!(reg.list().await.unwrap().is_empty());
            assert_eq!(reg.find_live("worker-01").await.unwrap(), None);
            assert_eq!(reg.find(&theirs.id).await.unwrap(), None);
            let change = reg.transition(&theirs.id, Approved, &admin(), t0()).await;
            assert_eq!(change.unwrap(), MachineChange::NotFound);
            let cert = reg.record_certificate(&theirs.id, "ab", t0()).await;
            assert_eq!(cert.unwrap(), MachineChange::NotFound);
            let redeemed = reg.redeem(&unused, "worker-02", t0()).await;
            assert_eq!(redeemed.unwrap(), Err(RedeemRefusal::Token));
            // Its own machine may have the other tenant's name.
            join(&reg, "worker-01").await;
        })
        .await;

        tenancy::within(in_tenant(&acme), async {
            let mine = reg.list().await.unwrap();
            assert_eq!(mine.len(), 1);
            assert_eq!(mine[0].state, Pending);
        })
        .await;
    }

    /// Ending a machine's sessions takes privileges the application's role
    /// has only when machines have certificates (initdb/20-queue-role.sh):
    /// without them it is a failure, not "none". The sessions ended for real
    /// are the worker's TLS tests (`queue_conn/machines_tests.rs`).
    #[tokio::test]
    async fn ending_a_machines_sessions_needs_its_privileges() {
        use auth_gateway::MachineSessions;
        use infrastructure::PgMachineSessions;
        let db = testkit::spawn_test_db().await;
        let app = app_pool(&db, tenancy::Mode::Single).await;
        let refused = PgMachineSessions::new(app.clone()).end("worker-01").await;
        assert!(refused
            .unwrap_err()
            .to_string()
            .contains("pg_signal_backend"));
        sqlx::query("GRANT pg_read_all_stats, pg_signal_backend TO app")
            .execute(&db.pool)
            .await
            .unwrap();
        let sessions = PgMachineSessions::new(app);
        assert_eq!(sessions.end("worker-01").await.unwrap(), 0, "none open");
        assert_eq!(sessions.end("Bad Name").await.unwrap(), 0);
    }

    /// The application VM's machine (0066, points 15 and 16) is stored as
    /// any other, but no join token is ever for its role.
    #[tokio::test]
    async fn the_application_vm_is_a_machine_that_never_joins() {
        let db = testkit::spawn_test_db().await;
        let reg = PgMachines::new(app_pool(&db, tenancy::Mode::Single).await);
        sqlx::query(
            "INSERT INTO machines (tenant_id, name, role, state) \
             VALUES ($1::uuid, 'qd-app', 'app', 'approved')",
        )
        .bind(MAIN)
        .execute(&db.pool)
        .await
        .expect("the role app is a machine's");
        let app = reg.find_live("qd-app").await.unwrap().unwrap();
        assert_eq!((app.role, app.state), (MachineRole::App, Approved));

        let mint = RingJoinTokenMint::new();
        let for_app = NewJoinToken {
            token_hash: mint.hash(&mint.mint().unwrap()),
            role: MachineRole::App,
            pool: None,
            node: None,
            created_by: admin(),
            expires_at: t0() + HOUR,
        };
        let token = reg.issue_token(&for_app).await;
        assert!(token.is_err(), "no join token for app");
    }
}
