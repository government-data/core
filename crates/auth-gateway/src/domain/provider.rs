//! The gateway as an OAuth2 / OIDC provider: the applications that sign their
//! users in through it ([`OidcClient`]), the one-time codes of the
//! authorization-code flow ([`AuthorizationCode`]) and the keys that sign the
//! tokens ([`TokenSigner`]).
//!
//! The reverse of [`crate::domain::oauth`], where the gateway is the client of
//! someone else's provider. Pure, like the rest of the domain: the ports are
//! implemented by `infrastructure` (Postgres, Ed25519).

use std::time::SystemTime;

use crate::domain::{AdminAccount, AdminId, MachineRole, RepositoryError};

/// An application's `client_id`: lowercase letters, digits and `-`, so it is
/// safe in URLs and logs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientId(String);

impl ClientId {
    /// Parse and normalize (trimmed, lowercased) a client id.
    pub fn parse(raw: &str) -> Result<Self, ClientIdError> {
        let id = raw.trim().to_ascii_lowercase();
        if id.is_empty() || id.len() > 64 {
            return Err(ClientIdError);
        }
        if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(ClientIdError);
        }
        Ok(Self(id))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `client_id` that is empty, too long, or has characters other than
/// letters, digits and `-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdError;

impl std::fmt::Display for ClientIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a client id is 1 to 64 letters, digits or '-'")
    }
}

impl std::error::Error for ClientIdError {}

/// How an application proves who it is at the token endpoint.
#[derive(Clone, PartialEq, Eq)]
pub enum ClientAuth {
    /// A browser or mobile app that cannot keep a secret: PKCE alone binds
    /// the code to the app that asked for it.
    Public,
    /// A server-side app with a secret; only its hash is stored.
    Secret { hash: String },
    /// The machines of one role (decision 0066, point 6; RFC 8705
    /// `tls_client_auth`): no secret, but the certificate of an approved
    /// machine of `role`, as Traefik's internal entry point passes it. Any
    /// such machine gets the client's tokens, and only by
    /// `client_credentials`.
    Certificate { role: MachineRole },
}

// The hash is not the secret, but it has no business in logs either.
impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => f.write_str("Public"),
            Self::Secret { .. } => f.write_str("Secret(<redacted>)"),
            Self::Certificate { role } => write!(f, "Certificate({})", role.as_str()),
        }
    }
}

/// An application registered to sign its users in through the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcClient {
    pub id: ClientId,
    /// Shown to the person signing in.
    pub name: String,
    /// The exact URIs the code may be sent back to (no wildcards: a loose
    /// match is how codes get stolen).
    pub redirect_uris: Vec<String>,
    pub auth: ClientAuth,
    pub settings: ClientSettings,
}

impl OidcClient {
    /// Whether `uri` is one of the registered redirect URIs, compared exactly.
    pub fn allows_redirect(&self, uri: &str) -> bool {
        self.redirect_uris.iter().any(|r| r == uri)
    }

    /// Whether `/authorize` may come without a PKCE challenge: only for a
    /// confidential client flagged for it, whose code is then bound to its
    /// secret at `/token`. A public client always needs PKCE.
    pub fn pkce_optional(&self) -> bool {
        !self.settings.pkce_required && matches!(self.auth, ClientAuth::Secret { .. })
    }

    /// Whether it may use the `client_credentials` grant: flagged for it,
    /// with a secret or a machine role.
    pub fn machine(&self) -> bool {
        self.settings.client_credentials
            && matches!(
                self.auth,
                ClientAuth::Secret { .. } | ClientAuth::Certificate { .. }
            )
    }

    /// The role of the machines whose certificate authenticates it, if it
    /// is such a client.
    pub fn machine_role(&self) -> Option<MachineRole> {
        match self.auth {
            ClientAuth::Certificate { role } => Some(role),
            _ => None,
        }
    }
}

/// The JWS algorithm an application's tokens are signed with: `EdDSA`
/// (Ed25519, the default) or `ES256` (P-256), for relying parties that only
/// verify RSA or EC keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SigningAlg {
    #[default]
    EdDSA,
    ES256,
}

impl SigningAlg {
    /// The `alg` of the JWS header.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EdDSA => "EdDSA",
            Self::ES256 => "ES256",
        }
    }

    /// The algorithm named `alg`, if it is one the gateway signs with.
    pub fn parse(alg: &str) -> Option<Self> {
        match alg {
            "EdDSA" => Some(Self::EdDSA),
            "ES256" => Some(Self::ES256),
            _ => None,
        }
    }
}

/// How the provider treats one application, beyond who it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSettings {
    /// The algorithm of its id_token and access tokens.
    pub token_alg: SigningAlg,
    /// Whether `/authorize` needs PKCE. Only a confidential client may turn
    /// it off (see [`OidcClient::pkce_optional`]).
    pub pkce_required: bool,
    /// The IAM policy an account needs to get a code for it (`None`: any).
    pub required_policy: Option<String>,
    /// Whether it may use the `client_credentials` grant (RFC 6749 §4.4).
    pub client_credentials: bool,
    /// The audiences its machine tokens may be for; the first is the default.
    pub audiences: Vec<String>,
    /// The IAM service account it acts as with `client_credentials`: the
    /// token's subject, and its policies go in the `policy` claim.
    pub service_account: Option<String>,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            token_alg: SigningAlg::default(),
            pkce_required: true,
            required_policy: None,
            client_credentials: false,
            audiences: Vec::new(),
            service_account: None,
        }
    }
}

impl ClientSettings {
    /// Whether these settings make sense for a client that authenticates
    /// with `auth`: turning PKCE off needs a secret; machine tokens need a
    /// secret or a machine role, and an audience; a machine role's client
    /// has machine tokens and nothing else.
    pub fn check(&self, auth: &ClientAuth) -> Result<(), &'static str> {
        let secret = matches!(auth, ClientAuth::Secret { .. });
        let certificate = matches!(auth, ClientAuth::Certificate { .. });
        if !self.pkce_required && !secret {
            return Err("only a client with a secret may turn PKCE off");
        }
        if self.client_credentials && !(secret || certificate) {
            return Err("client_credentials needs a secret or a machine role");
        }
        if matches!(auth, ClientAuth::Certificate { role } if !role.joins()) {
            return Err("the application VM's role gets no machine client");
        }
        if certificate && !self.client_credentials {
            return Err("a machine role's client only uses client_credentials");
        }
        if self.client_credentials && self.audiences.is_empty() {
            return Err("client_credentials needs at least one audience");
        }
        Ok(())
    }
}

/// A one-time code issued at `/authorize`, redeemed at `/token`. Stored by
/// the hash of the code, never the code itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationCode {
    pub client_id: ClientId,
    pub admin_id: AdminId,
    /// The redirect URI the code was sent to; `/token` must repeat it.
    pub redirect_uri: String,
    /// The PKCE `S256` challenge; `/token` must bring its verifier. `None`
    /// only for a client that may skip PKCE and did.
    pub code_challenge: Option<String>,
    /// Echoed in the id_token, if the application sent one.
    pub nonce: Option<String>,
    /// The scopes granted (always including `openid`).
    pub scopes: Vec<String>,
    /// When the person signed in (the session's start), for `auth_time`.
    pub auth_time: SystemTime,
    pub expires_at: SystemTime,
}

/// Port: the registered applications.
#[async_trait::async_trait]
pub trait OidcClientRegistry: Send + Sync {
    async fn find(&self, id: &ClientId) -> Result<Option<OidcClient>, RepositoryError>;

    /// Register an application; `Ok(false)` if the id is taken.
    async fn insert(&self, client: &OidcClient) -> Result<bool, RepositoryError>;

    /// Register it or, if the id exists, replace its name, redirect URIs and
    /// how it authenticates (an installation management registers for itself).
    async fn upsert(&self, client: &OidcClient) -> Result<(), RepositoryError>;

    /// The applications registered with a start address, to list them in the
    /// avatar's «Aplicaciones».
    async fn listed(&self) -> Result<Vec<ListedApp>, RepositoryError> {
        Ok(Vec::new())
    }
}

/// An application that signs in with this installation, as «Aplicaciones»
/// lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedApp {
    pub id: String,
    pub name: String,
    pub detail: String,
    /// Where it starts (https).
    pub url: String,
    /// Its icon (https), if it has one.
    pub icon: Option<String>,
}

/// Port: the names of an account's IAM policies (ADR 0016), for the `policy`
/// claim: the applications that sign in with this installation (RustFS)
/// apply their policies of the same names.
#[async_trait::async_trait]
pub trait AccountPolicies: Send + Sync {
    async fn policies(&self, admin: &AdminAccount) -> Result<Vec<String>, RepositoryError>;

    /// The policy names of an IAM service account (by id), for the tokens of
    /// a machine client that acts as it. None by default.
    async fn service_account_policies(
        &self,
        _service_account: &str,
    ) -> Result<Vec<String>, RepositoryError> {
        Ok(Vec::new())
    }
}

/// Port: the outstanding authorization codes.
#[async_trait::async_trait]
pub trait AuthorizationCodeStore: Send + Sync {
    async fn insert(
        &self,
        code_hash: &str,
        code: &AuthorizationCode,
    ) -> Result<(), RepositoryError>;

    /// Fetch and delete in one step, so a code can be redeemed only once.
    async fn take(&self, code_hash: &str) -> Result<Option<AuthorizationCode>, RepositoryError>;
}

/// Port: the key that signs the tokens (JWS compact serialization).
///
/// Takes and returns JSON as text so the domain needs no JSON library; the
/// application layer builds the claims.
pub trait TokenSigner: Send + Sync {
    /// Sign `claims` (a JSON object) with `typ` in the header (`JWT` for an
    /// id_token, `at+jwt` for an access token, RFC 9068).
    fn sign(&self, typ: &str, claims: &str) -> String;

    /// The claims of a token this signer issued, if the signature and the
    /// `typ` check out. Expiry and audience are the caller's to check.
    fn verify(&self, typ: &str, token: &str) -> Option<String>;

    /// The `alg` the tokens are signed with (a [`SigningAlg`] name), for the
    /// discovery document and to pick the signer of each application.
    fn algorithm(&self) -> &'static str;

    /// The public keys, as a JWK Set (JSON), for `/jwks`.
    fn jwks(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_id_is_normalized_and_restricted() {
        assert_eq!(ClientId::parse(" Gestion ").unwrap().as_str(), "gestion");
        assert_eq!(
            ClientId::parse("facturacion-2").unwrap().as_str(),
            "facturacion-2"
        );
        assert!(ClientId::parse("").is_err());
        assert!(ClientId::parse("a b").is_err());
        assert!(ClientId::parse("a/b").is_err());
        assert!(ClientId::parse(&"a".repeat(65)).is_err());
    }

    #[test]
    fn redirect_uris_match_exactly() {
        let client = OidcClient {
            id: ClientId::parse("gestion").unwrap(),
            name: "Gestión".into(),
            redirect_uris: vec!["https://gestion.example.com/callback".into()],
            auth: ClientAuth::Public,
            settings: ClientSettings::default(),
        };
        assert!(client.allows_redirect("https://gestion.example.com/callback"));
        assert!(!client.allows_redirect("https://gestion.example.com/callback/"));
        assert!(!client.allows_redirect("https://gestion.example.com/callback?x=1"));
        assert!(!client.allows_redirect("https://evil.example.com/callback"));
    }

    #[test]
    fn a_machine_role_client_is_for_client_credentials_only() {
        let worker = ClientAuth::Certificate {
            role: MachineRole::Worker,
        };
        let machine = ClientSettings {
            client_credentials: true,
            audiences: vec!["quality-worker".into()],
            ..ClientSettings::default()
        };
        assert_eq!(machine.check(&worker), Ok(()));
        assert!(ClientSettings::default().check(&worker).is_err());
        let no_pkce = ClientSettings {
            pkce_required: false,
            ..machine.clone()
        };
        assert!(no_pkce.check(&worker).is_err());
        let no_audience = ClientSettings {
            audiences: Vec::new(),
            ..machine.clone()
        };
        assert!(no_audience.check(&worker).is_err());
        assert!(machine.check(&ClientAuth::Public).is_err());
        let app = ClientAuth::Certificate {
            role: MachineRole::App,
        };
        assert!(machine.check(&app).is_err(), "the application VM gets none");

        let client = OidcClient {
            id: ClientId::parse("qd-worker").unwrap(),
            name: "Workers".into(),
            redirect_uris: Vec::new(),
            auth: worker.clone(),
            settings: machine,
        };
        assert!(client.machine());
        assert!(!client.pkce_optional());
        assert_eq!(client.machine_role(), Some(MachineRole::Worker));
        assert_eq!(format!("{worker:?}"), "Certificate(worker)");
    }
}
