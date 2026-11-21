//! A source's catalog name on Trino (decision 0048, point 1). No source is
//! made a catalog any more (decision 0077): only the name stays, for the
//! worker's and the jobs' Trino paths until they go too.

/// The catalog's name in Trino: `src_<source>`.
pub fn catalog_name(source: &str) -> String {
    format!("src_{source}")
}
