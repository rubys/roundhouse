//! Inline transpile of `runtime/ruby/*.rb` → TS at app emit time.
//!
//! Replaces the old "runtime_transpile_ts bin writes to checked-in
//! `runtime/typescript/*.ts` files" workflow. The Ruby + RBS sources
//! are baked into the binary via `include_str!`; at emit time we
//! parse them and produce the TS classes inline. No drift possible —
//! the Ruby source IS the source of truth, no checked-in artifact
//! to fall out of sync.
//!
//! Hand-written runtime files (`juntos.ts`, `minitest.ts`,
//! etc.) stay as `include_str!` of `.ts` content in `src/emit/typescript.rs`.
//! This module covers ONLY the generated set.
//!
//! The mapping table here is the lift-and-shift of
//! `src/bin/runtime_transpile_ts.rs`'s PAIRS, with a single new
//! function (`typescript_units`) that returns the parsed-and-emitted
//! result. Tree-shake (a follow-up) operates on the parsed
//! `LibraryClass`es before final emit, dropping methods nothing in
//! the app references.
//!
//! This module is target-specific (TypeScript). When other targets
//! catch up to the level where they need transpiled framework Ruby,
//! they get sibling functions (`crystal_units`, `rust_units`, …)
//! that consume the same source set with target-specific imports
//! and preludes.

use crate::dialect::{LibraryClass, MethodDef};
use crate::emit::typescript::{emit_library_class, emit_module};
use crate::runtime_src::{parse_library_with_rbs, parse_methods_with_rbs};
use std::path::PathBuf;

/// Strategy: each entry picks one of two pipelines.
///
/// `Module` — flat list of `def self.*` helpers (e.g. inflector.rb).
/// Each method becomes a standalone `export function`.
///
/// `Library` — file with one or more class/module definitions
/// preserving parent/includes (e.g. errors.rb). Output is a
/// concatenation of the per-class TS class declarations.
enum Mode {
    Module,
    Library,
}

/// Cross-runtime imports each transpiled file needs. Each entry:
/// (named_import_clause, source_module). The runtime files all
/// emit under `src/<name>.ts`, so cross-references use `./<name>.js`.
type ImportSpec = &'static [(&'static str, &'static str)];
const NO_IMPORTS: ImportSpec = &[];

/// Hand-written TS prepended after imports and before the
/// transpiled class bodies. Used for module-scope constants
/// (`STATUS_CODES = {...}.freeze`) the lowerer doesn't yet
/// recognize.
const NO_PRELUDE: &str = "";

/// `(class_name, method_name)` pairs that hand-written runtime files
/// (server.ts, test_support.ts, broadcasts.ts) call into directly.
/// Tree-shake walks app-side bodies for reachability; calls that
/// only appear in hand-written runtime code aren't visible to that
/// walk and would otherwise drop. Listing them here keeps them.
type ExtraRoots = &'static [(&'static str, &'static str)];
const NO_EXTRA_ROOTS: ExtraRoots = &[];

/// One entry in the runtime transpile table. Maps a Ruby+RBS pair
/// to its emit shape (target path, mode, imports, prelude).
struct RuntimeEntry {
    rb_src: &'static str,
    rbs_src: &'static str,
    rb_path: &'static str,  // for error messages only
    /// Enclosing module name as written in the Ruby source (e.g.
    /// `"ActiveRecord"` for `module ActiveRecord; class Base; end; end`).
    /// Tree-shake uses this to register parsed classes under their
    /// qualified name (`ActiveRecord::Base`) so app-side parent
    /// references like `class ApplicationRecord < ActiveRecord::Base`
    /// resolve in the registry. Empty when no module wrapper.
    namespace: &'static str,
    out_path: &'static str,
    mode: Mode,
    imports: ImportSpec,
    prelude: &'static str,
    /// `(class, method)` pairs that the per-target hand-written
    /// runtime references but that wouldn't be discovered by the
    /// reachability walk over app-side bodies.
    extra_roots: ExtraRoots,
}

/// One emitted runtime file ready to be written into the target
/// project's `src/` tree.
pub struct RuntimeUnit {
    pub out_path: PathBuf,
    pub content: String,
    /// The parsed `LibraryClass`es (for `Mode::Library`). Empty
    /// for `Mode::Module`. Tree-shake operates on these *before*
    /// they're emitted; today the parse-and-emit happens together,
    /// but the data is preserved so a future tree-shake pass can
    /// filter `methods` before re-emit.
    pub classes: Vec<LibraryClass>,
    pub functions: Vec<MethodDef>,
    /// Enclosing module name (e.g. `"ActiveRecord"`). See
    /// `RuntimeEntry.namespace`.
    pub namespace: &'static str,
    /// Hand-written runtime call sites, propagated from
    /// `RuntimeEntry.extra_roots`.
    pub extra_roots: ExtraRoots,
}

/// The TypeScript-target transpile table. Each entry corresponds
/// to one `runtime/ruby/<x>.rb` file we want to emit as a TS file
/// in the output project.
const TYPESCRIPT_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "src/inflector.ts",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "ActiveRecord",
        out_path: "src/errors.ts",
        mode: Mode::Library,
        imports: &[("type Base", "./active_record_base.js")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/validations.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/validations.rbs"),
        rb_path: "runtime/ruby/active_record/validations.rb",
        namespace: "ActiveRecord",
        out_path: "src/validations.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "src/active_record_base.ts",
        mode: Mode::Library,
        imports: &[
            ("Validations", "./validations.js"),
            ("RecordNotFound, RecordInvalid", "./errors.js"),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "src/action_controller_base.ts",
        mode: Mode::Library,
        imports: &[("Parameters", "./parameters.js")],
        // Module-scope `STATUS_CODES = {...}.freeze` from base.rb.
        // Hand-mirrored until the lowerer learns module constants.
        prelude: "const STATUS_CODES: Record<string, number> = {\n\
        \x20 ok: 200, created: 201, accepted: 202, no_content: 204,\n\
        \x20 moved_permanently: 301, found: 302, see_other: 303, not_modified: 304,\n\
        \x20 bad_request: 400, unauthorized: 401, forbidden: 403, not_found: 404,\n\
        \x20 unprocessable_entity: 422, internal_server_error: 500,\n\
        };\n\n",
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/parameters.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/parameters.rbs"),
        rb_path: "runtime/ruby/action_controller/parameters.rb",
        namespace: "ActionController",
        out_path: "src/parameters.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "ActionDispatch",
        out_path: "src/router.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written `server.ts` and `test_support.ts` call
        // `Router.match(method, path, table)` directly. Treeshake's
        // app-side walk doesn't see those callsites, so without
        // these roots `Router.match` would drop and `src/router.ts`
        // would emit empty.
        extra_roots: &[("Router", "match")],
    },
];

/// Parse + emit the TypeScript runtime files. Returns one `RuntimeUnit`
/// per entry, ready to be written to disk by the caller.
///
/// Caller passes a `transform` closure that gets the parsed
/// `Vec<LibraryClass>` before emission — this is the hook for
/// runtime tree-shake (filter `lc.methods` to a reachable set).
/// Callers that don't need tree-shake pass `|c| c` (identity).
pub fn typescript_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(TYPESCRIPT_RUNTIME.len());
    for entry in TYPESCRIPT_RUNTIME {
        let unit = transpile_entry(entry, &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

fn transpile_entry<F>(entry: &RuntimeEntry, transform: &mut F) -> Result<RuntimeUnit, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let (emitted, classes, functions) = match entry.mode {
        Mode::Module => {
            let methods = parse_methods_with_rbs(entry.rb_src, entry.rbs_src)?;
            let body = emit_module(&methods)?;
            (body, Vec::new(), methods)
        }
        Mode::Library => {
            let classes = parse_library_with_rbs(
                entry.rb_src.as_bytes(),
                entry.rbs_src,
                entry.rb_path,
            )?;
            let classes = transform(entry.out_path, classes);
            let mut body = String::new();
            for (i, c) in classes.iter().enumerate() {
                if i > 0 {
                    body.push('\n');
                }
                body.push_str(&emit_library_class(c)?);
            }
            (body, classes, Vec::new())
        }
    };

    let mut import_block = String::new();
    for (name, source) in entry.imports {
        import_block.push_str(&format!("import {{ {name} }} from \"{source}\";\n"));
    }
    if !import_block.is_empty() {
        import_block.push('\n');
    }

    let header = format!(
        "// Generated from {} at app emit time.\n\
         // Do not edit by hand — edit the source `.rb` and re-run emit.\n\n",
        entry.rb_path,
    );

    let content = format!("{header}{import_block}{}{}", entry.prelude, emitted);

    Ok(RuntimeUnit {
        out_path: PathBuf::from(entry.out_path),
        content,
        classes,
        functions,
        namespace: entry.namespace,
        extra_roots: entry.extra_roots,
    })
}
