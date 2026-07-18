//! Jbuilder template ingestion — the source is plain Ruby (no template
//! wrapper to compile), so we feed it straight through the generic
//! expression ingester. The resulting `View` body is the raw Jbuilder
//! DSL call sequence; `jbuilder_to_library` walks it into a
//! string-accumulator method body.

use std::path::Path;

use crate::Symbol;
use crate::dialect::View;
use crate::ty::Row;

use super::IngestResult;
use super::expr::ingest_ruby_program;

/// Ingest a single `.jbuilder` template. The path-extension shape
/// `articles/_article.json.jbuilder` yields name=`articles/_article`,
/// format=`json`. Mirrors `ingest_view` for ERB; the discriminator
/// downstream is `format == "json"`.
pub fn ingest_jbuilder(source: &str, rel_path: &Path, file: &str) -> IngestResult<View> {
    let path_str = rel_path.to_string_lossy();
    let no_jbuilder = path_str.strip_suffix(".jbuilder").unwrap_or(&path_str);
    let (name, format) = match no_jbuilder.rsplit_once('.') {
        Some((stem, fmt)) => (stem.to_string(), fmt.to_string()),
        None => (no_jbuilder.to_string(), "json".to_string()),
    };

    let body = ingest_ruby_program(source, file)?;

    Ok(View {
        name: Symbol::from(name),
        format: Symbol::from(format),
        locals: Row::closed(),
        body,
        strict_locals: None,
    })
}
