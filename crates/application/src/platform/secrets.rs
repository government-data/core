//! Global secrets: registered once in Settings, by whoever may do
//! `admin:ManageSecrets` on them (`secret/<name>`).
//! Nobody sees a value: it is stored encrypted, as the ciphertext of its JSON
//! text (AES-GCM with the Settings secrets key), and only opened when used.

use iam::catalog::MANAGE_SECRETS;
use iam::Actor;
use platform::{Connection, NewSecret, Secret, SecretId};
use serde_json::{json, Value};

use super::{require, PlatformError, PlatformService, R};

const MAX_NAME: usize = 100;
const MAX_DESCRIPTION: usize = 500;
const MAX_VALUE_BYTES: usize = 16 * 1024;

/// A letter or `_`, then letters, digits, `_`, `.` or `-`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        && name.len() <= MAX_NAME
}

impl PlatformService {
    /// The ones the actor may manage, without their values.
    pub async fn list_secrets(&self, actor: &Actor) -> R<Vec<Secret>> {
        if !actor.can_some(MANAGE_SECRETS) {
            return Err(PlatformError::Forbidden);
        }
        let scope = actor.scope(MANAGE_SECRETS, "secret");
        if scope.is_nothing() {
            return Ok(Vec::new());
        }
        Ok(self.repo.list_secrets(&scope).await?)
    }

    fn require_secret(actor: &Actor, name: &str) -> R<()> {
        if actor.can(MANAGE_SECRETS, &format!("secret/{name}")) {
            Ok(())
        } else {
            Err(PlatformError::Forbidden)
        }
    }

    /// The secret `id`, if the actor may manage it (else `NotFound`: it does
    /// not confirm that it exists).
    async fn secret_of(&self, actor: &Actor, id: &SecretId) -> R<Secret> {
        let secret = self
            .repo
            .list_secrets(&actor.scope(MANAGE_SECRETS, "secret"))
            .await?
            .into_iter()
            .find(|s| &s.id == id)
            .ok_or(PlatformError::NotFound)?;
        Ok(secret)
    }

    pub async fn create_secret(
        &self,
        actor: &Actor,
        name: &str,
        description: &str,
        value: &Value,
    ) -> R<Secret> {
        let name = name.trim().to_string();
        Self::require_secret(actor, &name)?;
        require(
            valid_name(&name),
            "name",
            &texts::tr!("app.platform.secrets.invalidName", name = name),
        )?;
        let description = description.trim().to_string();
        require(
            description.chars().count() <= MAX_DESCRIPTION,
            "description",
            &texts::tr!("app.platform.secrets.descriptionTooLong"),
        )?;
        let (sealed, kind, meta) = self.sealed_secret(value, None)?;
        Ok(self
            .repo
            .create_secret(&NewSecret {
                name,
                description,
                sealed,
                kind,
                meta,
            })
            .await?)
    }

    /// Change the description or the value.
    pub async fn update_secret(
        &self,
        actor: &Actor,
        id: &SecretId,
        description: Option<&str>,
        value: Option<&Value>,
    ) -> R<Secret> {
        self.secret_of(actor, id).await?;
        let description = description.map(str::trim);
        if let Some(d) = description {
            require(
                d.chars().count() <= MAX_DESCRIPTION,
                "description",
                &texts::tr!("app.platform.secrets.descriptionTooLong"),
            )?;
        }
        // A connection that arrives with its secret fields empty keeps the
        // previous ones: the browser never has them.
        let before = match value.and_then(Connection::from_value) {
            Some(_) => self
                .repo
                .secret_values(std::slice::from_ref(id))
                .await?
                .get(id)
                .map(|sealed| self.open_sealed(sealed))
                .transpose()?
                .as_ref()
                .and_then(Connection::from_value),
            None => None,
        };
        let sealed = value
            .map(|v| self.sealed_secret(v, before.as_ref()))
            .transpose()?;
        Ok(self
            .repo
            .update_secret(
                id,
                description,
                sealed.as_ref().map(|(s, _, m)| (s.as_str(), m)),
            )
            .await?)
    }

    /// Remove it.
    pub async fn delete_secret(&self, actor: &Actor, id: &SecretId) -> R<()> {
        self.secret_of(actor, id).await?;
        Ok(self.repo.delete_secret(id).await?)
    }

    /// The encrypted value, its kind and its public part: a connection (with
    /// `before`'s secrets where they arrive empty) or a single value.
    fn sealed_secret(
        &self,
        value: &Value,
        before: Option<&Connection>,
    ) -> R<(String, String, Value)> {
        if let Some(conn) = Connection::from_value(value) {
            let conn = match before {
                Some(b) => conn.keep_secrets_from(b),
                None => conn,
            };
            if let Err(why) = conn.validate() {
                return Err(PlatformError::Invalid(format!("value: {why}")));
            }
            let full = conn.to_value();
            require(
                full.to_string().len() <= MAX_VALUE_BYTES,
                "value",
                &texts::tr!("app.platform.secrets.valueTooLong"),
            )?;
            return Ok((
                self.seal_secret(&full)?,
                platform::CONNECTION_KIND.into(),
                conn.public(),
            ));
        }
        require(
            !value.is_object() && !value.is_array() && !value.is_null(),
            "value",
            &texts::tr!("app.platform.secrets.scalarValue"),
        )?;
        require(
            value.to_string().len() <= MAX_VALUE_BYTES,
            "value",
            &texts::tr!("app.platform.secrets.valueTooLong"),
        )?;
        Ok((self.seal_secret(value)?, "value".into(), json!({})))
    }

    /// Encrypt a secret's value: its JSON text. Without a key (only in tests
    /// and development), the JSON in clear.
    fn seal_secret(&self, value: &Value) -> R<String> {
        let Some(cipher) = &self.cipher else {
            return Ok(value.to_string());
        };
        cipher.encrypt(&value.to_string()).map_err(|e| {
            PlatformError::Internal(texts::tr!("app.platform.secrets.encryptFailed", e = e))
        })
    }

    /// Decrypt a value [`Self::seal_secret`] encrypted.
    fn open_sealed(&self, sealed: &str) -> R<Value> {
        let plain = match &self.cipher {
            Some(cipher) => cipher.decrypt(sealed).map_err(|e| {
                PlatformError::Internal(texts::tr!("app.platform.secrets.decryptFailed", e = e))
            })?,
            None => sealed.to_string(),
        };
        serde_json::from_str(&plain)
            .map_err(|e| PlatformError::Internal(format!("unreadable secret: {e}")))
    }
}
