//! The identity model and its ports: value objects plus the traits the
//! `infrastructure` crate implements (argon2, Postgres, LDAP, OIDC). Pure, as
//! the `domain` crate is — no web framework, no database driver.
//!
//! The shared kernel ([`AdminId`], [`Email`], [`Role`], [`RepositoryError`],
//! [`Clock`]) stays in `domain`, since audit and orchestration name it too;
//! it is re-exported here so this layer reads as one model.

pub mod auth;
pub mod directory;
pub mod lake_admin;
pub mod lake_pool;
pub mod lake_replication;
pub mod machine;
pub mod machine_alerts;
pub mod machine_certificate;
pub mod machine_install;
pub mod machine_series;
pub mod machine_sessions;
pub mod oauth;
pub mod password_reset;
pub mod provider;
pub mod service_tokens;
pub mod session;

pub use ::domain::{AdminId, Clock, Email, EmailError, RepositoryError, Role, RoleError};
pub use auth::{
    AccountStatus, AccountSummary, AdminAccount, AdminRepository, IpLockoutStore, LockoutPolicy,
    LockoutState, NewAdmin, PasswordHash, PasswordHashError, PasswordHasher, PasswordPolicy,
    PasswordPolicyError, PasswordRequirement, ProfilePicture,
};
pub use directory::{DirectoryAccounts, DirectoryAuthenticator, DirectoryError, DirectoryUser};
pub use lake_admin::{LakeAdmin, LakeAdminKeyStore, LakeAdminKeys, LakeAdminKeysKept, LakeProbe};
pub use lake_pool::{
    LakePool, LakePoolStore, PoolLayout, PoolRefusal, MAX_POOL_DRIVES, MAX_POOL_NODES,
    MIN_POOL_NODES, REPLICATION_ALIAS, RUSTFS_PEER_PORT,
};
pub use lake_replication::{
    LakeReplication, LakeWarehouses, ReplicationOutcome, ReplicationStateStore, ReplicationStep,
    WarehouseOutcome, WarehouseReplication, WarehouseState, Warehouses, REPLICATION_PROXY_PORT,
    REPLICA_RETENTION_DAYS, SOURCE_NONCURRENT_DAYS,
};
pub use machine::{
    is_join_token, CertificateSigner, CertificateTerm, JoinTokenMint, Machine, MachineChange,
    MachineId, MachineRegistry, MachineRole, MachineState, NewJoinToken, RedeemRefusal,
    SignedCertificate, SigningRefusal, JOIN_TOKEN_LEN, JOIN_TOKEN_PREFIX,
};
pub use machine_alerts::{
    AlertChange, AlertKey, AlertSeverity, FiringAlert, MachineAlertStore, ALERTING_HEARTBEAT,
};
pub use machine_certificate::{CertificateReader, MachineCertificate, MACHINE_ORGANIZATION};
pub use machine_install::{
    InstallFile, InstallFiles, JoinGrant, JoinTokenCheck, Lake, LakeRootKeyStore, LakeRootKeys,
    MAIN_POOL,
};
pub use machine_series::{
    Line, MachineCharts, MachineSeries, SeriesRange, SeriesUnavailable, SeriesWindow,
};
pub use machine_sessions::MachineSessions;
pub use oauth::{
    AuthorizeParams, ExchangeRequest, LinkedIdentity, OAuthError, OAuthIdentity,
    OAuthIdentityRepository, OAuthProvider, OAuthSecretGenerator, PendingAuthStore,
    PendingAuthorization, PkcePair, ProviderId,
};
pub use password_reset::{LoginHandoffStore, PasswordResetStore, ResetTokenGenerator};
pub use provider::{
    AccountPolicies, AuthorizationCode, AuthorizationCodeStore, ClientAuth, ClientId,
    ClientIdError, ClientSettings, ListedApp, OidcClient, OidcClientRegistry, SigningAlg,
    TokenSigner,
};
pub use service_tokens::{ServicePrincipal, ServiceTokens};
pub use session::{
    CsrfToken, Session, SessionPolicy, SessionRepository, SessionToken, SessionTokenGenerator,
};
