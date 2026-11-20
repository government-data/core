//! The options of `oidc-client` and `oidc-client-update`, each checked on
//! its own; whether they fit together is the domain's
//! (`ClientSettings::check`) and the commands'.

use auth_gateway::{ClientSettings, MachineRole, SigningAlg};

/// The options of both commands; `None` leaves a setting as it is.
#[derive(Debug, Default)]
pub(super) struct Flags {
    pub(super) positional: Vec<String>,
    pub(super) public: bool,
    pub(super) name: Option<String>,
    pub(super) redirect_uris: Vec<String>,
    pub(super) home: Option<Option<String>>,
    pub(super) detail: Option<String>,
    pub(super) icon: Option<String>,
    alg: Option<SigningAlg>,
    pkce: Option<bool>,
    required_policy: Option<Option<String>>,
    pub(super) client_credentials: Option<bool>,
    /// Replace the audiences, if any is given.
    audiences: Vec<String>,
    service_account: Option<Option<String>>,
    /// The role of the machines whose certificate authenticates the client
    /// in place of a secret (decision 0066).
    pub(super) machine_role: Option<MachineRole>,
}

impl Flags {
    /// Parse `args`; `update` allows the options that only change a client.
    pub(super) fn parse(args: &[String], update: bool) -> Result<Self, String> {
        let mut f = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            match arg.as_str() {
                "--public" if !update => f.public = true,
                "--name" if update => f.name = Some(value()?),
                "--redirect-uri" if update => f.redirect_uris.push(value()?),
                "--home" => f.home = Some(Some(value()?)),
                "--no-home" if update => f.home = Some(None),
                "--detail" => f.detail = Some(value()?),
                "--icon" => f.icon = Some(value()?),
                "--alg" => {
                    let alg = value()?;
                    f.alg = Some(
                        SigningAlg::parse(&alg)
                            .ok_or_else(|| format!("--alg {alg}: EdDSA or ES256"))?,
                    );
                }
                "--pkce" if update => f.pkce = Some(true),
                "--no-pkce" => f.pkce = Some(false),
                "--require-policy" => f.required_policy = Some(Some(value()?)),
                "--no-require-policy" if update => f.required_policy = Some(None),
                "--client-credentials" => f.client_credentials = Some(true),
                "--no-client-credentials" if update => f.client_credentials = Some(false),
                "--audience" => f.audiences.push(value()?),
                "--service-account" => f.service_account = Some(Some(value()?)),
                "--no-service-account" if update => f.service_account = Some(None),
                "--machine-role" => {
                    let role = value()?;
                    let parsed = MachineRole::parse(&role).filter(|r| r.joins());
                    f.machine_role = Some(parsed.ok_or_else(|| {
                        format!("--machine-role {role}: {}", role_names().join(", "))
                    })?);
                }
                other if other.starts_with("--") => return Err(format!("unknown option {other}")),
                _ => f.positional.push(arg.clone()),
            }
        }
        Ok(f)
    }

    /// Apply the settings options to `settings`, each checked on its own;
    /// whether they fit together is [`ClientSettings::check`]'s.
    pub(super) fn apply(&self, settings: &mut ClientSettings) -> Result<(), String> {
        for url in self.home.iter().flatten().chain(self.icon.iter()) {
            if !url.starts_with("https://") {
                return Err(format!("{url}: the start address and the icon are https"));
            }
        }
        if let Some(alg) = self.alg {
            settings.token_alg = alg;
        }
        if let Some(pkce) = self.pkce {
            settings.pkce_required = pkce;
        }
        if let Some(policy) = &self.required_policy {
            if let Some(name) = policy {
                check_policy_name(name)?;
            }
            settings.required_policy.clone_from(policy);
        }
        if let Some(on) = self.client_credentials {
            settings.client_credentials = on;
        }
        if !self.audiences.is_empty() {
            for a in &self.audiences {
                if a.is_empty() || a.len() > 200 || a.chars().any(char::is_whitespace) {
                    return Err(format!(
                        "{a:?}: an audience is 1 to 200 characters, no spaces"
                    ));
                }
            }
            settings.audiences.clone_from(&self.audiences);
        }
        if let Some(account) = &self.service_account {
            if let Some(id) = account {
                check_uuid(id)?;
            }
            settings.service_account.clone_from(account);
        }
        Ok(())
    }
}

fn role_names() -> Vec<&'static str> {
    // Not the application VM's: it gets no OIDC client (0066, point 15).
    let roles = MachineRole::ALL.iter().filter(|r| r.joins());
    roles.map(|r| r.as_str()).collect()
}

fn check_policy_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let valid = name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{name:?}: a policy name is lowercase letters, digits and '-'"
        ))
    }
}

fn check_uuid(id: &str) -> Result<(), String> {
    let valid = id.len() == 36
        && id.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        });
    if valid {
        Ok(())
    } else {
        Err(format!("{id:?}: a service account id is a UUID"))
    }
}

pub(super) fn check_redirect_uris(uris: &[String]) -> Result<(), String> {
    for uri in uris {
        let local = uri.starts_with("http://localhost") || uri.starts_with("http://127.0.0.1");
        if !(uri.starts_with("https://") || local) || uri.contains('#') {
            return Err(format!(
                "{uri}: a redirect URI is https (or http://localhost), without #"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_role_is_one_of_the_roles() {
        let f = Flags::parse(&args("qd-worker W --machine-role worker"), false).unwrap();
        assert_eq!(f.machine_role, Some(MachineRole::Worker));
        let f = Flags::parse(&args("qd-worker --machine-role lake"), true).unwrap();
        assert_eq!(f.machine_role, Some(MachineRole::Lake));
        assert!(Flags::parse(&args("a --machine-role admin"), false).is_err());
        assert!(Flags::parse(&args("a --machine-role"), false).is_err());
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_flags_set_the_settings_and_leave_the_rest() {
        let f = Flags::parse(
            &args("trino Trino https://t/cb --alg ES256 --no-pkce --require-policy quality-admin"),
            false,
        )
        .unwrap();
        assert_eq!(f.positional, args("trino Trino https://t/cb"));
        let mut s = ClientSettings::default();
        f.apply(&mut s).unwrap();
        assert_eq!(s.token_alg, SigningAlg::ES256);
        assert!(!s.pkce_required);
        assert_eq!(s.required_policy.as_deref(), Some("quality-admin"));
        assert!(!s.client_credentials);

        let f = Flags::parse(
            &args("w --client-credentials --audience trino --audience lake"),
            true,
        )
        .unwrap();
        f.apply(&mut s).unwrap();
        assert!(s.client_credentials);
        assert_eq!(s.audiences, args("trino lake"));
        // Untouched by this update.
        assert_eq!(s.token_alg, SigningAlg::ES256);
        let f = Flags::parse(&args("w --pkce --no-require-policy"), true).unwrap();
        f.apply(&mut s).unwrap();
        assert!(s.pkce_required);
        assert_eq!(s.required_policy, None);
    }

    #[test]
    fn wrong_or_misplaced_flags_are_refused() {
        assert!(Flags::parse(&args("a --alg RS256"), false).is_err());
        assert!(Flags::parse(&args("a --audience"), false).is_err());
        assert!(Flags::parse(&args("a --frobnicate"), false).is_err());
        // Only an update changes the name or turns things back on.
        assert!(Flags::parse(&args("a --name B"), false).is_err());
        assert!(Flags::parse(&args("a --pkce"), false).is_err());
        assert!(Flags::parse(&args("a --public"), true).is_err());
        let mut s = ClientSettings::default();
        for bad in [
            "a --require-policy Admin",
            "a --service-account sa-1",
            "a --home http://x",
        ] {
            let f = Flags::parse(&args(bad), false).unwrap();
            assert!(f.apply(&mut s).is_err(), "{bad}");
        }
        let f = Flags::parse(
            &args("a --service-account 6f1c2a4e-0b7d-4c55-9a3e-2d8f7b1e9c01"),
            false,
        )
        .unwrap();
        assert!(f.apply(&mut s).is_ok());
    }
}
