//! What build is running.

/// The product version: the release tag's (`APP_VERSION` at build time) or, in
/// development, the crate's.
pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The commit it was built from (`APP_COMMIT` at build time), to know exactly
/// what code is running. None in development.
pub const COMMIT: Option<&str> = option_env!("APP_COMMIT");
