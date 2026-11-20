//! Verification for decision 0021: the versioned catalog in Dolt, driven from
//! sqlx the way the application would. Each step prints OK or FAIL.
//!
//! Against a throwaway Dolt server:
//!
//! ```sh
//! docker run -d --name qd-dolt-test -p 127.0.0.1:3307:3306 \
//!   -e DOLT_ROOT_PASSWORD=qdtest -e DOLT_ROOT_HOST=% \
//!   -v qd-dolt-test-backups:/backups dolthub/dolt-sql-server:latest
//! docker exec qd-dolt-test chmod 777 /backups
//! cargo run --manifest-path tools/verify/dolt-sqlx/Cargo.toml
//! docker rm -f qd-dolt-test && docker volume rm qd-dolt-test-backups
//! ```

use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::{Executor, Row};

const URL: &str = "mysql://root:qdtest@127.0.0.1:3307";

macro_rules! step {
    ($name:expr, $body:expr) => {{
        let r: Result<String, Box<dyn std::error::Error>> = async { $body }.await;
        match r {
            Ok(s) => println!("[OK  ] {:<58} {}", $name, s),
            Err(e) => println!("[FAIL] {:<58} {}", $name, e.to_string().chars().take(300).collect::<String>()),
        }
    }};
}

async fn one(pool: &MySqlPool, sql: &str) -> Result<String, Box<dyn std::error::Error>> {
    let row = sqlx::query(sql).fetch_one(pool).await?;
    Ok(row.try_get::<String, _>(0).or_else(|_| row.try_get::<i64, _>(0).map(|v| v.to_string()))?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let admin = MySqlPoolOptions::new().max_connections(2).connect(URL).await?;
    for db in ["tenant_a", "tenant_b", "tenant_a_restored"] {
        admin.execute(format!("DROP DATABASE IF EXISTS {db}").as_str()).await?;
    }

    step!("one database per tenant", {
        admin.execute("CREATE DATABASE tenant_a").await?;
        admin.execute("CREATE DATABASE tenant_b").await?;
        Ok(one(&admin, "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name LIKE 'tenant_%'").await?)
    });

    // The application's pool for one tenant: every connection starts on main.
    let a = MySqlPoolOptions::new().max_connections(4).connect(&format!("{URL}/tenant_a")).await?;

    step!("schema and first commit on main (prepared statements)", {
        a.execute("CREATE TABLE rules (id VARCHAR(16) PRIMARY KEY, dataset VARCHAR(64), yaml TEXT, status VARCHAR(16))").await?;
        sqlx::query("INSERT INTO rules VALUES (?, ?, ?, ?), (?, ?, ?, ?)")
            .bind("R-11").bind("clients").bind("check: required\nwhen: ActividadLaboral = 'EMPLEADO'").bind("approved")
            .bind("R-15").bind("clients").bind("check: equals\nvalue: Activo - Pasivo").bind("approved")
            .execute(&a).await?;
        let h: String = sqlx::query_scalar("CALL DOLT_COMMIT('-Am', 'catalog: first rules', '--author', 'Ana <ana@example.org>')")
            .fetch_one(&a).await?;
        Ok(format!("commit {}", &h[..8]))
    });

    step!("draft = branch, edited without checkout (db/branch name)", {
        sqlx::query("CALL DOLT_BRANCH('draft-r11-ana')").execute(&a).await?;
        sqlx::query("CALL DOLT_BRANCH('draft-r11-luis')").execute(&a).await?;
        // A branch is reachable as database `tenant_a/<branch>`: no session state,
        // so a pooled connection never stays on the wrong branch.
        sqlx::query("UPDATE `tenant_a/draft-r11-ana`.rules SET yaml = ?, status = 'draft' WHERE id = 'R-11'")
            .bind("check: required\nwhen: ActividadLaboral IN ('EMPLEADO', 'CONTRATISTA')")
            .execute(&a).await?;
        sqlx::query("UPDATE `tenant_a/draft-r11-luis`.rules SET yaml = ?, status = 'draft' WHERE id = 'R-11'")
            .bind("check: required\nwhen: ActividadLaboral = 'EMPLEADO' AND client_type = 'PN'")
            .execute(&a).await?;
        let main_yaml = one(&a, "SELECT yaml FROM rules WHERE id = 'R-11'").await?;
        Ok(format!("main untouched: {}", main_yaml.contains("= 'EMPLEADO'") && !main_yaml.contains("IN (")))
    });

    step!("commit each draft on its branch (dedicated connection)", {
        for (branch, who) in [("draft-r11-ana", "Ana <ana@example.org>"), ("draft-r11-luis", "Luis <luis@example.org>")] {
            let mut c = a.acquire().await?;
            c.execute(format!("USE `tenant_a/{branch}`").as_str()).await?;
            sqlx::query("CALL DOLT_COMMIT('-am', ?, '--author', ?)")
                .bind(format!("R-11: edit by {who}")).bind(who)
                .execute(&mut *c).await?;
            c.execute("USE tenant_a").await?;
        }
        Ok("2 commits".into())
    });

    step!("pooled connections still on main after USE of a branch", {
        let mut seen = vec![];
        for _ in 0..4 {
            seen.push(one(&a, "SELECT active_branch()").await?);
        }
        Ok(seen.join(","))
    });

    step!("diff of a draft against main, cell by cell", {
        let rows = sqlx::query(
            "SELECT diff_type, from_yaml, to_yaml, to_status FROM DOLT_DIFF('main', 'draft-r11-ana', 'rules')",
        )
        .fetch_all(&a).await?;
        let r = &rows[0];
        Ok(format!(
            "{} row(s): {} | {:?} -> {:?} | status -> {:?}",
            rows.len(),
            r.try_get::<String, _>(0)?,
            r.try_get::<Option<String>, _>(1)?.map(|s| s.replace('\n', " ")),
            r.try_get::<Option<String>, _>(2)?.map(|s| s.replace('\n', " ")),
            r.try_get::<Option<String>, _>(3)?
        ))
    });

    step!("approve = merge the first draft into main", {
        let mut c = a.acquire().await?;
        c.execute("USE tenant_a").await?;
        let row = sqlx::query("CALL DOLT_MERGE('draft-r11-ana', '--no-ff', '-m', 'approve R-11 (Ana)', '--author', 'Approver <ok@example.org>')")
            .fetch_one(&mut *c).await?;
        let conflicts: i64 = row.try_get(2)?;
        let yaml = one(&a, "SELECT yaml FROM rules WHERE id = 'R-11'").await?;
        Ok(format!("conflicts={conflicts}, main now has IN: {}", yaml.contains("IN (")))
    });

    step!("second draft of the same rule: conflict detected", {
        let mut c = a.acquire().await?;
        c.execute("USE tenant_a").await?;
        c.execute("SET @@dolt_allow_commit_conflicts = 1").await?;
        c.execute("START TRANSACTION").await?;
        let res = sqlx::query("CALL DOLT_MERGE('draft-r11-luis', '-m', 'approve R-11 (Luis)')").fetch_one(&mut *c).await;
        let out = match res {
            Ok(row) => {
                let conflicts: i64 = row.try_get(2)?;
                let rows = sqlx::query("SELECT our_yaml, their_yaml FROM dolt_conflicts_rules").fetch_all(&mut *c).await?;
                format!("conflicts={conflicts}, rows in dolt_conflicts_rules={}", rows.len())
            }
            Err(e) => format!("merge refused: {}", e.to_string().chars().take(160).collect::<String>()),
        };
        c.execute("ROLLBACK").await?;
        c.execute("SET @@dolt_allow_commit_conflicts = 0").await?;
        Ok(out)
    });

    step!("main unchanged after the refused merge", {
        Ok(format!("IN still there: {}", one(&a, "SELECT yaml FROM rules WHERE id = 'R-11'").await?.contains("IN (")))
    });

    step!("run records the commit; AS OF rebuilds that catalog", {
        let first: String = one(&a, "SELECT commit_hash FROM dolt_log WHERE message = 'catalog: first rules'").await?;
        let head = one(&a, "SELECT HASHOF('main')").await?;
        let then = one(&a, &format!("SELECT yaml FROM rules AS OF '{first}' WHERE id = 'R-11'")).await?;
        Ok(format!("first={} head={} then has IN: {}", &first[..8], &head[..8], then.contains("IN (")))
    });

    step!("history and blame per row", {
        let hist = one(&a, "SELECT COUNT(*) FROM dolt_history_rules WHERE id = 'R-11'").await?;
        let blame = sqlx::query("SELECT id, committer, message FROM dolt_blame_rules ORDER BY id").fetch_all(&a).await?;
        let b: Vec<String> = blame
            .iter()
            .map(|r| format!("{}={}", r.get::<String, _>(0), r.get::<String, _>(1)))
            .collect();
        Ok(format!("versions of R-11={hist}; blame {}", b.join(" ")))
    });

    step!("hazard: a branch left on a pooled connection leaks", {
        let p = MySqlPoolOptions::new().max_connections(1).connect(&format!("{URL}/tenant_a")).await?;
        {
            let mut c = p.acquire().await?;
            c.execute("USE `tenant_a/draft-r11-luis`").await?;
        } // returned to the pool without going back to main
        Ok(format!("next user of the pool is on: {}", one(&p, "SELECT active_branch()").await?))
    });

    step!("mitigation: reset to main when a connection returns", {
        let p = MySqlPoolOptions::new()
            .max_connections(1)
            .after_release(|conn, _meta| Box::pin(async move {
                conn.execute("USE tenant_a").await?;
                Ok(true)
            }))
            .connect(&format!("{URL}/tenant_a")).await?;
        {
            let mut c = p.acquire().await?;
            c.execute("USE `tenant_a/draft-r11-luis`").await?;
        }
        Ok(format!("next user of the pool is on: {}", one(&p, "SELECT active_branch()").await?))
    });

    step!("tenant isolation: tenant_b has no rules table", {
        let b = MySqlPoolOptions::new().max_connections(1).connect(&format!("{URL}/tenant_b")).await?;
        let n = one(&b, "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = 'tenant_b'").await?;
        Ok(format!("tables in tenant_b={n}"))
    });

    step!("backup and restore", {
        let mut c = a.acquire().await?;
        c.execute("USE tenant_a").await?;
        sqlx::query("CALL DOLT_BACKUP('add', 'bk', 'file:///backups/tenant_a')").execute(&mut *c).await?;
        sqlx::query("CALL DOLT_BACKUP('sync', 'bk')").execute(&mut *c).await?;
        sqlx::query("CALL DOLT_BACKUP('restore', 'file:///backups/tenant_a', 'tenant_a_restored')").execute(&mut *c).await?;
        let n = one(&admin, "SELECT COUNT(*) FROM tenant_a_restored.rules").await?;
        let h1 = one(&a, "SELECT HASHOF('main')").await?;
        let r = MySqlPoolOptions::new().max_connections(1).connect(&format!("{URL}/tenant_a_restored")).await?;
        let h2 = one(&r, "SELECT HASHOF('main')").await?;
        Ok(format!("rules restored={n}, same head commit: {}", h1 == h2))
    });

    let version = one(&admin, "SELECT dolt_version()").await?;
    println!("dolt {version}");
    Ok(())
}
