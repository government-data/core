//! A source on Trino keeps its catalog (decision 0048, point 1): saved, its
//! catalog is dropped and created again with its current credentials;
//! deleted, its catalog is dropped first, so no catalog outlives its
//! source. Trino is reached through [`Trino`], which the server implements
//! as the API's own machine client.

use async_trait::async_trait;

use crate::source::{driver, Engine, Source};
use crate::trino_catalog::{self, CatalogError};

/// Trino's coordinator, as the API: runs one statement.
#[async_trait]
pub trait Trino: Send + Sync {
    async fn run(&self, sql: &str) -> Result<(), String>;
}

/// Whether the driver's sources have a catalog on Trino (`openedge` has
/// no connector yet: none).
pub fn has_catalog(driver_id: &str) -> bool {
    matches!(
        driver(driver_id).map(|d| d.engine),
        Some(Engine::Trino {
            connector: "sqlserver" | "oracle"
        })
    )
}

/// The field a source's catalog could not be made with, before it is saved:
/// its fields are replaced whole, so they are known; its secrets may be
/// left out (kept), so they are not checked here.
pub(crate) fn refused_field(source: &Source) -> Option<String> {
    if !has_catalog(&source.driver) {
        return None;
    }
    let mut known = source.clone();
    for key in ["user", "password"] {
        known.secrets.entry(key.into()).or_default();
    }
    match trino_catalog::create_catalog(&known) {
        Err(CatalogError::Field(key)) => Some(key),
        _ => None,
    }
}

/// Drops the source's catalog and creates it again with `source` as saved.
pub(crate) async fn replace(trino: &dyn Trino, source: &Source) -> Result<(), String> {
    let create = trino_catalog::create_catalog(source).map_err(|e| format!("{e:?}"))?;
    trino
        .run(&trino_catalog::drop_catalog(&source.name))
        .await?;
    trino.run(&create).await
}

/// Drops the catalog of the source `name`, if it has one.
pub(crate) async fn drop(trino: &dyn Trino, name: &str) -> Result<(), String> {
    trino.run(&trino_catalog::drop_catalog(name)).await
}
