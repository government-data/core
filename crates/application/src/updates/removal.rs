//! Whether a module may be removed (quality-sdk decision 0015): before
//! `remove` or `purge` of a module, the update agent asks the core, which
//! asks every installed module that declares a removal path, with the
//! removed module's name, and answers the refusals. The core names no
//! module: any module may refuse its own or another's removal, with a
//! reason and counts, never data. A module that declares a path and does
//! not answer, or answers what does not read, refuses ("when in doubt,
//! keep it").

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{UpdatesError, UpdatesService};

/// The longest reason a module gives.
const MAX_REASON: usize = 500;
/// The most counts a module gives.
const MAX_COUNTS: usize = 16;

/// A module that declares a removal path, and the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovalEndpoint {
    pub module: String,
    pub path: String,
}

/// A module's answer: `{"refuse": false}` or `{"refuse": true, "reason",
/// "counts"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RemovalAnswer {
    pub refuse: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

/// A refusal, with the module that refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalRefusal {
    pub module: String,
    pub reason: String,
    pub counts: BTreeMap<String, u64>,
}

/// Port: asks the modules' removal paths with `?module=<removed>`, each
/// answer in the endpoints' order; an error is no answer.
#[async_trait]
pub trait RemovalChecks: Send + Sync {
    async fn ask(
        &self,
        endpoints: &[RemovalEndpoint],
        removed: &str,
    ) -> Vec<(String, Result<RemovalAnswer, String>)>;
}

impl UpdatesService {
    /// Ask the modules' removal paths over `checks`.
    pub fn with_removal_checks(mut self, checks: Arc<dyn RemovalChecks>) -> Self {
        self.removal = Some(checks);
        self
    }

    /// The refusals of the removal of `removed` (none: it may go).
    pub async fn removal_refusals(
        &self,
        removed: &str,
    ) -> Result<Vec<RemovalRefusal>, UpdatesError> {
        if !super::modules::is_name(removed) {
            let why = texts::tr!("app.updates.modules.badName");
            return Err(UpdatesError::Invalid("module", why));
        }
        let installed = self
            .store
            .modules()
            .await
            .map_err(UpdatesError::Unavailable)?;
        let endpoints: Vec<RemovalEndpoint> = installed
            .into_iter()
            .filter_map(|m| {
                Some(RemovalEndpoint {
                    path: m.removal_path?,
                    module: m.name,
                })
            })
            .collect();
        if endpoints.is_empty() {
            return Ok(Vec::new());
        }
        let answers = match &self.removal {
            Some(checks) => checks.ask(&endpoints, removed).await,
            None => endpoints
                .iter()
                .map(|e| (e.module.clone(), Err("not asked".to_string())))
                .collect(),
        };
        Ok(answers.into_iter().filter_map(refusal).collect())
    }
}

/// The refusal in `answer`, if any; no answer, or one that does not read,
/// refuses.
fn refusal((module, answer): (String, Result<RemovalAnswer, String>)) -> Option<RemovalRefusal> {
    match answer {
        Ok(a) if !a.refuse => None,
        Ok(a) if readable(&a) => Some(RemovalRefusal {
            module,
            reason: a.reason.unwrap_or_default(),
            counts: a.counts,
        }),
        Ok(_) => {
            let reason = texts::tr!("app.updates.removal.unreadable", name = module);
            Some(RemovalRefusal {
                module,
                reason,
                counts: BTreeMap::new(),
            })
        }
        Err(e) => {
            tracing::warn!(module = %module, "removal check: {e}");
            let reason = texts::tr!("app.updates.removal.noAnswer", name = module);
            Some(RemovalRefusal {
                module,
                reason,
                counts: BTreeMap::new(),
            })
        }
    }
}

/// A reason of printable text and counts named `[a-z_]`.
fn readable(a: &RemovalAnswer) -> bool {
    let reason_ok = a.reason.as_deref().is_some_and(|r| {
        !r.trim().is_empty() && r.chars().count() <= MAX_REASON && !r.chars().any(char::is_control)
    });
    let counts_ok = a.counts.len() <= MAX_COUNTS
        && a.counts.keys().all(|k| {
            (1..=63).contains(&k.len()) && k.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        });
    reason_ok && counts_ok
}

#[cfg(test)]
#[path = "removal_tests.rs"]
mod tests;
