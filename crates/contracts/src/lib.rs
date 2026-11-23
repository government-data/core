//! contracts — the shared contract for pluggable feature modules.
//!
//! The backend is composed from independent feature *modules*. Each one
//! implements the [`Module`] trait to expose three seams: its migrations, its
//! runtime initialization, and its HTTP router. The composition root (`server`)
//! collects them in a [`ModuleRegistry`], runs their migrations (isolated per
//! module and ordered), initializes them, and merges their routers — all
//! through `dyn Module`, so it never depends on a concrete feature and no
//! feature depends on another.
//!
//! This crate is deliberately thin: it is the ONE thing every module shares. It
//! knows `axum` (a module's contract is to expose a `Router`) but holds no
//! domain model and no business logic.
//!
//! # Adding and removing modules
//!
//! Adding a feature is one line in the compositor —
//! `registry.register(my_feature::Module::new(..))` — plus the module's own
//! crate. Removing a feature is the reverse: drop its crate, its
//! `workspace.dependencies` entry, and that one registration line. Because
//! modules only ever meet through this crate, removing one cannot break the
//! compilation of the others.

mod access;
mod drain;
pub mod lake_admin;
mod lake_grant;
mod migration;
mod module;
mod registry;
mod view;

pub use access::{Access, AuditEntry, AuditTrail, Caller, Change};
pub use drain::{Drain, Work};
pub use lake_grant::{LakeGrant, LakeGrantError, LakeRead, LakeTable};
pub use migration::{
    InMemoryExecutor, Migration, MigrationError, MigrationExecutor, MigrationReport,
};
pub use module::{InitError, Module};
pub use registry::ModuleRegistry;
pub use view::{
    console_column, ReasonError, Reference, ViewDetail, ViewPurpose, ViewReason, COLUMN_NAME_MAX,
    REFERENCE_MAX,
};

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use utoipa::openapi::path::{HttpMethod, OperationBuilder, PathItem};
    use utoipa::openapi::{Info, OpenApi, Paths};

    /// A configurable stand-in used to exercise the registry without pulling in
    /// a real feature crate.
    #[derive(Debug)]
    struct StubModule {
        name: &'static str,
        migrations: Vec<Migration>,
        init_result: Result<(), &'static str>,
    }

    impl StubModule {
        fn new(name: &'static str, migrations: Vec<Migration>) -> Self {
            Self {
                name,
                migrations,
                init_result: Ok(()),
            }
        }

        fn failing_init(name: &'static str) -> Self {
            Self {
                name,
                migrations: Vec::new(),
                init_result: Err("boom"),
            }
        }
    }

    impl Module for StubModule {
        fn name(&self) -> &'static str {
            self.name
        }

        fn migrations(&self) -> Vec<Migration> {
            self.migrations.clone()
        }

        fn init(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            self.init_result.map_err(Into::into)
        }

        fn router(&self) -> Router {
            Router::new().route(&format!("/{}/ping", self.name), get(|| async { "pong" }))
        }

        fn openapi(&self) -> OpenApi {
            let ping = PathItem::new(HttpMethod::Get, OperationBuilder::new().build());
            OpenApi::new(
                Info::new(self.name, "0"),
                Paths::builder().path(format!("/{}/ping", self.name), ping),
            )
        }
    }

    fn two_migrations() -> Vec<Migration> {
        vec![
            Migration::new("0001_init", "SQL_ONE"),
            Migration::new("0002_more", "SQL_TWO"),
        ]
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn migrations_run_isolated_per_module_and_in_order() {
        let registry = ModuleRegistry::new()
            .register(StubModule::new("alpha", two_migrations()))
            .register(StubModule::new("beta", two_migrations()));

        let mut executor = InMemoryExecutor::default();
        let report = registry.run_migrations(&mut executor).expect("first run");

        // Registration order across modules, declared order within each module,
        // and every id namespaced by its module — so `alpha`'s `0001` and
        // `beta`'s `0001` both apply without colliding.
        assert_eq!(
            executor.applied(),
            ids(&[
                "alpha::0001_init",
                "alpha::0002_more",
                "beta::0001_init",
                "beta::0002_more",
            ])
            .as_slice()
        );
        assert_eq!(report.applied.len(), 4);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn rerunning_migrations_is_idempotent() {
        let registry = ModuleRegistry::new().register(StubModule::new("alpha", two_migrations()));

        let mut executor = InMemoryExecutor::default();
        registry.run_migrations(&mut executor).expect("first run");
        let second = registry.run_migrations(&mut executor).expect("second run");

        // Nothing pending the second time: the executor's record makes the run a
        // no-op that only reports skips.
        assert!(second.applied.is_empty());
        assert_eq!(
            second.skipped,
            ids(&["alpha::0001_init", "alpha::0002_more"])
        );
        assert_eq!(executor.applied().len(), 2);
    }

    #[test]
    fn duplicate_module_names_are_rejected_before_applying_anything() {
        let registry = ModuleRegistry::new()
            .register(StubModule::new("dup", two_migrations()))
            .register(StubModule::new("dup", two_migrations()));

        let mut executor = InMemoryExecutor::default();
        let error = registry.run_migrations(&mut executor).unwrap_err();

        assert!(matches!(error, MigrationError::DuplicateModule(name) if name == "dup"));
        assert!(
            executor.applied().is_empty(),
            "the uniqueness check must run before any migration is applied"
        );
    }

    #[test]
    fn init_runs_each_module_and_names_the_one_that_fails() {
        ModuleRegistry::new()
            .register(StubModule::new("healthy", Vec::new()))
            .init()
            .expect("a module whose init returns Ok must not fail the registry");

        let error = ModuleRegistry::new()
            .register(StubModule::failing_init("broken"))
            .init()
            .unwrap_err();
        assert_eq!(error.module, "broken");
    }

    #[test]
    fn router_merges_every_module_onto_the_base() {
        let registry = ModuleRegistry::new()
            .register(StubModule::new("alpha", Vec::new()))
            .register(StubModule::new("beta", Vec::new()));

        // Building the router without a panic proves the module routes merge
        // cleanly onto the base (no path collisions).
        let _app: Router = registry.router(Router::new().route("/health", get(|| async { "ok" })));
        assert_eq!(registry.modules().len(), 2);
    }

    #[test]
    fn openapi_merges_every_modules_paths_into_the_base() {
        let registry = ModuleRegistry::new()
            .register(StubModule::new("alpha", Vec::new()))
            .register(StubModule::new("beta", Vec::new()));

        let base = OpenApi::new(Info::new("installation", "1"), Paths::new());
        let doc = registry.openapi(base);
        // The base's own description stays; the modules bring their paths.
        assert_eq!(doc.info.title, "installation");
        let paths: Vec<_> = doc.paths.paths.keys().cloned().collect();
        assert_eq!(paths, ["/alpha/ping", "/beta/ping"]);
    }

    /// A module with work of its own.
    struct Busy;

    #[async_trait::async_trait]
    impl Drain for Busy {
        async fn work(&self, _horizon: std::time::Duration) -> Result<Work, String> {
            Ok(Work {
                running: 2,
                ..Work::default()
            })
        }
    }

    impl Module for Busy {
        fn name(&self) -> &'static str {
            "busy"
        }

        fn drain(&self) -> Option<std::sync::Arc<dyn Drain>> {
            Some(std::sync::Arc::new(Busy))
        }
    }

    #[tokio::test]
    async fn drains_lists_only_the_modules_that_count_their_work() {
        let registry = ModuleRegistry::new()
            .register(StubModule::new("alpha", Vec::new()))
            .register(Busy);
        let drains = registry.drains();
        assert_eq!(drains.len(), 1);
        assert_eq!(drains[0].0, "busy");
        let work = drains[0].1.work(std::time::Duration::ZERO).await.unwrap();
        assert_eq!(work.running, 2);
    }
}
