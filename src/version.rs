//! Which build is answering. Every surface that reports a number a
//! stranger might paste somewhere — `roundhouse --version`, the MCP
//! `serverInfo`, the LSP's server info — names the build the same way
//! the /ide/ summary line does: crate version, then the commit when the
//! build knew it (`build.rs` stamps `ROUNDHOUSE_COMMIT` from the
//! environment or from `git rev-parse`; a tarball build has none).

/// The crate version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The short commit the binary was built from, when known.
pub const COMMIT: Option<&str> = option_env!("ROUNDHOUSE_COMMIT");

/// `0.1.0 (8050e0a6)`, or `0.1.0` when the build carried no commit.
pub fn describe() -> String {
    match COMMIT {
        Some(c) => format!("{VERSION} ({c})"),
        None => VERSION.to_string(),
    }
}
