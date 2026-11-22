//! The OpenAPI document for the JSON API (bead authapp-a05b92).
//!
//! Derived from the handlers' `#[utoipa::path]` annotations and the
//! request/response types' `#[derive(ToSchema)]` — there is no hand-maintained
//! duplicate of the contract. The `server openapi` subcommand serializes
//! [`ApiDoc`] to JSON, and the monorepo generates the TypeScript client from
//! that spec, so front and back share one source of truth.
//!
//! The spec covers the JSON API a typed client consumes (auth, audit, health,
//! machines). The OAuth endpoints are browser redirects, not a typed-client
//! surface, and are left out. The feature modules' routes for people are not the API's:
//! each module describes its own (`contracts::Module::openapi`) and `server`
//! merges them into [`ApiDoc::document`] before writing it.

use utoipa::OpenApi;

/// The API's OpenAPI 3.1 document.
#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "knime-orchestador API",
        description = "The JSON API the admin frontend consumes.",
        version = "0.1.0",
    ),
    paths(
        crate::liveness_handler,
        crate::readiness_handler,
        crate::auth::login_handler,
        crate::session::me_handler,
        crate::session::locale_handler,
        crate::captcha::captcha_handler,
        crate::session::logout_handler,
        crate::oauth::providers_handler,
        crate::users::list_handler,
        crate::users::create_handler,
        crate::users::resend_handler,
        crate::users::update_handler,
        crate::users::delete_handler,
        crate::oauth::identities_handler,
        crate::oauth::unlink_handler,
        crate::password_reset::request_handler,
        crate::password_reset::check_handler,
        crate::password_reset::confirm_handler,
        crate::audit::list_events,
        crate::machines::list_machines_handler,
        crate::machines::metrics::metrics_handler,
        crate::machines::issue_token_handler,
        crate::machines::lifecycle::approve_handler,
        crate::machines::lifecycle::drain_handler,
        crate::machines::lifecycle::retire_handler,
        crate::machines::lake_admin::replica_admin_keys_handler,
        crate::machines::lake_admin::set_replica_admin_keys_handler,
        crate::machines::lake_replica::replica_state_handler,
        crate::machines::lake_admin::main_admin_keys_handler,
        crate::machines::lake_admin::set_main_admin_keys_handler,
        crate::machines::lake_main::main_state_handler,
        crate::machines::lake_pools::pools_handler,
        crate::machines::install::role_pool_env_handler,
        crate::machines::lake_pools::define_pool_handler,
        crate::machines::lake_replication::replication_handler,
        crate::machines::join::join_handler,
        crate::machines::install::join_script_handler,
        crate::machines::install::install_ca_handler,
        crate::machines::install::role_compose_handler,
        crate::machines::install::role_file_handler,
        crate::machines::install::role_root_keys_handler,
        crate::machines::renew::renew_handler,
        crate::machines::dolt::dolt_hold_handler,
        crate::machines::dolt::dolt_release_handler,
        crate::machines::lake::lake_check_handler,
        crate::machines::alerts::alerts_handler,
    ),
    components(schemas(
        crate::error::ErrorResponse,
        crate::auth::LoginBody,
        crate::auth::LoginOk,
        crate::session::MeOut,
        crate::session::LocaleIn,
        crate::captcha::CaptchaOut,
        crate::oauth::ProvidersOut,
        crate::users::UsersOut,
        crate::users::UserOut,
        crate::users::CreateUserIn,
        crate::users::CreatedUserOut,
        crate::users::UpdateUserIn,
        crate::oauth::IdentitiesOut,
        crate::oauth::IdentityOut,
        crate::password_reset::ResetRequestBody,
        crate::password_reset::ResetRequestOut,
        crate::password_reset::ResetCheckBody,
        crate::password_reset::ResetCheckOut,
        crate::password_reset::PasswordPolicyOut,
        crate::password_reset::ResetConfirmBody,
        crate::audit::EventOut,
        crate::machines::MachineOut,
        crate::machines::MachinesOut,
        crate::machines::metrics::MachineMetricsOut,
        crate::machines::metrics::MachineSeriesOut,
        crate::machines::metrics::LabelledLineOut,
        crate::machines::JoinTokenIn,
        crate::machines::JoinTokenOut,
        crate::machines::lake_admin::LakeAdminKeysOut,
        crate::machines::lake_admin::LakeAdminKeysIn,
        crate::machines::lake_replica::ReplicaStateOut,
        crate::machines::lake_main::MainLakeStateOut,
        crate::machines::lake_pools::LakePoolsOut,
        crate::machines::lake_pools::LakePoolOut,
        crate::machines::lake_pools::PoolNodeOut,
        crate::machines::lake_pools::LakePoolIn,
        crate::machines::lake_replication::ReplicationOut,
        crate::machines::lake_replication::WarehouseReplicationOut,
        crate::machines::install::JoinCommandOut,
        crate::machines::join::JoinIn,
        crate::machines::join::JoinOut,
        crate::machines::renew::RenewIn,
        crate::machines::renew::RenewOut,
        crate::machines::dolt::DoltHoldOut,
        crate::machines::dolt::DoltReleaseOut,
        crate::machines::alerts::GrafanaDeliveryIn,
        crate::machines::alerts::GrafanaAlertIn,
        crate::machines::alerts::AlertsOut,
    )),
    tags(
        (name = "auth", description = "Sign-in, session, and identity"),
        (name = "audit", description = "Authentication audit trail"),
        (name = "health", description = "Readiness probe"),
        (name = "machines", description = "The installation's machines and how they join"),
    ),
)]
pub struct ApiDoc;

impl ApiDoc {
    /// The document, for the compositor to merge the modules' routes into
    /// (`contracts::ModuleRegistry::openapi`).
    pub fn document() -> utoipa::openapi::OpenApi {
        <Self as OpenApi>::openapi()
    }

    /// The spec as pretty-printed JSON — what `server openapi` writes and the
    /// client generator reads.
    pub fn to_pretty_json() -> String {
        <Self as OpenApi>::openapi()
            .to_pretty_json()
            .expect("serializing the OpenAPI document cannot fail")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_lists_every_documented_path() {
        let json = ApiDoc::to_pretty_json();
        for path in [
            "/health",
            "/ready",
            "/auth/login",
            "/auth/me",
            "/auth/logout",
            "/auth/oauth/providers",
            "/auth/me/identities",
            "/users",
            "/users/{id}",
            "/users/{id}/activation",
            "/auth/me/identities/{provider}/{subject}",
            "/audit/events",
            "/machines",
            "/machines/join-tokens",
            "/machines/{id}/approve",
            "/machines/{id}/drain",
            "/machines/{id}/retire",
            "/install/join",
            "/install/join.sh",
            "/install/ca.crt",
            "/install/{role}/compose.yml",
            "/internal/machines/renew",
        ] {
            assert!(json.contains(path), "spec is missing {path}");
        }
        // A schema the client's types come from.
        assert!(json.contains("LoginOk"));
        assert!(json.contains("MachinesOut"));
    }

    /// The client generator keys each operation by its id, so two operations
    /// with one id give the client a duplicate type and one wrong response.
    #[test]
    fn every_operation_has_its_own_id() {
        let spec: serde_json::Value =
            serde_json::from_str(&ApiDoc::to_pretty_json()).expect("the spec is JSON");
        let mut seen = std::collections::BTreeMap::new();
        for (path, item) in spec["paths"].as_object().expect("the spec has paths") {
            for (method, operation) in item.as_object().expect("a path item is an object") {
                let Some(id) = operation["operationId"].as_str() else {
                    continue;
                };
                let here = format!("{method} {path}");
                if let Some(first) = seen.insert(id.to_owned(), here.clone()) {
                    panic!("operationId {id} is used by {first} and {here}");
                }
            }
        }
        assert!(seen.contains_key("list_machines_handler"));
    }
}
