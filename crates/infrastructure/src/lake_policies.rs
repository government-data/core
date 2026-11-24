//! A grant's session policy, asked of the main lake's adapter and cached
//! (decision 0078, point 9, the user's ruling of 2026-11-20): keyed by the
//! grant and the protocol and version the adapter says it serves, kept for
//! [`POLICY_TTL`]. The policy is not a credential: the STS credentials made
//! with it keep their own short lifetime. A job starts on a cached policy
//! while the adapter is down; once one has expired and the adapter does
//! not answer, the call fails (the job waits and is tried again), never
//! with a broader policy. No policy is logged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use application::lake::{LakeAdmins, LakeError, LakeGrant, SessionPolicy};
use async_trait::async_trait;
use auth_gateway::Lake;

/// How long a translated policy is kept.
pub const POLICY_TTL: Duration = Duration::from_secs(3600);

/// What the adapter says it is: its protocol and version, the cache's key
/// with the grant.
#[async_trait]
pub trait AdapterVersion: Send + Sync {
    async fn version(&self) -> Result<String, LakeError>;
}

#[async_trait]
impl AdapterVersion for crate::lake_admins::AdapterLakeAdmins {
    async fn version(&self) -> Result<String, LakeError> {
        let about = self.main_about().await?;
        let protocols = about.protocols.join(",");
        Ok(format!(
            "{protocols} {} {}",
            about.implementation, about.version
        ))
    }
}

struct Kept {
    policy: SessionPolicy,
    until: Instant,
}

/// The cache, in front of the main lake's adapter.
pub struct SessionPolicies {
    admins: Arc<dyn LakeAdmins>,
    version: Arc<dyn AdapterVersion>,
    ttl: Duration,
    /// The version the adapter said last, and the policies by key.
    kept: Mutex<(Option<String>, HashMap<String, Kept>)>,
}

impl std::fmt::Debug for SessionPolicies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionPolicies").finish_non_exhaustive()
    }
}

impl SessionPolicies {
    /// Policies from the main lake's adapter of `admins`, keyed by what
    /// `version` says, kept for [`POLICY_TTL`].
    pub fn new(admins: Arc<dyn LakeAdmins>, version: Arc<dyn AdapterVersion>) -> Self {
        Self {
            admins,
            version,
            ttl: POLICY_TTL,
            kept: Mutex::default(),
        }
    }

    /// The same, keeping each policy for `ttl` (tests).
    pub fn keeping(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    fn key(version: &str, grant: &LakeGrant) -> String {
        let grant = serde_json::to_string(grant).unwrap_or_default();
        format!("{version}\n{grant}")
    }

    /// `grant`'s policy: kept if it has not expired, else asked again (and
    /// the adapter's version with it).
    pub async fn policy(&self, grant: &LakeGrant) -> Result<SessionPolicy, LakeError> {
        let now = Instant::now();
        {
            let kept = self.kept.lock().map_err(|_| poisoned())?;
            if let Some(version) = &kept.0 {
                if let Some(k) = kept.1.get(&Self::key(version, grant)) {
                    if k.until > now {
                        return Ok(k.policy.clone());
                    }
                }
            }
        }
        let version = self.version.version().await?;
        let admin = self.admins.for_lake(&Lake::main(), "");
        let policy = admin.session_policy(grant).await?;
        let mut kept = self.kept.lock().map_err(|_| poisoned())?;
        if kept.0.as_deref() != Some(version.as_str()) {
            kept.1.clear();
            kept.0 = Some(version.clone());
        }
        kept.1.retain(|_, k| k.until > now);
        let until = now + self.ttl;
        let entry = Kept {
            policy: policy.clone(),
            until,
        };
        kept.1.insert(Self::key(&version, grant), entry);
        Ok(policy)
    }
}

fn poisoned() -> LakeError {
    LakeError::Unavailable("the session policies' cache".into())
}
