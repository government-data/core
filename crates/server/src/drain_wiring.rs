//! The modules' work in flight, for the update agent (decision 0003): every
//! registered code module that counts its own work ([`contracts::Drain`]),
//! and every installed application module that declares a drain path, as
//! the update agent tells them, asked over HTTP ([`AppModules`]); each
//! within [`ANSWER_WITHIN`]. The server names none of them.

use std::sync::Arc;
use std::time::Duration;

use application::{InstalledModule, ModuleWork, ModulesWork, UpdatesService};
use async_trait::async_trait;
use contracts::{Drain, ModuleRegistry};
use infrastructure::module_drain::{DrainEndpoint, HttpModuleDrains};

/// A module that takes longer counts as not drained.
pub(crate) const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// What does not answer when the installed modules cannot be read.
const INSTALLED: &str = "installed modules";

/// `updates` with what the update agent waits for: each module's own work,
/// and each installed application module's as the agent tells them
/// (0003); and whether a module may be removed: every installed module's
/// removal path (quality-sdk 0015).
pub(crate) fn with_module_checks(
    updates: UpdatesService,
    modules: &ModuleRegistry,
) -> Result<UpdatesService, Box<dyn std::error::Error>> {
    let app_modules = AppModules::from_env(updates.clone(), ANSWER_WITHIN)?;
    let removals = infrastructure::module_removal::HttpModuleRemovals::from_env(ANSWER_WITHIN)?;
    Ok(updates
        .with_modules_work(Arc::new(CodeModules::of(modules, ANSWER_WITHIN)))
        .with_modules_work(Arc::new(app_modules))
        .with_removal_checks(Arc::new(removals)))
}

/// The registry's [`Drain`]s as the application's [`ModulesWork`].
pub(crate) struct CodeModules {
    drains: Vec<(&'static str, Arc<dyn Drain>)>,
    within: Duration,
}

impl CodeModules {
    /// Each module asked within `within` ([`ANSWER_WITHIN`] when serving).
    pub(crate) fn of(modules: &ModuleRegistry, within: Duration) -> Self {
        Self {
            drains: modules.drains(),
            within,
        }
    }
}

#[async_trait]
impl ModulesWork for CodeModules {
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)> {
        let mut answers = Vec::with_capacity(self.drains.len());
        for (name, drain) in &self.drains {
            let answer = match tokio::time::timeout(self.within, drain.work(horizon)).await {
                Ok(Ok(w)) => Ok(ModuleWork {
                    running: w.running,
                    queued: w.queued,
                    scheduled_soon: w.scheduled_soon,
                }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(format!("no answer within {:?}", self.within)),
            };
            answers.push(((*name).to_string(), answer));
        }
        answers
    }
}

/// Where the installed modules come from: the update agent's list
/// (`PUT /updater/v1/modules`).
#[async_trait]
pub(crate) trait Installed: Send + Sync {
    async fn installed(&self) -> Result<Vec<InstalledModule>, String>;
}

#[async_trait]
impl Installed for UpdatesService {
    async fn installed(&self) -> Result<Vec<InstalledModule>, String> {
        self.installed_modules().await
    }
}

/// The application modules' drain paths as the application's
/// [`ModulesWork`]: each installed module that declares a drain path,
/// asked at `<base>/modules/<name><drain_path>` (Traefik's internal entry
/// point) with the machine's certificate.
pub(crate) struct AppModules {
    installed: Arc<dyn Installed>,
    base: String,
    drains: HttpModuleDrains,
}

impl AppModules {
    /// The modules the update agent told, asked at
    /// `https://<INTERNAL_DOMAIN>:<TRAEFIK_INTERNAL_PORT>` (by default
    /// `internal.localhost` and 9443, as Traefik's) with
    /// `LAKE_CLIENT_CERT_FILE`.
    pub(crate) fn from_env(installed: UpdatesService, within: Duration) -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let domain = var("INTERNAL_DOMAIN").unwrap_or_else(|| "internal.localhost".into());
        let port = var("TRAEFIK_INTERNAL_PORT").unwrap_or_else(|| "9443".into());
        let base = format!("https://{}:{}", domain.trim(), port.trim());
        let drains = HttpModuleDrains::from_env(within)?;
        Ok(Self::of(Arc::new(installed), &base, drains))
    }

    fn of(installed: Arc<dyn Installed>, base: &str, drains: HttpModuleDrains) -> Self {
        Self {
            installed,
            base: base.trim_end_matches('/').to_string(),
            drains,
        }
    }

    fn endpoints(&self, modules: Vec<InstalledModule>) -> Vec<DrainEndpoint> {
        modules
            .into_iter()
            .filter_map(|m| {
                let path = m.drain_path?;
                let url = format!("{}/modules/{}{path}", self.base, m.name);
                Some(DrainEndpoint {
                    module: m.name,
                    url,
                })
            })
            .collect()
    }
}

#[async_trait]
impl ModulesWork for AppModules {
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)> {
        // Not knowing which modules are installed is not drained.
        let modules = match self.installed.installed().await {
            Ok(modules) => modules,
            Err(e) => return vec![(INSTALLED.to_string(), Err(e))],
        };
        self.drains
            .work(&self.endpoints(modules), horizon)
            .await
            .into_iter()
            .map(|(module, answer)| {
                let answer = answer.map(|w| ModuleWork {
                    running: w.running,
                    queued: w.queued,
                    scheduled_soon: w.scheduled_soon,
                });
                (module, answer)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use contracts::{Module, Work};

    struct Counting(Option<Duration>);

    #[async_trait]
    impl Drain for Counting {
        async fn work(&self, _horizon: Duration) -> Result<Work, String> {
            if let Some(wait) = self.0 {
                tokio::time::sleep(wait).await;
            }
            Ok(Work {
                running: 1,
                queued: 2,
                scheduled_soon: 3,
            })
        }
    }

    struct Named(&'static str, Option<Duration>);

    impl Module for Named {
        fn name(&self) -> &'static str {
            self.0
        }

        fn drain(&self) -> Option<Arc<dyn Drain>> {
            Some(Arc::new(Counting(self.1)))
        }
    }

    #[tokio::test]
    async fn a_module_that_does_not_answer_in_time_is_an_error() {
        let within = Duration::from_millis(50);
        let registry = ModuleRegistry::new()
            .register(Named("quick", None))
            .register(Named("slow", Some(Duration::from_secs(5))));
        let answers = CodeModules::of(&registry, within)
            .work(Duration::ZERO)
            .await;
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].0, "quick");
        let quick = answers[0].1.as_ref().unwrap();
        assert_eq!(
            (quick.running, quick.queued, quick.scheduled_soon),
            (1, 2, 3)
        );
        assert_eq!(answers[1].0, "slow");
        assert!(answers[1].1.is_err());
    }

    /// The installed modules, fixed.
    struct Fixed(Result<Vec<InstalledModule>, String>);

    #[async_trait]
    impl Installed for Fixed {
        async fn installed(&self) -> Result<Vec<InstalledModule>, String> {
            self.0.clone()
        }
    }

    fn installed(name: &str, drain_path: Option<&str>) -> InstalledModule {
        InstalledModule {
            name: name.into(),
            version: "1.0.0".into(),
            drain_path: drain_path.map(Into::into),
            route_path: None,
            menu: None,
            permissions: None,
            provides: Vec::new(),
            roles: Vec::new(),
            removal_path: None,
            events: None,
        }
    }

    fn app_modules(modules: Result<Vec<InstalledModule>, String>, base: &str) -> AppModules {
        let drains = HttpModuleDrains::plain(Duration::from_millis(500));
        AppModules::of(Arc::new(Fixed(modules)), base, drains)
    }

    /// A fake Traefik: `/modules/reports/drain` counts for the horizon
    /// asked; anything else is not found.
    async fn fake_entry_point() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let asked = "GET /modules/reports/drain?horizon_secs=60 ";
                let (status, body) = if request.starts_with(asked) {
                    ("200 OK", r#"{"running": 2, "scheduled_soon": 1}"#)
                } else {
                    ("404 Not Found", "{}")
                };
                let answer = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            }
        });
        base
    }

    #[tokio::test]
    async fn each_installed_module_with_a_drain_path_is_asked_under_its_name() {
        let base = fake_entry_point().await;
        let modules = vec![
            installed("quality-data", None),
            installed("reports", Some("/drain")),
            installed("grafana", Some("/drain")),
        ];
        let answers = app_modules(Ok(modules), &base)
            .work(Duration::from_secs(60))
            .await;
        let names: Vec<_> = answers.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(names, ["reports", "grafana"], "no drain path, not asked");
        let reports = answers[0].1.as_ref().unwrap();
        assert_eq!(
            (reports.running, reports.queued, reports.scheduled_soon),
            (2, 0, 1)
        );
        assert!(answers[1].1.is_err(), "grafana: not found");
    }

    #[tokio::test]
    async fn without_installed_modules_no_application_module_is_asked() {
        let modules = app_modules(Ok(Vec::new()), "http://127.0.0.1:9");
        assert!(modules.work(Duration::from_secs(60)).await.is_empty());
    }

    #[tokio::test]
    async fn not_knowing_the_installed_modules_is_not_drained() {
        let modules = app_modules(Err("database down".into()), "http://127.0.0.1:9");
        let answers = modules.work(Duration::ZERO).await;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, INSTALLED);
        assert!(answers[0].1.is_err());
    }

    #[tokio::test]
    async fn an_application_module_that_does_not_answer_is_an_error() {
        let modules = vec![installed("reports", Some("/drain"))];
        let answers = app_modules(Ok(modules), "http://127.0.0.1:9")
            .work(Duration::ZERO)
            .await;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, "reports");
        assert!(answers[0].1.is_err());
    }
}
