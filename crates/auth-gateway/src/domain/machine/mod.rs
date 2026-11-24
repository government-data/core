//! The installation's machines and the tokens they join with (decisions
//! 0029 and 0066): the model and its ports. The use cases are in
//! `application::machines`; the ports live here, with identity, because
//! `infrastructure` implements them and the token endpoint will ask them for
//! a machine's certificate (0066, point 6).
//!
//! A machine is `pending` when it joins, `approved` by a person, `draining`
//! when it is being taken out, and `retired` for good. Nothing leaves
//! `retired`.

use std::time::SystemTime;

use crate::domain::{AdminId, RepositoryError};

/// What every join token starts with, so a leaked one is recognisable.
pub const JOIN_TOKEN_PREFIX: &str = "qdj_";

/// The length of a join token: the prefix and 32 bytes in base64url
/// without padding (43 characters).
pub const JOIN_TOKEN_LEN: usize = JOIN_TOKEN_PREFIX.len() + 43;

/// Whether `token` has a join token's form. A token without it is refused
/// without looking it up, with the same refusal as an unknown one.
pub fn is_join_token(token: &str) -> bool {
    token.len() == JOIN_TOKEN_LEN
        && token.starts_with(JOIN_TOKEN_PREFIX)
        && token[JOIN_TOKEN_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A machine's name, a pool's name: lowercase letters, digits and hyphens,
/// 1 to 63, as the CA's `MachineIdentity` takes them.
pub fn is_label(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What a machine does; its certificate's `OU`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MachineRole {
    Worker,
    TrinoCoordinator,
    TrinoWorker,
    Lake,
    LakeReplica,
    /// The application VM (0066, point 15): its agent keeps the certificate
    /// PostgreSQL, Traefik's internal entry point and Dolt serve. It gets no
    /// jobs and no OIDC client, and does not join: its first certificate is
    /// signed on the spot by the installer (point 16).
    App,
    /// The installation's update agent (decision 0003): it calls
    /// `/updater/v1/` on the internal entry point with this certificate.
    /// Like `app`, it does not join and gets no OIDC client: the installer
    /// signs its certificate on the spot (`server updater-certificate`).
    Updater,
}

impl MachineRole {
    pub const ALL: [Self; 7] = [
        Self::Worker,
        Self::TrinoCoordinator,
        Self::TrinoWorker,
        Self::Lake,
        Self::LakeReplica,
        Self::App,
        Self::Updater,
    ];

    /// The stored form, also the certificate's `OU`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::TrinoCoordinator => "trino-coordinator",
            Self::TrinoWorker => "trino-worker",
            Self::Lake => "lake",
            Self::LakeReplica => "lake-replica",
            Self::App => "app",
            Self::Updater => "updater",
        }
    }

    /// Whether a machine of this role joins with a token (0029). The
    /// application VM does not (0066, point 16), and gets no OIDC client
    /// (point 15): a join token or a machine client for it is refused. Nor
    /// does the update agent (decision 0003).
    pub fn joins(self) -> bool {
        !matches!(self, Self::App | Self::Updater)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == raw)
    }

    /// How long this role's certificate lasts, decided by the role and
    /// never by what the machine asks (0066, points 1, 9 and 11): Trino's
    /// coordinator, and Dolt on the application VM, read their certificate
    /// only when they start and restart only when free, so theirs is ended
    /// by its replacement, not by a date; so is the update agent's, which
    /// the installer signs again before it ends; every other role renews its
    /// own every 8 hours and lasts a day.
    pub fn certificate_term(self) -> CertificateTerm {
        match self {
            Self::TrinoCoordinator | Self::App | Self::Updater => CertificateTerm::UntilReplaced,
            _ => CertificateTerm::Day,
        }
    }
}

/// How long a machine's certificate lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateTerm {
    /// 24 hours.
    Day,
    /// The one-year ceiling of 0066, point 9.
    UntilReplaced,
}

/// A certificate the CA signed for a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCertificate {
    /// The certificate, PEM.
    pub pem: String,
    /// Its serial, lowercase hex.
    pub serial: String,
    pub not_after: SystemTime,
}

/// Why the CA did not sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigningRefusal {
    /// The request is not one the CA signs (it cannot be read, its key is
    /// not EC P-256, its signature does not verify), or the name cannot be
    /// a certificate's.
    Request(String),
    /// The CA failed.
    Failed(String),
}

/// Port: the installation's CA, as the join needs it (0066, point 2). Of a
/// request only its key is kept: subject, names, uses and dates are the CA's.
pub trait CertificateSigner: Send + Sync {
    /// Whether `csr_pem` would be signed for the machine `name`, so that a
    /// bad request is refused before a join token is used.
    fn check(&self, csr_pem: &str, name: &str) -> Result<(), SigningRefusal>;

    /// Sign `csr_pem` for `O=qd, OU=<role>, CN=<name>`, with `name` as its
    /// alternative name (the application machine's also `dolt`, decision
    /// 0080's amendment), for `term`.
    fn sign(
        &self,
        csr_pem: &str,
        role: MachineRole,
        name: &str,
        term: CertificateTerm,
    ) -> Result<SignedCertificate, SigningRefusal>;

    /// The CA's own certificate, PEM.
    fn ca_pem(&self) -> String;
}

/// Where a machine is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MachineState {
    /// Joined, waiting for a person. It has a certificate, but gets no jobs.
    Pending,
    Approved,
    /// Being taken out: it finishes what it has and gets nothing new.
    Draining,
    Retired,
}

impl MachineState {
    pub const ALL: [Self; 4] = [Self::Pending, Self::Approved, Self::Draining, Self::Retired];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Draining => "draining",
            Self::Retired => "retired",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    /// The states a machine can come to `self` from: pending → approved →
    /// draining → retired, and any but retired straight to retired.
    pub fn reached_from(self) -> &'static [Self] {
        match self {
            Self::Pending => &[],
            Self::Approved => &[Self::Pending],
            Self::Draining => &[Self::Approved],
            Self::Retired => &[Self::Pending, Self::Approved, Self::Draining],
        }
    }

    /// Whether a machine in `self` may go to `to`.
    pub fn may_become(self, to: Self) -> bool {
        to.reached_from().contains(&self)
    }

    /// Whether a machine in `self` gets tokens with its certificate (0066,
    /// point 6): approved, and draining, which finishes its jobs and must
    /// report them. A pending one has a certificate but must not work yet;
    /// a retired one is out.
    pub fn gets_tokens(self) -> bool {
        matches!(self, Self::Approved | Self::Draining)
    }
}

/// A machine's id (a UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineId(String);

impl MachineId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A machine, as `machines` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub role: MachineRole,
    pub pool: Option<String>,
    pub state: MachineState,
    /// What its agent reports running.
    pub version: Option<String>,
    /// The serial of the certificate in use, lowercase hex.
    pub cert_serial: Option<String>,
    pub cert_not_after: Option<SystemTime>,
    /// A renewed certificate the machine has not presented yet (0066, point
    /// 14): accepted too, and the one in use once it is first presented.
    pub cert_serial_next: Option<String>,
    pub cert_not_after_next: Option<SystemTime>,
    pub created_at: SystemTime,
    pub approved_at: Option<SystemTime>,
    pub approved_by: Option<AdminId>,
    pub retired_at: Option<SystemTime>,
    pub retired_by: Option<AdminId>,
}

/// A join token to store: its hash, never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJoinToken {
    pub token_hash: String,
    pub role: MachineRole,
    pub pool: Option<String>,
    /// The node of a lake pool the token is for (0066 step 11, ruling 1 of
    /// 2026-11-14): only a machine of that name joins with it.
    pub node: Option<String>,
    pub created_by: AdminId,
    pub expires_at: SystemTime,
}

/// Why a join token did not make a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemRefusal {
    /// Unknown, used or expired: one answer for the three.
    Token,
    /// A machine that is not retired has the name. The token is not used.
    NameTaken,
    /// The token names a node and the name is not that node's. The token
    /// is not used.
    NodeName,
}

/// The outcome of a change of state or of certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineChange {
    Done(Box<Machine>),
    /// The machine is in a state the change is not allowed from.
    NotAllowed(MachineState),
    NotFound,
}

/// Port: the machines and their join tokens, in the current tenant.
#[async_trait::async_trait]
pub trait MachineRegistry: Send + Sync {
    /// Store a join token; returns its id.
    async fn issue_token(&self, token: &NewJoinToken) -> Result<String, RepositoryError>;

    /// Use the token whose hash is `token_hash`, if it is unused and not
    /// expired at `now`, and make with it a `pending` machine named `name`
    /// with the token's role and pool, all at once: of two at the same time
    /// exactly one succeeds. A token that names a node makes only a machine
    /// of that name. A refusal leaves everything as it was.
    async fn redeem(
        &self,
        token_hash: &str,
        name: &str,
        now: SystemTime,
    ) -> Result<Result<Machine, RedeemRefusal>, RepositoryError>;

    /// Make an `approved` machine named `name` of `role`, with no token and
    /// no person: the application VM's, made by the installer (0066, point
    /// 16). `None` when a machine that is not retired has the name.
    async fn create_approved(
        &self,
        name: &str,
        role: MachineRole,
        now: SystemTime,
    ) -> Result<Option<Machine>, RepositoryError>;

    /// Move a machine to `to` if its state is one of `to.reached_from()`,
    /// recording who and when for `approved` and `retired`.
    async fn transition(
        &self,
        id: &MachineId,
        to: MachineState,
        by: &AdminId,
        now: SystemTime,
    ) -> Result<MachineChange, RepositoryError>;

    /// Set the certificate in use of a machine that is not retired, and
    /// clear its next one.
    async fn record_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError>;

    /// Set the next certificate of a machine that is not retired, replacing
    /// any next one it had; the one in use stays.
    async fn record_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
        not_after: SystemTime,
    ) -> Result<MachineChange, RepositoryError>;

    /// The next certificate becomes the one in use, if `serial` is still the
    /// machine's next one and it is not retired; of two at once one does it.
    async fn promote_next_certificate(
        &self,
        id: &MachineId,
        serial: &str,
    ) -> Result<MachineChange, RepositoryError>;

    async fn find(&self, id: &MachineId) -> Result<Option<Machine>, RepositoryError>;

    /// The machine named `name` that is not retired.
    async fn find_live(&self, name: &str) -> Result<Option<Machine>, RepositoryError>;

    /// Every machine, retired ones too, by name.
    async fn list(&self) -> Result<Vec<Machine>, RepositoryError>;
}

/// Port: makes join tokens and the hash that is stored instead.
pub trait JoinTokenMint: Send + Sync {
    /// A fresh token: [`JOIN_TOKEN_PREFIX`] and 32 random bytes, base64url.
    /// `None` if the system gave no random bytes.
    fn mint(&self) -> Option<String>;

    /// What is stored of `token` (SHA-256, hex).
    fn hash(&self, token: &str) -> String;
}

#[cfg(test)]
mod tests;
