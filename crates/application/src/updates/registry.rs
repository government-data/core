//! The registry's pull credential (quality-sdk decision 0013): the update
//! agent asks the installation's registry for a token (it lasts 12 hours)
//! and hands it here (`PUT /updater/v1/registry`); the core passes it only
//! to joined machines of a role an installed module declares. It is kept
//! **in memory only**, replaced by each one handed, forgotten at its
//! expiry: never in the database, a log, the audit or an answer but the
//! machines'.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Utc};

use super::{UpdatesError, UpdatesService};

/// The longest a token may still last when handed: the registry's 12
/// hours, and 5 minutes of clocks apart.
pub(super) const MAX_TOKEN_LIFE: Duration = Duration::minutes(12 * 60 + 5);

/// A registry token as the agent hands it.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistryToken {
    /// The registry's DNS name, with no scheme, port or path.
    pub host: String,
    pub username: String,
    pub password: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for RegistryToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryToken")
            .field("host", &self.host)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// The token held, shared by every clone of the service.
#[derive(Default)]
pub(super) struct Held(Arc<RwLock<Option<RegistryToken>>>);

impl Clone for Held {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl UpdatesService {
    /// Hold `token` in place of the one before; an invalid one is refused
    /// and the one before is kept.
    pub fn hold_registry_token(&self, token: RegistryToken) -> Result<(), UpdatesError> {
        check(&token, Utc::now()).map_err(|e| UpdatesError::Invalid("registry", e))?;
        *self.registry.0.write().unwrap_or_else(|p| p.into_inner()) = Some(token);
        Ok(())
    }

    /// The token held, if it has not expired; an expired one is forgotten.
    pub fn registry_token(&self) -> Option<RegistryToken> {
        let now = Utc::now();
        let mut held = self.registry.0.write().unwrap_or_else(|p| p.into_inner());
        if held.as_ref().is_some_and(|t| t.expires_at <= now) {
            *held = None;
        }
        held.clone()
    }
}

fn check(token: &RegistryToken, now: DateTime<Utc>) -> Result<(), String> {
    if !is_dns_name(&token.host) {
        return Err(texts::tr!("app.updates.registry.badHost"));
    }
    let printable = |s: &str, space: bool| {
        s.chars()
            .all(|c| c.is_ascii_graphic() || (space && c == ' '))
    };
    if !(1..=64).contains(&token.username.len()) || !printable(&token.username, false) {
        return Err(texts::tr!("app.updates.registry.badUsername"));
    }
    if !(1..=16384).contains(&token.password.len()) || !printable(&token.password, true) {
        return Err(texts::tr!("app.updates.registry.badPassword"));
    }
    if token.expires_at <= now || token.expires_at > now + MAX_TOKEN_LIFE {
        return Err(texts::tr!("app.updates.registry.badExpiry"));
    }
    Ok(())
}

/// A DNS name: at most 253, labels of letters, digits and `-` that neither
/// start nor end with `-`.
fn is_dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::updates::tests::MemoryMaintenance;

    pub(crate) fn token(lasts: Duration) -> RegistryToken {
        RegistryToken {
            host: "123456789012.dkr.ecr.us-east-1.amazonaws.com".into(),
            username: "AWS".into(),
            password: "c2VjcmV0".into(),
            expires_at: Utc::now() + lasts,
        }
    }

    fn service() -> UpdatesService {
        UpdatesService::new(Arc::new(MemoryMaintenance::default()))
    }

    #[test]
    fn a_token_is_held_by_every_clone_and_replaced_by_the_next() {
        let service = service();
        assert_eq!(service.registry_token(), None, "none before the agent");
        let first = token(Duration::hours(12));
        service.hold_registry_token(first.clone()).unwrap();
        assert_eq!(service.clone().registry_token(), Some(first));
        let next = RegistryToken {
            password: "bmV4dA==".into(),
            ..token(Duration::hours(6))
        };
        service.hold_registry_token(next.clone()).unwrap();
        assert_eq!(service.registry_token(), Some(next));
    }

    #[test]
    fn an_invalid_token_is_refused_and_the_one_held_kept() {
        let service = service();
        let held = token(Duration::hours(1));
        service.hold_registry_token(held.clone()).unwrap();
        let long = "a".repeat(64);
        let bad = [
            RegistryToken {
                host: "https://registry".into(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                host: "registry:5000".into(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                host: "registry/path".into(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                host: format!("{long}.com"),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                username: String::new(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                username: "A WS".into(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                username: "a".repeat(65),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                password: String::new(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                password: "ñ".into(),
                ..token(Duration::hours(1))
            },
            RegistryToken {
                password: "a".repeat(16385),
                ..token(Duration::hours(1))
            },
            token(Duration::seconds(-1)),
            token(Duration::minutes(12 * 60 + 6)),
        ];
        for t in bad {
            let refused = service.hold_registry_token(t.clone());
            assert!(
                matches!(refused, Err(UpdatesError::Invalid("registry", _))),
                "{t:?}"
            );
        }
        assert_eq!(service.registry_token(), Some(held));
    }

    #[test]
    fn an_expired_token_is_forgotten() {
        let service = service();
        let soon = token(Duration::milliseconds(50));
        service.hold_registry_token(soon).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(service.registry_token(), None);
    }

    #[test]
    fn the_debug_shows_no_password() {
        let shown = format!("{:?}", token(Duration::hours(1)));
        assert!(!shown.contains("c2VjcmV0"), "{shown}");
        assert!(shown.contains("amazonaws.com"));
    }
}
