//! The modules' work in flight, for the update agent (decision 0003): every
//! registered code module that counts its own work ([`contracts::Drain`]),
//! and every application module that declares a drain path, asked over HTTP
//! ([`AppModules`]); each within [`ANSWER_WITHIN`]. The server names none
//! of them.

use std::sync::Arc;
use std::time::Duration;

use application::{ModuleWork, ModulesWork};
use async_trait::async_trait;
use contracts::{Drain, ModuleRegistry};
use infrastructure::module_drain::{DrainEndpoint, HttpModuleDrains};

/// A module that takes longer counts as not drained.
pub(crate) const ANSWER_WITHIN: Duration = Duration::from_secs(5);

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

/// The application modules' drain paths as the application's
/// [`ModulesWork`].
pub(crate) struct AppModules(HttpModuleDrains);

impl AppModules {
    /// The installed application modules' drain paths. None yet: the core
    /// does not read the installed modules' manifests until decision 0002
    /// says how, so no application module is asked.
    pub(crate) fn installed(within: Duration) -> Result<Self, String> {
        Self::of(Vec::new(), within)
    }

    fn of(endpoints: Vec<DrainEndpoint>, within: Duration) -> Result<Self, String> {
        HttpModuleDrains::new(endpoints, within).map(Self)
    }
}

#[async_trait]
impl ModulesWork for AppModules {
    async fn work(&self, horizon: Duration) -> Vec<(String, Result<ModuleWork, String>)> {
        self.0
            .work(horizon)
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

    #[tokio::test]
    async fn without_installed_modules_no_application_module_is_asked() {
        let modules = AppModules::installed(ANSWER_WITHIN).unwrap();
        assert!(modules.work(Duration::from_secs(60)).await.is_empty());
    }

    #[tokio::test]
    async fn an_application_module_that_does_not_answer_is_an_error() {
        let unreachable = DrainEndpoint {
            module: "reports".into(),
            url: "http://127.0.0.1:9/drain".into(),
        };
        let modules = AppModules::of(vec![unreachable], Duration::from_millis(200)).unwrap();
        let answers = modules.work(Duration::ZERO).await;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, "reports");
        assert!(answers[0].1.is_err());
    }
}
