//! Who may do an action on which resources, for a module that hands every
//! person's access to another system at once (decision 0089, step 5: the
//! quality module's neutral grants for the rules merge of `engine-trino`),
//! where [`crate::Caller`] answers only for the caller of one request.
//! The composition root answers it from the platform's IAM.

use async_trait::async_trait;

/// A person and the resources they may do the action on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grantee {
    /// The account id, as the `sub` of the person's tokens.
    pub account: String,
    /// Those of the resources asked the person may do it on, in order.
    pub resources: Vec<String>,
}

/// The port.
#[async_trait]
pub trait Grantees: Send + Sync {
    /// Every active person (activated, not only invited) who may do
    /// `action` on at least one of `resources` (`pipeline/<id>`), with
    /// those they may: deny unless allowed, an explicit deny wins, as for
    /// a request.
    async fn allowed(&self, action: &str, resources: &[String]) -> Result<Vec<Grantee>, String>;
}
