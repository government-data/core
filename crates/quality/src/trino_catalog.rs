//! A source on Trino as its catalog (decision 0048, point 1): the
//! statements the API runs when the source is saved and deleted. Pure: the
//! values are the source's, checked here so none can add a property to the
//! JDBC URL, and quoted as SQL literals.

use crate::source::{driver, Engine, Source};

/// Why a source cannot be made a catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// Its driver is not read by Trino, or Trino has no catalog for it yet.
    NotOnTrino,
    /// A field missing, or with a character the JDBC URL would read as
    /// syntax (`;`, `/`, `@`, `?`, `=`, spaces).
    Field(String),
}

/// The catalog's name in Trino: `src_<source>`.
pub fn catalog_name(source: &str) -> String {
    format!("src_{source}")
}

/// A value as a SQL string literal.
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A field that goes into the JDBC URL as it is.
fn url_part<'a>(source: &'a Source, key: &str) -> Result<&'a str, CatalogError> {
    let value = source
        .fields
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| CatalogError::Field(key.into()))?;
    let unsafe_char = |c: char| c.is_whitespace() || ";/@?=&".contains(c);
    if value.is_empty() || value.chars().any(unsafe_char) {
        return Err(CatalogError::Field(key.into()));
    }
    Ok(value)
}

/// The JDBC URL of a source on Trino, with its driver's options (for SQL
/// Server, `encrypt=…;trustServerCertificate=…`, as its driver reads them).
fn url(source: &Source, connector: &str) -> Result<String, CatalogError> {
    let host = url_part(source, "host")?;
    let port = source
        .port()
        .ok_or_else(|| CatalogError::Field("port".into()))?;
    match connector {
        "sqlserver" => {
            let database = url_part(source, "database")?;
            let options = source
                .fields
                .get("options")
                .map_or(String::new(), |o| format!(";{}", o.trim_start_matches(';')));
            Ok(format!(
                "jdbc:sqlserver://{host}:{port};databaseName={database}{options}"
            ))
        }
        "oracle" => {
            let service = url_part(source, "service")?;
            Ok(format!("jdbc:oracle:thin:@//{host}:{port}/{service}"))
        }
        _ => Err(CatalogError::NotOnTrino),
    }
}

/// `CREATE CATALOG src_<name> USING <connector> WITH (…)`, with the
/// source's current credentials.
pub fn create_catalog(source: &Source) -> Result<String, CatalogError> {
    let Some(Engine::Trino { connector }) = driver(&source.driver).map(|d| d.engine) else {
        return Err(CatalogError::NotOnTrino);
    };
    let url = url(source, connector)?;
    let secret = |key: &str| {
        source
            .secrets
            .get(key)
            .ok_or_else(|| CatalogError::Field(key.into()))
    };
    Ok(format!(
        "CREATE CATALOG {} USING {connector} WITH (\"connection-url\" = {}, \"connection-user\" = {}, \"connection-password\" = {})",
        catalog_name(&source.name),
        literal(&url),
        literal(secret("user")?),
        literal(secret("password")?),
    ))
}

/// `DROP CATALOG IF EXISTS src_<name>`.
pub fn drop_catalog(source: &str) -> String {
    format!("DROP CATALOG IF EXISTS {}", catalog_name(source))
}
