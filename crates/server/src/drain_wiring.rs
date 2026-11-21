//! The code modules' work in flight, for the update agent (decision 0003):
//! every registered module that counts its own work ([`contracts::Drain`])
//! is asked, each within [`ANSWER_WITHIN`]. The server names none of them.

use std::sync::Arc;
use std::time::Duration;

use application::{ModuleWork, ModulesWork};
use async_trait::async_trait;
use contracts::{Drain, ModuleRegistry};

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
}
