//! ERB view ingestion — compile the template to Ruby, then feed it
//! through the generic expression ingester. The resulting `View` body
//! is a `Seq` of `_buf` operations the emitter pattern-matches back to
//! template form.

use std::path::Path;

use crate::Symbol;
use crate::dialect::View;
use crate::erb;
use crate::ty::Row;

use super::IngestResult;
use super::expr::ingest_ruby_program;

/// Ingest a single `.erb` template. The path-extension shape
/// `posts/index.html.erb` yields name=`posts/index`, format=`html`.
pub fn ingest_view(source: &str, rel_path: &Path, file: &str) -> IngestResult<View> {
    let path_str = rel_path.to_string_lossy();
    let no_erb = path_str.strip_suffix(".erb").unwrap_or(&path_str);
    let (name, format) = match no_erb.rsplit_once('.') {
        Some((stem, fmt)) => (stem.to_string(), fmt.to_string()),
        None => (no_erb.to_string(), "html".to_string()),
    };

    // Compile ERB to Ruby, then ingest the compiled Ruby through our
    // existing pipeline. The resulting View body is a `Seq` of `_buf`
    // operations the emitter pattern-matches back to template form.
    let compiled = erb::compile_erb(source);
    let body = ingest_ruby_program(&compiled, file)?;

    Ok(View {
        name: Symbol::from(name),
        format: Symbol::from(format),
        locals: Row::closed(),
        body,
    })
}
