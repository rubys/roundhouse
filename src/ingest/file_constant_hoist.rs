//! Whether a file-level constant hoist onto the first host class would run.
//!
//! Ingest places remaining top-level constants on the first ordinary class of
//! the file, else the first admitted `Class.new` factory. That is sound only
//! when every reader can see that host and the value does not name a class
//! defined after it — otherwise Ruby NameErrors at load or call time.

use std::collections::{HashMap, HashSet};

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::library_class_order;

use super::{IngestError, IngestResult};

/// Refuse a file-level constant hoist that would not run. The host is the
/// first ordinary class of the file, else the first admitted factory —
/// the same choice the hoist itself uses. A reader outside that host
/// (and outside its nested classes / subclasses) cannot see a constant
/// placed in the host body; a rooted `::NAME` lookup still wants Object
/// even on the host; a value that names a class defined after the host
/// runs before that class exists.
pub(super) fn ensure_safe(
    file: &str,
    file_constants: &[(Symbol, Expr)],
    out: &[LibraryClass],
    factories: &[LibraryClass],
) -> IngestResult<()> {
    let host = out
        .first()
        .or_else(|| factories.first())
        .ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: "file-level constants beside Class.new factories need a host class that can keep them reachable".into(),
        })?;
    let host_name = host.name.0.as_str();
    let by_name: HashMap<&str, &LibraryClass> = out
        .iter()
        .chain(factories.iter())
        .map(|class| (class.name.0.as_str(), class))
        .collect();
    let ordered = ordered_names(factories, out);
    let host_pos = ordered
        .iter()
        .position(|name| *name == host_name)
        .ok_or_else(|| IngestError::Unsupported {
            file: file.into(),
            message: format!("file-level constant host `{host_name}` missing from the file's ordered classes"),
        })?;
    let after_host: HashSet<&str> = ordered.iter().skip(host_pos + 1).copied().collect();
    let hoisted: HashSet<&str> = file_constants.iter().map(|(name, _)| name.as_str()).collect();

    for class in out.iter().chain(factories.iter()) {
        let sees_host = class_sees_host_constants(class, host_name, &by_name);
        for expr in class_reader_exprs(class) {
            // Classes that see the host can use a bare `TIMEOUT` after
            // hoist. Rooted `::TIMEOUT` still means Object::TIMEOUT, so
            // refuse that on every class — including the host.
            let hit = find_const_path(expr, |path| {
                if is_rooted_path(path) {
                    is_hoisted_const_path(path, &hoisted)
                } else {
                    !sees_host && is_hoisted_const_path(path, &hoisted)
                }
            });
            if let Some(name) = hit {
                return Err(IngestError::Unsupported {
                    file: file.into(),
                    message: format!(
                        "file-level constant `{name}` is read from `{}`, which cannot see it after hoisting onto `{host_name}`",
                        class.name.0.as_str()
                    ),
                });
            }
        }
    }
    for (_, value) in file_constants {
        if let Some(name) = find_const_path(value, |path| is_rooted_path(path) && is_hoisted_const_path(path, &hoisted))
        {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "file-level constant value roots `::{name}`, which would miss the hoist onto `{host_name}`"
                ),
            });
        }
        if let Some(name) = find_const_path(value, |path| names_after_host_class(path, &after_host).is_some()) {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "file-level constant value names `{name}`, defined after host `{host_name}` that would hold it"
                ),
            });
        }
    }
    Ok(())
}

/// Emission order of the file's classes without cloning their bodies —
/// same deps rule as [`super::class_new_factory::order_with_file_classes`].
fn ordered_names<'a>(factories: &'a [LibraryClass], rest: &'a [LibraryClass]) -> Vec<&'a str> {
    if factories.is_empty() {
        return rest.iter().map(|class| class.name.0.as_str()).collect();
    }
    let pending: Vec<&LibraryClass> = factories.iter().chain(rest.iter()).collect();
    library_class_order::order_by_deps(
        pending,
        |class| class.name.0.as_str(),
        |class| {
            class
                .parent
                .iter()
                .chain(class.includes.iter())
                .map(|dep| dep.0.as_str().to_string())
                .collect()
        },
    )
    .into_iter()
    .map(|class| class.name.0.as_str())
    .collect()
}

fn class_sees_host_constants(
    class: &LibraryClass,
    host: &str,
    by_name: &HashMap<&str, &LibraryClass>,
) -> bool {
    let name = class.name.0.as_str();
    if name == host || name.starts_with(&format!("{host}::")) {
        return true;
    }
    let mut cur = class.parent.as_ref().map(|parent| parent.0.as_str());
    let mut seen = HashSet::new();
    while let Some(parent) = cur {
        if !seen.insert(parent) {
            break;
        }
        if parent == host {
            return true;
        }
        cur = by_name
            .get(parent)
            .and_then(|class| class.parent.as_ref())
            .map(|parent| parent.0.as_str());
    }
    false
}

/// Expressions that may read a file-level constant after hoist: method
/// bodies, their parameter defaults (including the block param), class
/// constants, class-ivar seeds, and unknown class-body calls.
fn class_reader_exprs(class: &LibraryClass) -> Vec<&Expr> {
    let mut out = Vec::new();
    for method in &class.methods {
        out.push(&method.body);
        for param in &method.params {
            if let Some(default) = &param.default {
                out.push(default);
            }
        }
        if let Some(default) = method.block_param.as_ref().and_then(|param| param.default.as_ref()) {
            out.push(default);
        }
    }
    out.extend(class.constants.iter().map(|(_, value)| value));
    out.extend(class.class_ivar_initializers.iter());
    out.extend(class.unknown_calls.iter());
    out
}

fn is_rooted_path(path: &[Symbol]) -> bool {
    path.first().is_some_and(|segment| segment.as_str().is_empty())
}

/// A const whose first real segment is a hoisted top-level name: bare
/// `TIMEOUT`, rooted `::TIMEOUT`, or qualified `TIMEOUT::PI` /
/// `::TIMEOUT::PI`. After hoist those all miss Object.
fn is_hoisted_const_path(path: &[Symbol], hoisted: &HashSet<&str>) -> bool {
    // Bare `TIMEOUT`, rooted `::TIMEOUT`, and qualified `TIMEOUT::PI` /
    // `::TIMEOUT::PI` — any path whose first real segment is a hoisted
    // name would miss Object after the hoist.
    let segments: Vec<&str> = path
        .iter()
        .map(|segment| segment.as_str())
        .filter(|segment| !segment.is_empty())
        .collect();
    segments
        .first()
        .copied()
        .is_some_and(|first| hoisted.contains(first))
}

/// True when any non-empty prefix of the const path is a class defined
/// after the host (`LimitError` and `LimitError::TOKEN` both hit).
fn names_after_host_class(path: &[Symbol], after_host: &HashSet<&str>) -> Option<String> {
    let segments: Vec<&str> = path
        .iter()
        .map(|segment| segment.as_str())
        .filter(|segment| !segment.is_empty())
        .collect();
    for end in 1..=segments.len() {
        let joined = segments[..end].join("::");
        if after_host.contains(joined.as_str()) {
            return Some(joined);
        }
    }
    None
}

fn find_const_path(expr: &Expr, matches: impl Fn(&[Symbol]) -> bool) -> Option<String> {
    let mut found = None;
    for_each_const_path(expr, &mut |path| {
        if found.is_some() {
            return;
        }
        if matches(path) {
            found = Some(
                path.iter()
                    .map(|segment| segment.as_str())
                    .filter(|segment| !segment.is_empty())
                    .collect::<Vec<_>>()
                    .join("::"),
            );
        }
    });
    found
}

/// Every const path in `expr`, including `LValue::Const` assign /
/// op-assign targets that `for_each_child` does not expose as Exprs.
fn for_each_const_path(expr: &Expr, visit: &mut dyn FnMut(&[Symbol])) {
    if let ExprNode::Const { path } = &*expr.node {
        visit(path);
    }
    match &*expr.node {
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
            if let LValue::Const { path } = target {
                visit(path);
            }
        }
        ExprNode::MultiAssign { targets, .. } => {
            for target in targets {
                if let LValue::Const { path } = target {
                    visit(path);
                }
            }
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |child| for_each_const_path(child, visit));
}
