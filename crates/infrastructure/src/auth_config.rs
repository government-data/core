//! The authentication settings every binary that signs people in reads from
//! the environment (`server`, `cuenta`): password, lockout and session
//! policies, the argon2 cost, the login rate limit and the install routes'.

use std::str::FromStr;
use std::time::Duration;

use auth_gateway::{LockoutPolicy, PasswordPolicy, SessionPolicy};
use http_kit::rate_limit::RateLimitConfig;

use crate::Argon2Params;

/// The install routes' requests per address and minute by default: a run of
/// `join.sh` for the role that asks most, the lake replica, makes four (its
/// compose file, two entrypoints, its root keys), so thirty let seven whole
/// runs through a minute from one address (servers behind one NAT, a run
/// repeated after a refusal), where the login's ten let two.
pub const INSTALL_RATE_LIMIT_MAX_REQUESTS: u32 = 30;

/// Authentication configuration: the password policy, lockout policy, and argon2
/// cost parameters, all sourced from the environment so an operator tunes them
/// without a rebuild. The secrets themselves (the bootstrap password) are read
/// separately at the point of use, never held here.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Argon2id cost parameters (OWASP defaults).
    pub argon2: Argon2Params,
    /// The password-strength policy enforced at bootstrap.
    pub password_policy: PasswordPolicy,
    /// The progressive lockout policy applied on failed logins.
    pub lockout_policy: LockoutPolicy,
    /// The session idle/absolute expiration policy.
    pub session_policy: SessionPolicy,
    /// The app-level rate limit on login attempts, per IP and per account;
    /// joining a machine (`/install/join`) is limited by it too.
    pub login_rate_limit: RateLimitConfig,
    /// The app-level rate limit per IP on the install routes that check a
    /// join token (decision 0066 step 11).
    pub install_rate_limit: RateLimitConfig,
    /// How long a password-reset link works.
    pub password_reset_ttl: Duration,
}

impl AuthConfig {
    /// Read the auth configuration from the environment.
    ///
    /// Password policy:
    /// - `ADMIN_PASSWORD_MIN_LENGTH` (default `12`)
    /// - `ADMIN_PASSWORD_REQUIRE_UPPERCASE` / `_LOWERCASE` / `_DIGIT` (default `true`)
    /// - `ADMIN_PASSWORD_REQUIRE_SYMBOL` (default `false`)
    ///
    /// Lockout policy:
    /// - `ADMIN_LOCKOUT_MAX_ATTEMPTS` (default `5`)
    /// - `ADMIN_LOCKOUT_BASE_SECONDS` (default `60`)
    /// - `ADMIN_LOCKOUT_MAX_SECONDS` (default `3600`)
    ///
    /// Session policy:
    /// - `SESSION_IDLE_TIMEOUT_SECS` (default `1800`, 30 minutes)
    /// - `SESSION_ABSOLUTE_TIMEOUT_SECS` (default `43200`, 12 hours)
    ///
    /// Login rate limit (app-level, independent of Traefik's edge limit;
    /// applied separately per client IP and per submitted account email):
    /// - `LOGIN_RATE_LIMIT_MAX_REQUESTS` (default `10`)
    /// - `LOGIN_RATE_LIMIT_WINDOW_SECS` (default `60`)
    ///
    /// Install rate limit (app-level, per client IP, on the routes that check
    /// a join token: a role's compose file, its entrypoints and a lake's root
    /// keys; `join.sh` and the CA are not limited, joining keeps the login's
    /// limit):
    /// - `INSTALL_RATE_LIMIT_MAX_REQUESTS` (default
    ///   [`INSTALL_RATE_LIMIT_MAX_REQUESTS`])
    /// - `INSTALL_RATE_LIMIT_WINDOW_SECS` (default `60`)
    ///
    /// Password recovery:
    /// - `PASSWORD_RESET_TTL_MINUTES` (default `30`): how long an emailed
    ///   reset link works
    ///
    /// Argon2 parameters follow [`Argon2Params::from_env`] (OWASP defaults). A
    /// present-but-unparseable value is an error, never a silent fallback.
    pub fn from_env() -> Result<Self, AuthConfigError> {
        let recommended = PasswordPolicy::recommended();
        let password_policy = PasswordPolicy {
            min_length: parse_var("ADMIN_PASSWORD_MIN_LENGTH", recommended.min_length)?,
            require_uppercase: parse_bool(
                "ADMIN_PASSWORD_REQUIRE_UPPERCASE",
                recommended.require_uppercase,
            )?,
            require_lowercase: parse_bool(
                "ADMIN_PASSWORD_REQUIRE_LOWERCASE",
                recommended.require_lowercase,
            )?,
            require_digit: parse_bool("ADMIN_PASSWORD_REQUIRE_DIGIT", recommended.require_digit)?,
            require_symbol: parse_bool(
                "ADMIN_PASSWORD_REQUIRE_SYMBOL",
                recommended.require_symbol,
            )?,
        };

        let lockout = LockoutPolicy::recommended();
        let lockout_policy = LockoutPolicy {
            max_attempts: parse_var("ADMIN_LOCKOUT_MAX_ATTEMPTS", lockout.max_attempts)?,
            base_delay: Duration::from_secs(parse_var(
                "ADMIN_LOCKOUT_BASE_SECONDS",
                lockout.base_delay.as_secs(),
            )?),
            max_delay: Duration::from_secs(parse_var(
                "ADMIN_LOCKOUT_MAX_SECONDS",
                lockout.max_delay.as_secs(),
            )?),
        };

        let argon2 = Argon2Params::from_env()
            .map_err(|e| AuthConfigError::InvalidValue("ARGON2_*".to_string(), e.to_string()))?;

        let recommended_session = SessionPolicy::recommended();
        let session_policy = SessionPolicy {
            idle_timeout: Duration::from_secs(parse_var(
                "SESSION_IDLE_TIMEOUT_SECS",
                recommended_session.idle_timeout.as_secs(),
            )?),
            absolute_timeout: Duration::from_secs(parse_var(
                "SESSION_ABSOLUTE_TIMEOUT_SECS",
                recommended_session.absolute_timeout.as_secs(),
            )?),
        };

        let login_rate_limit = RateLimitConfig {
            max_requests: parse_var("LOGIN_RATE_LIMIT_MAX_REQUESTS", 10)?,
            window: Duration::from_secs(parse_var("LOGIN_RATE_LIMIT_WINDOW_SECS", 60)?),
        };

        let install_rate_limit = RateLimitConfig {
            max_requests: parse_var(
                "INSTALL_RATE_LIMIT_MAX_REQUESTS",
                INSTALL_RATE_LIMIT_MAX_REQUESTS,
            )?,
            window: Duration::from_secs(parse_var("INSTALL_RATE_LIMIT_WINDOW_SECS", 60)?),
        };

        let password_reset_ttl =
            Duration::from_secs(parse_var::<u64>("PASSWORD_RESET_TTL_MINUTES", 30)?.max(1) * 60);

        Ok(Self {
            argon2,
            password_policy,
            lockout_policy,
            session_policy,
            login_rate_limit,
            install_rate_limit,
            password_reset_ttl,
        })
    }
}

/// Parse an optional environment variable, falling back to `default` when unset.
fn parse_var<T>(key: &str, default: T) -> Result<T, AuthConfigError>
where
    T: FromStr,
{
    match std::env::var(key) {
        Ok(raw) => raw
            .parse()
            .map_err(|_| AuthConfigError::InvalidValue(key.to_string(), raw)),
        Err(_) => Ok(default),
    }
}

/// Parse a boolean environment variable, accepting `true/false/1/0/yes/no`.
fn parse_bool(key: &str, default: bool) -> Result<bool, AuthConfigError> {
    match std::env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(AuthConfigError::InvalidValue(key.to_string(), raw)),
        },
        Err(_) => Ok(default),
    }
}

/// A malformed authentication configuration value.
#[derive(Debug)]
pub enum AuthConfigError {
    /// A setting was present but could not be parsed.
    InvalidValue(String, String),
}

impl std::fmt::Display for AuthConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue(key, value) => write!(f, "invalid {key}: {value:?}"),
        }
    }
}

impl std::error::Error for AuthConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_routes_have_a_limit_of_their_own() {
        let (max, window) = (
            "INSTALL_RATE_LIMIT_MAX_REQUESTS",
            "INSTALL_RATE_LIMIT_WINDOW_SECS",
        );
        std::env::remove_var(max);
        std::env::remove_var(window);
        let config = AuthConfig::from_env().unwrap();
        assert_eq!(config.install_rate_limit.max_requests, 30);
        assert_eq!(config.install_rate_limit.window, Duration::from_secs(60));
        std::env::set_var(max, "8");
        std::env::set_var(window, "120");
        let config = AuthConfig::from_env().unwrap();
        assert_eq!(config.install_rate_limit.max_requests, 8);
        assert_eq!(config.install_rate_limit.window, Duration::from_secs(120));
        std::env::set_var(max, "many");
        assert!(AuthConfig::from_env().is_err());
        std::env::remove_var(max);
        std::env::remove_var(window);
    }
}
