//! The identity routes (axum): signing in and out, the session cookies and the
//! middleware that requires them, OAuth, password resets, user management and the gateway's own OIDC
//! provider. The `api` crate mounts them
//! next to its own routes.

pub mod auth;
pub mod captcha;
pub mod client_cert;
pub mod client_cert_info;
pub mod oauth;
pub mod password_reset;
pub mod provider;
pub mod rbac;
pub mod session;
pub mod users;
pub mod verify;
