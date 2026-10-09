//! Hand-written support modules for Ruby constants the app names but
//! no shared runtime file provides to the Rust target.
//!
//! Each [`SupportModule`] pairs a top-level constant (`SecureRandom`)
//! with a `runtime/rust/<stem>.rs` file. [`apply`] ships the file, and
//! imports the constant into every emitted file that names it, only
//! when some emitted body does — an app that never names the constant
//! carries no extra module (real-blog's emit is unchanged).
//!
//! A constant with no module here is not faked: the emitted reference
//! stays unresolved and rustc names it, which is the honest "not
//! supported yet" signal (AGENTS.md invariant 6).

use std::path::PathBuf;

use crate::emit::EmittedFile;

pub struct SupportModule {
    /// Ruby constant as it appears in emitted code (`Const::method(..)`).
    pub constant: &'static str,
    /// Module file stem under `src/`.
    pub stem: &'static str,
    /// Hand-written module text, or `None` when the module is a
    /// transpiled runtime file (`runtime_loader::RUST_RUNTIME`) that is
    /// already among the emitted files when the app needs it.
    pub source: Option<&'static str>,
}

pub const SUPPORT_MODULES: &[SupportModule] = &[SupportModule {
    constant: "BrowserBlocker",
    stem: "browser_blocker",
    source: None,
}, SupportModule {
    constant: "SecureRandom",
    stem: "secure_random",
    source: Some(include_str!("../../../runtime/rust/secure_random.rs")),
}, SupportModule {
    constant: "SchematizedJson",
    stem: "schematized_json",
    source: Some(include_str!("../../../runtime/rust/schematized_json.rs")),
}, SupportModule {
    constant: "ActiveJob",
    stem: "active_job",
    source: Some(include_str!("../../../runtime/rust/active_job.rs")),
}, SupportModule {
    constant: "TokenFor",
    stem: "token_for",
    source: Some(include_str!("../../../runtime/rust/token_for.rs")),
}, SupportModule {
    constant: "ActiveSupport",
    stem: "active_support",
    source: Some(include_str!("../../../runtime/rust/active_support.rs")),
}];

/// Adds each used support module and its import. Run before
/// `emit_lib_rs` so `lib.rs` declares the new modules.
pub fn apply(files: &mut Vec<EmittedFile>) {
    for module in SUPPORT_MODULES {
        let module_path = PathBuf::from(format!("src/{}.rs", module.stem));
        let import = format!(
            "#[allow(unused_imports)]\nuse crate::{}::{};\n",
            module.stem, module.constant
        );
        if module.source.is_none() && !files.iter().any(|f| f.path == module_path) {
            continue;
        }
        let needle = format!("{}::", module.constant);
        let mut used = false;
        for file in files.iter_mut() {
            if !is_app_rust_file(&file.path) || file.path == module_path {
                continue;
            }
            if !names_constant(&file.content, &needle) || file.content.contains(&import) {
                continue;
            }
            file.content = insert_import(&file.content, &import);
            used = true;
        }
        if let (true, Some(source)) = (used, module.source) {
            if !files.iter().any(|f| f.path == module_path) {
                files.push(EmittedFile {
                    path: module_path,
                    content: source.to_string(),
                });
            }
        }
    }
}

/// App-side emitted source: everything under `src/` except the
/// hand-written and transpiled runtime files at the crate root.
fn is_app_rust_file(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy();
    p.ends_with(".rs") && p.starts_with("src/") && p.matches('/').count() >= 2
}

/// `Const::` at an identifier boundary (not `OtherConst::`, not `a::Const::`).
fn names_constant(content: &str, needle: &str) -> bool {
    content.match_indices(needle).any(|(i, _)| {
        let before = content[..i].chars().next_back();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    })
}

/// Inserts after any leading inner doc/attribute lines, which must stay first.
fn insert_import(content: &str, import: &str) -> String {
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("//!") || t.starts_with("#![") {
            offset += line.len();
        } else {
            break;
        }
    }
    format!("{}{}{}", &content[..offset], import, &content[offset..])
}
