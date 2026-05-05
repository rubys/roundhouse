//! Build `(ClassId, ClassInfo)` pairs from lowered LibraryClasses or
//! LibraryFunctions, suitable for threading into another lowerer's
//! shared class registry. Used by per-target emit pipelines (TS,
//! Crystal, ...) to wire model + view + route_helper signatures into
//! the controller and test lowerers so cross-class dispatch
//! (`Article.find(...)`, `Views::Articles.index(...)`,
//! `RouteHelpers.article_path(...)`) types correctly.
//!
//! Both helpers register under TWO ClassIds: the full path (e.g.
//! `Views::Articles`) and a last-segment alias (`Articles`). The
//! body-typer's `Const { path }` resolver instantiates by `path.last()`,
//! so the alias keeps bare references resolvable.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::dialect::{LibraryClass, LibraryFunction};
use crate::ident::{ClassId, Symbol};

/// Group methods across same-named LibraryClasses (e.g. view modules
/// from the same dir share a `Views::X` ClassId), then register each
/// group under both its full ClassId and its last-segment alias.
pub fn extras_from_lcs(lcs: &[LibraryClass]) -> Vec<(ClassId, ClassInfo)> {
    let mut grouped: HashMap<ClassId, ClassInfo> = HashMap::new();
    for lc in lcs {
        let info = grouped.entry(lc.name.clone()).or_default();
        let from = crate::lower::class_info_from_library_class(lc);
        for (k, v) in from.class_methods {
            info.class_methods.insert(k, v);
        }
        for (k, v) in from.instance_methods {
            info.instance_methods.insert(k, v);
        }
        for (k, v) in from.class_method_kinds {
            info.class_method_kinds.insert(k, v);
        }
        for (k, v) in from.instance_method_kinds {
            info.instance_method_kinds.insert(k, v);
        }
    }
    finalize_with_alias(grouped)
}

/// Group LibraryFunctions by module_path so all `RouteHelpers.*` (or
/// `Schema.*`, etc.) functions register under one ClassId. Each
/// function becomes a class-method entry whose Ty is the function's
/// signature.
pub fn extras_from_funcs(funcs: &[LibraryFunction]) -> Vec<(ClassId, ClassInfo)> {
    let mut grouped: HashMap<ClassId, ClassInfo> = HashMap::new();
    for func in funcs {
        let raw = func
            .module_path
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("::");
        let id = ClassId(Symbol::from(raw));
        let info = grouped.entry(id).or_default();
        if let Some(sig) = &func.signature {
            info.class_methods.insert(func.name.clone(), sig.clone());
            info.class_method_kinds.insert(
                func.name.clone(),
                crate::dialect::AccessorKind::Method,
            );
        }
    }
    finalize_with_alias(grouped)
}

fn finalize_with_alias(
    grouped: HashMap<ClassId, ClassInfo>,
) -> Vec<(ClassId, ClassInfo)> {
    let mut out: Vec<(ClassId, ClassInfo)> = Vec::new();
    for (full_id, info) in grouped {
        let raw = full_id.0.as_str();
        let last = raw.rsplit("::").next().unwrap_or(raw).to_string();
        if last != raw {
            let mut alias = ClassInfo::default();
            alias.class_methods = info.class_methods.clone();
            alias.instance_methods = info.instance_methods.clone();
            alias.class_method_kinds = info.class_method_kinds.clone();
            alias.instance_method_kinds = info.instance_method_kinds.clone();
            out.push((ClassId(Symbol::from(last)), alias));
        }
        out.push((full_id, info));
    }
    out
}
