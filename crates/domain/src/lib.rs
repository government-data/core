//! Domain layer — the core of the hexagonal architecture.
//!
//! This crate holds the business model and the *ports* (traits) the outer
//! layers implement or consume. It depends on nothing but the standard
//! library (and `sha2`, a pure hash, for the audit chain): no web framework,
//! no database driver. That purity is what the
//! dependency direction protects — `api → application → domain` and
//! `infrastructure → domain` both point inward, so the domain never reaches
//! outward to a framework.
//!
//! Identity — passwords, sessions, OAuth, the directory — is its own crate,
//! `auth-gateway`; here stays only the shared kernel it builds on.

pub mod audit;
pub mod audit_chain;
pub mod audit_chain_check;
pub mod audit_view;
pub mod build;
pub mod health;
pub mod kernel;
pub mod slot;

pub use audit::{
    AuditEvent, AuditEventType, AuditEventTypeError, AuditId, AuditRepository, NewActionAudit,
    NewAuditEvent,
};
pub use audit_view::{NewViewAudit, ViewShown};
pub use build::{COMMIT, VERSION};
pub use health::{Health, HealthCheck, Readiness};
pub use kernel::{
    is_valid_email, AdminId, Clock, Email, EmailError, RepositoryError, Role, RoleError,
};
