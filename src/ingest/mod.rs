//! Prism → Roundhouse IR.
//!
//! Reads Ruby source (a single file or a Rails app directory) and produces an
//! [`App`](crate::App). This is the reverse of [`crate::emit::ruby`]; together
//! they form the round-trip forcing function.
//!
//! Scope for the initial landing is the tiny-blog fixture: a single model, a
//! single controller with one action, a trivial routes file, and a schema.
//! The ingester deliberately panics on unrecognized constructs — a failed
//! ingest is a signal that the IR (or the recognizer) needs to grow.
//!
//! Organized one submodule per Rails concern: [`app`] orchestrates the
//! whole-directory walk, and [`model`], [`controller`], [`routes`],
//! [`schema`], [`view`], [`test`], [`fixture`] each handle a single source
//! type. The expression-level recursive descent lives in [`expr`]; small
//! cross-cutting Prism AST helpers live in [`util`].

pub mod app;
pub mod controller;
pub mod expr;
pub mod fixture;
pub mod library_class;
pub mod model;
pub mod routes;
pub mod schema;
pub mod test;
pub mod util;
pub mod view;

pub use app::{ingest_app, ingest_app_from_tree, ingest_app_with_vfs};
pub use controller::ingest_controller;
pub use expr::ingest_expr;
pub use fixture::ingest_fixture_file;
pub use library_class::{
    classify_class_file, ingest_library_class, ingest_library_classes, ClassKind,
};
pub use model::ingest_model;
pub use routes::ingest_routes;
pub use schema::ingest_schema;
pub use test::ingest_test_file;
pub use view::ingest_view;

// Errors ----------------------------------------------------------------

#[derive(Debug)]
pub enum IngestError {
    Io(std::io::Error),
    Parse { file: String, message: String },
    Unsupported { file: String, message: String },
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Parse { file, message } => write!(f, "parse error in {file}: {message}"),
            Self::Unsupported { file, message } => {
                write!(f, "unsupported construct in {file}: {message}")
            }
        }
    }
}

impl std::error::Error for IngestError {}

impl From<std::io::Error> for IngestError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

type IngestResult<T> = Result<T, IngestError>;
