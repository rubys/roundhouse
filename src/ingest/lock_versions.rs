//! Not left to the runtime: the emitted program runs on roundhouse's, where no framework or gem version exists, so the lockfile's is the literal.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::gems::Lockfile;
use crate::ident::Symbol;

const FRAMEWORKS: &[(&str, &str)] = &[
    ("Rails", "railties"),
    ("ActiveRecord", "activerecord"),
    ("ActiveSupport", "activesupport"),
    ("ActiveModel", "activemodel"),
    ("ActionPack", "actionpack"),
    ("ActionView", "actionview"),
    ("ActiveJob", "activejob"),
    ("ActionMailer", "actionmailer"),
    ("ActionCable", "actioncable"),
    ("ActiveStorage", "activestorage"),
    ("ActionText", "actiontext"),
    ("ActionMailbox", "actionmailbox"),
];

pub(super) fn fold_lock_versions(app: &mut App) {
    let Some(lock) = app.gem_lock.clone() else { return };
    crate::lower::for_each_hook_body(app, &mut |body| fold(body, &lock));
    for model in &mut app.models {
        for item in &mut model.body {
            let crate::dialect::ModelBodyItem::Association { assoc, .. } = item else { continue };
            let expr = match assoc {
                crate::dialect::Association::BelongsTo { default, .. } => default.as_mut(),
                crate::dialect::Association::HasMany { scope, .. } | crate::dialect::Association::HasOne { scope, .. } => scope.as_mut(),
                _ => None,
            };
            if let Some(expr) = expr {
                fold(expr, &lock);
            }
        }
    }
    for view in &mut app.views {
        fold(&mut view.body, &lock);
    }
}

fn fold(e: &mut Expr, lock: &Lockfile) {
    if let Some(node) = folded(e, lock) {
        *e.node = node;
        return;
    }
    e.node.for_each_child_mut(&mut |c| fold(c, lock));
}

fn folded(e: &Expr, lock: &Lockfile) -> Option<ExprNode> {
    match &*e.node {
        ExprNode::Const { path } => {
            let path: Vec<&str> = path.iter().map(Symbol::as_str).skip_while(|s| s.is_empty()).collect();
            let [framework, "VERSION", field] = path.as_slice() else { return None };
            let version = Parts::of(framework_version(lock, framework)?)?;
            Some(match *field {
                "MAJOR" => int(version.major),
                "MINOR" => int(version.minor),
                "TINY" => int(version.tiny),
                "PRE" => version.pre.map_or(ExprNode::Lit { value: Literal::Nil }, str_lit),
                "STRING" => str_lit(version.text),
                _ => return None,
            })
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } if args.is_empty() => {
            let ExprNode::Const { path } = &*recv.node else {
                return gem_spec_version(recv, method.as_str(), lock);
            };
            let [framework] = path.iter().map(Symbol::as_str).skip_while(|s| s.is_empty()).collect::<Vec<_>>()[..] else {
                return None;
            };
            let version = framework_version(lock, framework)?;
            Parts::of(version)?;
            match method.as_str() {
                // Rails alone answers `version` as a String; the frameworks answer a Gem::Version.
                "version" if framework == "Rails" => Some(str_lit(version)),
                "version" | "gem_version" => Some(gem_version_new(e, version)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn gem_spec_version(spec: &Expr, method: &str, lock: &Lockfile) -> Option<ExprNode> {
    if method != "version" {
        return None;
    }
    let ExprNode::Send { recv: Some(specs), method: index, args, block: None, .. } = &*spec.node else { return None };
    let ExprNode::Send { recv: Some(gem), method: loaded, args: none, block: None, .. } = &*specs.node else { return None };
    let named_gem = matches!(&*gem.node, ExprNode::Const { path }
        if path.iter().map(Symbol::as_str).skip_while(|s| s.is_empty()).eq(["Gem"]));
    if index.as_str() != "[]" || loaded.as_str() != "loaded_specs" || !none.is_empty() || !named_gem {
        return None;
    }
    let [key] = args.as_slice() else { return None };
    let ExprNode::Lit { value: Literal::Str { value: name } } = &*key.node else { return None };
    Some(gem_version_new(spec, lock.release_version_of(name)?))
}

fn framework_version<'a>(lock: &'a Lockfile, framework: &str) -> Option<&'a str> {
    let (_, gem) = FRAMEWORKS.iter().find(|(name, _)| *name == framework)?;
    lock.release_version_of(gem)
}

struct Parts<'a> {
    major: i64,
    minor: i64,
    tiny: i64,
    pre: Option<&'a str>,
    text: &'a str,
}

impl<'a> Parts<'a> {
    fn of(text: &'a str) -> Option<Self> {
        let mut segments = text.split('.');
        let major = segments.next()?.parse().ok()?;
        let minor = segments.next()?.parse().ok()?;
        let tiny = segments.next()?.parse().ok()?;
        let pre = segments.next();
        if segments.next().is_some() {
            return None;
        }
        Some(Parts { major, minor, tiny, pre, text })
    }
}

fn int(value: i64) -> ExprNode {
    ExprNode::Lit { value: Literal::Int { value } }
}

fn str_lit(value: &str) -> ExprNode {
    ExprNode::Lit { value: Literal::Str { value: value.to_string() } }
}

fn gem_version_new(at: &Expr, version: &str) -> ExprNode {
    let mut class = Expr::new(at.span, ExprNode::Const { path: vec![Symbol::from("Gem"), Symbol::from("Version")] });
    // Not resolved at the borrowed span, where the resolver answered the name the read wrote; still reported there on a strict target.
    class.decisions |= crate::expr::GENERATED_CONST_REF;
    ExprNode::Send {
        recv: Some(class),
        method: Symbol::from("new"),
        args: vec![Expr::new(at.span, str_lit(version))],
        block: None,
        parenthesized: true,
    }
}
