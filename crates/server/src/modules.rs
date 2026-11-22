//! The feature modules this server mounts, and their migrations on
//! PostgreSQL. `bootstrap-admin` applies them after `infrastructure`'s (it
//! creates their bookkeeping table); `serve` applies them again, which does
//! nothing when they are in place, and refuses to start if one already
//! applied has changed.

use std::sync::Arc;
use std::time::Duration;

use auth_gateway::AuthorizationServer;
use contracts::{MigrationReport, ModuleRegistry};
use domain::{AuditEventType, AuditRepository, NewActionAudit};
use infrastructure::module_migrations::PgModuleMigrator;
use infrastructure::rustfs_catalog::TableLookup;
use infrastructure::rustfs_manifests::ManifestWriter;
use infrastructure::rustfs_sts::StsIssuer;
use infrastructure::{PgAuditRepository, PgPool, SecretBox};
use quality::catalog::Catalog;
use quality::draft_api::DraftCatalog;
use quality::job_map::Drafts;
use quality::job_sources::IssueLog;
use quality::lake::{LakeAccess, LakeIssuer, TableCatalog};
use quality::manifest::ManifestStore;
use quality::module::QualityModule;
use quality::proposal::Patterns;
use quality::source_store::Sealer;
use quality::worker_api::WorkerServices;
use texts::Language;

use crate::catalog_wiring::{proposal_alerts, DoltSnapshots};
use uuid::Uuid;

/// The modules as their schema: what `migrate` and `init` need. Adding a
/// feature is ONE line here and one in [`serving`]; its crate carries the
/// rest.
pub(crate) fn registry() -> ModuleRegistry {
    ModuleRegistry::new().register(QualityModule::default())
}

/// The modules with the services their routes use. The workers' routes need
/// the OIDC provider (on-premise) to know the machine behind a token;
/// without one they are not served. With the lake's issuer (`LAKE_ENDPOINT`
/// and the issuer's keys), jobs come with credentials for their warehouse;
/// with the catalog (`DOLT_URL`), with their dataset's mapping and rules;
/// and evaluations leave their manifests in the warehouse through the
/// same issuer. The sources' routes for people seal their secrets with the
/// installation's key, as the settings do, and ask the catalog which
/// mappings read a source before deleting it.
pub(crate) fn serving(
    pool: &PgPool,
    provider: Option<AuthorizationServer>,
    secret_box: SecretBox,
    dolt: Option<Arc<DoltSnapshots>>,
) -> Result<ModuleRegistry, Box<dyn std::error::Error>> {
    let issuer = StsIssuer::from_env()?;
    let manifests = issuer
        .clone()
        .map(|i| Arc::new(LakeManifests(ManifestWriter::new(i))) as Arc<dyn ManifestStore>);
    let tables = issuer
        .clone()
        .map(|i| Arc::new(LakeTables(TableLookup::new(i))) as Arc<dyn TableCatalog>);
    let lake = issuer.map(|i| Arc::new(LakeSts(i)) as Arc<dyn LakeIssuer>);
    let catalog = dolt.clone().map(|d| d as Arc<dyn Catalog>);
    let drafts = dolt.clone().map(|d| d as Arc<dyn Drafts>);
    let review = dolt.map(|d| d as Arc<dyn DraftCatalog>);
    let quality = match provider {
        Some(p) => QualityModule::serving(
            pool.clone(),
            WorkerServices {
                tokens: Arc::new(ProviderTokens(p)),
                lake,
                catalog: catalog.clone(),
                manifests,
                tables,
                drafts,
                proposal_alerts: Some(proposal_alerts(Language::Es)),
                sources: Some(Arc::new(BoxSealer(secret_box.clone()))),
                issues: Some(Arc::new(Issues(Arc::new(PgAuditRepository::new(
                    pool.clone(),
                ))))),
            },
        ),
        None => QualityModule::default(),
    }
    .with_sources(pool.clone(), Arc::new(BoxSealer(secret_box)), catalog)
    .with_patterns(semantic_patterns()?);
    let quality = match review {
        Some(review) => quality.with_drafts(review),
        None => quality,
    };
    Ok(ModuleRegistry::new().register(quality))
}

/// The installation's semantic patterns (decision 0038), from
/// `QUALITY_SEMANTIC_PATTERNS` (or its `_FILE`): a JSON object with the ones
/// it changes (`email`, `mobile`, `document`, `date`, `number`, `flag`); the
/// rest are Colombia's. Each must be one DuckDB takes, or the server does
/// not start.
fn semantic_patterns() -> Result<Patterns, Box<dyn std::error::Error>> {
    patterns_from(infrastructure::oidc_es256::key_from_env(
        "QUALITY_SEMANTIC_PATTERNS",
    )?)
}

fn patterns_from(json: Option<String>) -> Result<Patterns, Box<dyn std::error::Error>> {
    let Some(json) = json else {
        return Ok(Patterns::default());
    };
    let patterns: Patterns =
        serde_json::from_str(&json).map_err(|e| format!("QUALITY_SEMANTIC_PATTERNS: {e}"))?;
    let engine = quality_rules_duckdb::Runner::new()?;
    for (kind, pattern) in patterns.all() {
        if pattern.trim().is_empty() || !engine.takes_pattern(pattern) {
            return Err(
                format!("QUALITY_SEMANTIC_PATTERNS: {kind} is not a pattern DuckDB takes").into(),
            );
        }
    }
    Ok(patterns)
}

/// Credentials given to jobs, in the audit log: the machine as the service
/// account, the job, and the job's kind as its action (decision 0030).
struct Issues(Arc<dyn AuditRepository>);

#[async_trait::async_trait]
impl IssueLog for Issues {
    async fn issued(
        &self,
        job: Uuid,
        kind: &str,
        machine: &str,
        ip: &str,
        resource: &str,
    ) -> Result<(), String> {
        self.0
            .record_action(&NewActionAudit {
                event_type: AuditEventType::CredentialsIssued,
                admin_id: None,
                service_account: Some(machine.to_string()),
                job_id: Some(job.to_string()),
                action: format!("job:{kind}"),
                resource: resource.to_string(),
                ip: ip.to_string(),
                user_agent: None,
                occurred_at: std::time::SystemTime::now(),
            })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// The sources' secrets, sealed with the installation's key.
struct BoxSealer(SecretBox);

impl Sealer for BoxSealer {
    fn seal(&self, plain: &str) -> Result<String, String> {
        self.0.encrypt(plain)
    }

    fn open(&self, sealed: &str) -> Result<String, String> {
        self.0.decrypt(sealed)
    }
}

/// Where a bronze table's files are, read from the catalog with read-only
/// credentials of its own (decision 0037).
struct LakeTables(TableLookup);

#[async_trait::async_trait]
impl TableCatalog for LakeTables {
    async fn ensure_namespace(&self, warehouse: &str, namespace: &str) -> Result<(), String> {
        let policy = quality::lake::namespace_policy(warehouse);
        self.0.ensure_namespace(warehouse, namespace, &policy).await
    }

    async fn tables(&self, warehouse: &str, namespace: &str) -> Result<Vec<String>, String> {
        let policy = quality::lake::read_only_policy(warehouse);
        self.0.tables(warehouse, namespace, &policy).await
    }

    async fn table_prefix(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<Option<String>, String> {
        let policy = quality::lake::read_only_policy(warehouse);
        self.0.prefix(warehouse, namespace, table, &policy).await
    }

    async fn drop_table(
        &self,
        warehouse: &str,
        namespace: &str,
        table: &str,
    ) -> Result<bool, String> {
        let Some(prefix) = self.table_prefix(warehouse, namespace, table).await? else {
            return Ok(false);
        };
        let policy = quality::lake::drop_policy(warehouse, namespace, table, &prefix);
        self.0
            .drop(warehouse, (namespace, table), &prefix, &policy)
            .await
            .map(|()| true)
    }
}

/// A cut's manifests, each written with credentials for that write only.
struct LakeManifests(ManifestWriter);

#[async_trait::async_trait]
impl ManifestStore for LakeManifests {
    async fn put_once(&self, warehouse: &str, key: &str, body: Vec<u8>) -> Result<bool, String> {
        let policy = quality::lake::manifests_policy(warehouse);
        self.0.put_once(warehouse, key, body, &policy).await
    }
}

/// A job's lake credentials, from RustFS's STS.
struct LakeSts(StsIssuer);

#[async_trait::async_trait]
impl LakeIssuer for LakeSts {
    async fn issue(
        &self,
        warehouse: &str,
        session: &str,
        policy: &str,
    ) -> Result<LakeAccess, String> {
        let c = self
            .0
            .assume_role(session, policy, Duration::from_secs(15 * 60))
            .await
            .map_err(|e| e.to_string())?;
        Ok(LakeAccess {
            endpoint: self.0.endpoint().to_string(),
            warehouse: warehouse.to_string(),
            access_key: c.access_key,
            secret_key: c.secret_key,
            session_token: c.session_token,
            expires_at: c.expires_at.to_rfc3339(),
        })
    }
}

/// The workers' tokens, as the provider issued them.
struct ProviderTokens(AuthorizationServer);

impl quality::worker_api::MachineTokens for ProviderTokens {
    fn client(&self, token: &str) -> Option<quality::worker_api::MachineCaller> {
        let (client, machine) = self
            .0
            .machine_caller(token, quality::worker_api::AUDIENCE)?;
        Some(quality::worker_api::MachineCaller { client, machine })
    }
}

pub(crate) async fn migrate(
    modules: &ModuleRegistry,
    pool: &PgPool,
) -> Result<MigrationReport, Box<dyn std::error::Error>> {
    let mut migrator = PgModuleMigrator::load(pool).await?;
    migrator.verify(modules.modules().iter().flat_map(|m| {
        m.migrations()
            .into_iter()
            .map(move |mig| (format!("{}::{}", m.name(), mig.name), mig.sql))
    }))?;
    let report = modules.run_migrations(&mut migrator)?;
    migrator.run(pool).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installations_patterns_must_be_ones_duckdb_takes() {
        assert_eq!(patterns_from(None).unwrap(), Patterns::default());
        let mobile = patterns_from(Some(r#"{ "mobile": "9\\d{8}" }"#.into())).unwrap();
        assert_eq!(
            (mobile.mobile.as_str(), mobile.email),
            (r"9\d{8}", Patterns::default().email)
        );
        for bad in [
            r#"{ "mobile": "(unclosed" }"#,
            r#"{ "mobile": " " }"#,
            r#"{ "phone": "x" }"#,
            "not json",
        ] {
            assert!(patterns_from(Some(bad.into())).is_err(), "{bad}");
        }
    }
}
