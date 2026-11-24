//! Sign-in against a corporate directory (LDAP), alongside local accounts.

use super::*;

/// Where [`LoginService`] finds its directory, shared by every clone so the
/// settings can turn LDAP on, off or point it elsewhere without a restart.
#[derive(Clone, Default)]
pub struct DirectorySlot(domain::slot::Slot<Option<Arc<DirectoryLogin>>>);

impl DirectorySlot {
    /// The directory in use right now, if any.
    pub fn get(&self) -> Option<Arc<DirectoryLogin>> {
        self.0.get().flatten()
    }

    /// Replace the directory; `None` turns directory sign-in off.
    pub fn set(&self, directory: Option<DirectoryLogin>) {
        self.0.set(directory.map(Arc::new));
    }
}

impl std::fmt::Debug for DirectorySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DirectorySlot").field(&self.get()).finish()
    }
}

/// Sign-in against a corporate directory, alongside local accounts.
#[derive(Clone)]
pub struct DirectoryLogin {
    /// Verifies the credentials.
    pub authenticator: Arc<dyn DirectoryAuthenticator>,
    /// Creates/refreshes the local account and its group permissions.
    pub accounts: Arc<dyn DirectoryAccounts>,
    /// Group DNs (any case) whose members get the global `admin` role.
    pub admin_groups: Vec<String>,
}

impl std::fmt::Debug for DirectoryLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryLogin")
            .field("admin_groups", &self.admin_groups)
            .finish_non_exhaustive()
    }
}

/// The global role of a directory user who is not in an admin group: their
/// permissions come entirely from the areas their groups map to.
pub const DIRECTORY_USER_ROLE: &str = "user";

impl LoginService {
    /// The directory half of [`Self::login`].
    ///
    /// The directory decides whether the password is right; this side keeps
    /// the same lockout bookkeeping as a local login, refuses people whose
    /// groups grant nothing here, and refreshes the local account and its area
    /// permissions on every success — so removing someone from a group in AD
    /// takes effect on their next sign-in.
    pub(super) async fn directory_login(
        &self,
        directory: &DirectoryLogin,
        request: &LoginRequest,
        ip: &str,
        ip_state: &crate::LockoutState,
        now: std::time::SystemTime,
        record: bool,
    ) -> Result<AuthenticatedAdmin, LoginError> {
        let internal = |e: &dyn std::fmt::Display| LoginError::Internal(e.to_string());

        // A known directory account that is locked stays locked, even with the
        // right password — same rule as a local account.
        let known = match Email::parse(&request.email) {
            Ok(email) => self
                .repo
                .find_by_email(&email)
                .await
                .map_err(|e| internal(&e))?,
            Err(_) => None,
        };
        if let Some(account) = &known {
            if self.policy.is_locked(&account.lockout, now) {
                if record {
                    self.record_ip_failure(ip, ip_state, now).await?;
                }
                return Err(self.too_many_attempts(&account.lockout, now));
            }
        }

        let user = directory
            .authenticator
            .authenticate(request.email.trim(), &request.password)
            .await
            .map_err(|e| LoginError::DirectoryUnavailable(e.0))?;
        let Some(user) = user else {
            if record {
                if let Some(account) = &known {
                    self.register_account_failure(account, now).await?;
                }
                self.record_ip_failure(ip, ip_state, now).await?;
            }
            return Err(LoginError::InvalidCredentials);
        };

        let is_admin = user.groups.iter().any(|group| {
            directory
                .admin_groups
                .iter()
                .any(|admin| admin.eq_ignore_ascii_case(group))
        });
        if !is_admin
            && !directory
                .accounts
                .grants_access(&user.groups)
                .await
                .map_err(|e| internal(&e))?
        {
            // Right password, but nothing here is theirs. Same answer as a
            // wrong password: the response must not confirm the account.
            tracing::info!(user = %user.username, "directory user has no mapped group; refused");
            return Err(LoginError::InvalidCredentials);
        }

        let role = if is_admin {
            Role::admin()
        } else {
            Role::parse(DIRECTORY_USER_ROLE).map_err(|e| internal(&e))?
        };
        let account: AdminAccount = match directory.accounts.upsert(&user, &role).await {
            Ok(account) => account,
            Err(domain::RepositoryError::EmailTaken) => {
                // A local account owns this email; the directory may not take
                // it over. Its owner signs in with the local password.
                tracing::warn!(email = %user.email, "directory login collides with a local account");
                return Err(LoginError::InvalidCredentials);
            }
            Err(e) => return Err(internal(&e)),
        };
        if self.policy.is_locked(&account.lockout, now) {
            if record {
                self.record_ip_failure(ip, ip_state, now).await?;
            }
            return Err(self.too_many_attempts(&account.lockout, now));
        }
        directory
            .accounts
            .sync_groups(&account.id, &user.groups)
            .await
            .map_err(|e| internal(&e))?;
        self.reset_account(&account.id, now).await?;
        self.reset_ip(ip, now).await?;
        Ok(AuthenticatedAdmin {
            id: account.id,
            role,
        })
    }
}
