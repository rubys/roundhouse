//! `Dry::Initializer` classes lowered into the plain Ruby they stand for.
//!
//! ```ruby
//! class Command
//!   extend Dry::Initializer
//!   option :id, Types::Coercible::Integer
//!   option :note, Types::Strict::String, optional: true
//!   option :size, Types::Strict::Integer, default: -> { limit * 2 }
//! end
//! ```
//!
//! `extend Dry::Initializer` makes `param` and `option` the constructor and
//! the readers. The emitted tree has no dry-initializer, so each class gets
//! the constructor the gem builds, as `initialize(*params, options = {})`:
//! the hash is how every target passes keywords. (So a positional Hash
//! there, which the gem ignores, is read as the options too, and a call
//! leaving out a param, which the gem rejects, binds the options to it.)
//! A `param` is read from its position, an `option` from its key, each
//! through its type (a dry type, read by `ingest::dry_types`, or a `proc`).
//! A missing option takes its default (a block run on the instance, its
//! value typed too), is left unset when `optional:` (its reader nil, its
//! instance variable the gem's `UNDEFINED` when the app reads that), and
//! otherwise raises `KeyError`. Type failures raise dry-types' own
//! `CoercionError`/`ConstraintError`, which the gem lets through. Unknown
//! keys are ignored, as the gem ignores them; `required: true` drops the
//! default and `optional:`; a type's own `.default` is never reached. A
//! subclass's options follow its parent's, and one declared again
//! replaces the earlier where it stood: that class assigns the whole
//! hierarchy itself.
//!
//! Checked against dry-initializer 3.2. A class defining its own
//! `initialize` where one is generated, redeclaring a param, declaring a
//! `param` that may be left out, or declaring anything not
//! modeled here is left as it was, ledgered, with its whole hierarchy, as
//! under `Dry::Struct`: a refused class under a lowered ancestor would
//! inherit a constructor that ignores its own options. The refused classes
//! keep no declarations, so constructing one is reported, not emitted to
//! fail.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{LibraryClass, LibraryClassOrigin, MethodReceiver};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

use super::dry_types::{
    Base, DryType, Gen, Scope, coerced, dry_type, resolve, types_modules,
};
use super::{IngestError, survey};

/// One `param` or `option`.
#[derive(Clone)]
struct Declared {
    /// The key it is read from, and its index among the hierarchy's params.
    name: Symbol,
    param: Option<usize>,
    /// The reader and instance variable: `as:`, or the name.
    target: Symbol,
    ty: Option<DryType>,
    /// A block, run on the instance when the value is missing.
    default: Option<Expr>,
    optional: bool,
    reader: Reader,
}

#[derive(Clone, Copy, PartialEq)]
enum Reader {
    Public,
    Private,
    Protected,
    None,
}

pub(super) fn lower_dry_initializers(app: &mut App, sources: &[crate::span::SourceFile]) {
    let types_modules = types_modules(sources);
    let names: HashSet<String> = app
        .library_classes
        .iter()
        .flat_map(|lc| {
            std::iter::once(lc.name.0.as_str().to_string()).chain(
                lc.constants
                    .iter()
                    .map(move |(n, _)| format!("{}::{}", lc.name.0.as_str(), n.as_str())),
            )
        })
        .collect();
    // The app's own classes, which an `Array.of` may name in place of a type.
    let classes: HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()))
        .collect();
    let constants: HashMap<String, Expr> = app
        .library_classes
        .iter()
        .flat_map(|lc| {
            lc.constants
                .iter()
                .map(move |(n, v)| (format!("{}::{}", lc.name.0.as_str(), n.as_str()), v.clone()))
        })
        .collect();
    // Only a struct lowered by `ingest::dry_struct` can be built here.
    let structs: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .filter(|lc| lc.origin == Some(LibraryClassOrigin::DryStruct))
        .map(|lc| (lc.name.0.as_str().to_string(), None))
        .collect();

    // Each initializer class and its initializer parent (None for the class
    // that extends `Dry::Initializer`).
    let class_names: HashSet<String> = app
        .library_classes
        .iter()
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    for lc in &app.library_classes {
        if !lc.is_module && lc.unknown_calls.iter().any(extends_initializer) {
            parent_of.insert(lc.name.0.as_str().to_string(), None);
        }
    }
    if parent_of.is_empty() {
        return;
    }
    loop {
        let before = parent_of.len();
        for lc in &app.library_classes {
            let name = lc.name.0.as_str();
            if parent_of.contains_key(name) || lc.is_module {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            if let Some(resolved) = resolve(
                name,
                parent.0.as_str().trim_start_matches("::"),
                &class_names,
            ) {
                if parent_of.contains_key(&resolved) {
                    parent_of.insert(name.to_string(), Some(resolved));
                }
            }
        }
        if parent_of.len() == before {
            break;
        }
    }
    let chain_of = |name: &str| -> Vec<String> {
        let mut chain = vec![name.to_string()];
        while let Some(Some(parent)) = parent_of.get(chain.last().unwrap()) {
            chain.push(parent.clone());
        }
        chain.reverse();
        chain
    };

    // Read every class, ancestors first so a param knows its position and
    // an option its parent's names.
    let mut order: Vec<String> = parent_of.keys().cloned().collect();
    order.sort_by_key(|n| chain_of(n).len());
    let mut read: HashMap<String, Vec<Declared>> = HashMap::new();
    let mut refused: HashSet<String> = HashSet::new();
    for name in &order {
        let lc = app
            .library_classes
            .iter()
            .find(|lc| lc.name.0.as_str() == name)
            .expect("listed");
        let ancestors: Vec<String> = chain_of(name).into_iter().filter(|n| n != name).collect();
        let inherited = effective(&ancestors, &read);
        let scope = Scope {
            types_modules: &types_modules,
            owner: name,
            names: &names,
            structs: &structs,
            constants: &constants,
            classes: &classes,
            depth: 0,
        };
        match read_class(lc, &scope, &inherited, parent_of[name].is_none()) {
            Ok(declared) => {
                read.insert(name.clone(), declared);
            }
            Err(reason) => {
                survey::record(&IngestError::Unsupported {
                    file: name.clone(),
                    message: format!("Dry::Initializer not lowered: {reason}"),
                });
                refused.insert(name.clone());
            }
        }
    }
    // `Dry::Initializer::UNDEFINED` read anywhere: an option left out holds
    // it, as under the gem. Otherwise nil, keeping the variable one type.
    let sentinel = app.library_classes.iter().any(|lc| {
        lc.unknown_calls.iter().any(reads_undefined)
            || lc.methods.iter().any(|m| reads_undefined(&m.body))
    });

    // Build every class's methods before committing to any.
    let mut built: HashMap<String, (Vec<crate::dialect::MethodDef>, Vec<(Symbol, Expr)>)> =
        HashMap::new();
    let mut failed: HashSet<String> = HashSet::new();
    for (name, declared) in &read {
        if chain_of(name).iter().any(|n| refused.contains(n)) {
            continue;
        }
        let ancestors: Vec<String> = chain_of(name).into_iter().filter(|n| n != name).collect();
        let inherited = effective(&ancestors, &read);
        // A class redeclaring an ancestor's option assigns the whole
        // hierarchy itself: the ancestor's would refuse what it now allows.
        let source = if declared.iter().any(|d| inherited.iter().any(|i| i.name == d.name)) {
            let all: Vec<Declared> = effective(&chain_of(name), &read).into_iter().cloned().collect();
            synthesized_source(name, &all, 0, true, sentinel)
        } else {
            let params = inherited.iter().filter(|d| d.param.is_some()).count();
            synthesized_source(name, declared, params, parent_of[name].is_none(), sentinel)
        };
        let (parsed, diags) = crate::ingest::prism::scope(|| {
            crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_initializer>")
        });
        match parsed {
            Ok(classes) if diags.is_empty() => {
                let mut methods = Vec::new();
                let mut generated = Vec::new();
                for c in classes {
                    methods.extend(c.methods);
                    generated.extend(c.constants);
                }
                built.insert(name.clone(), (methods, generated));
            }
            Ok(_) => {
                survey::record_synthesis_failure(name.clone(), "Dry::Initializer lowering", &diags);
                failed.insert(name.clone());
            }
            Err(err) => {
                survey::record(&err);
                failed.insert(name.clone());
            }
        }
    }

    // All or nothing per hierarchy: a refused class under a lowered
    // ancestor would inherit a constructor that ignores its own options.
    let refused_roots: HashSet<String> = refused
        .iter()
        .chain(&failed)
        .map(|n| chain_of(n)[0].clone())
        .collect();
    let mut any = false;
    for lc in &mut app.library_classes {
        let name = lc.name.0.as_str().to_string();
        let Some(parent) = parent_of.get(&name) else {
            continue;
        };
        // The parent by its resolved name, as later passes look it up; a
        // refused class too, or the emitter takes `Base` for a gem's class
        // and replays the declarations it cannot load.
        if let Some(parent) = parent {
            lc.parent = Some(ClassId(Symbol::from(parent.as_str())));
        }
        if refused_roots.contains(&chain_of(&name)[0]) {
            continue;
        }
        let Some((mut methods, generated)) = built.remove(&name) else {
            continue;
        };
        lc.unknown_calls
            .retain(|call| !is_initializer_declaration(call));
        methods.append(&mut lc.methods);
        lc.methods = methods;
        lc.constants.extend(generated);
        lc.origin = Some(LibraryClassOrigin::DryInitializer);
        any = true;
    }
    // The gem is never emitted, and a refused hierarchy's declarations
    // are dropped with it: what the lowered classes raise must exist.
    if any {
        app.library_classes.extend(error_classes(sentinel));
    }
}

fn reads_undefined(expr: &Expr) -> bool {
    let mut found = is_const(expr, "Dry::Initializer::UNDEFINED");
    expr.node
        .for_each_child(&mut |c| found |= reads_undefined(c));
    found
}

/// After both lowerings, the constants holding a dry type go, app-wide.
/// The gems are never emitted, so such a constant would fail the load of
/// everything after it, lowered or not; a class left on the gems cannot
/// run either way. Not before this point: the lowerings read them.
pub(super) fn drop_type_constants(app: &mut App, sources: &[crate::span::SourceFile]) {
    let types_modules = types_modules(sources);
    if types_modules.is_empty() {
        return;
    }
    let names: HashSet<String> = app
        .library_classes
        .iter()
        .map(|lc| lc.name.0.as_str().to_string())
        .collect();
    let plain = super::dry_types::plain_constants(&app.library_classes, &types_modules, &names);
    for lc in &mut app.library_classes {
        let holder = lc.name.0.as_str().to_string();
        lc.constants.retain(|(_, value)| {
            !super::dry_types::holds_dry_type(value, &holder, &types_modules, &names, &plain)
        });
    }
}

fn is_const(expr: &Expr, name: &str) -> bool {
    matches!(&*expr.node, ExprNode::Const { path }
        if path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::") == name)
}

fn extends_initializer(call: &Expr) -> bool {
    matches!(&*call.node, ExprNode::Send { recv: None, method, args, block: None, .. }
        if method.as_str() == "extend" && args.len() == 1 && is_const(&args[0], "Dry::Initializer"))
}

fn is_initializer_declaration(call: &Expr) -> bool {
    extends_initializer(call)
        || matches!(&*call.node, ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "param" | "option"))
}

fn read_class(
    lc: &LibraryClass,
    scope: &Scope<'_>,
    inherited: &[&Declared],
    is_root: bool,
) -> Result<Vec<Declared>, String> {
    let mut params = inherited.iter().filter(|d| d.param.is_some()).count();
    let mut declared: Vec<Declared> = Vec::new();
    for call in &lc.unknown_calls {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            ..
        } = &*call.node
        else {
            continue;
        };
        let is_param = match method.as_str() {
            "param" => true,
            "option" => false,
            _ => continue,
        };
        if block.is_some() {
            return Err(format!("`{}` with a block", method.as_str()));
        }
        let d = read_declaration(args, scope).map_err(|e| {
            format!(
                "`{} {}`: {e}",
                method.as_str(),
                args.first()
                    .map(crate::emit::ruby::emit_expr)
                    .unwrap_or_default()
            )
        })?;
        // An option declared again replaces the earlier one in place, as
        // the gem does; a param keeps its position, so it may not be.
        if let Some(earlier) = inherited
            .iter()
            .copied()
            .chain(declared.iter())
            .find(|o| o.name == d.name)
            && (earlier.param.is_some() || is_param)
        {
            return Err(format!("`{}` declared again", d.name.as_str()));
        }
        // Positions are fixed in the signature: one that may be left out
        // would need the gem's arity juggling.
        if is_param && (d.optional || d.default.is_some()) {
            return Err(format!("`param {}` that may be left out", d.name.as_str()));
        }
        let d = Declared {
            param: is_param.then(|| {
                params += 1;
                params - 1
            }),
            ..d
        };
        match declared.iter_mut().find(|o| o.name == d.name) {
            Some(earlier) => *earlier = d,
            None => declared.push(d),
        }
    }
    // The gem's constructor sits in a module, under any the class writes;
    // the lowered one is the class's own, so only a class that gets none
    // (a subclass adding no params) may write its own and `super` to it.
    let own_initialize = lc
        .methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "initialize");
    let redeclares = declared.iter().any(|d| inherited.iter().any(|i| i.name == d.name));
    if own_initialize && (is_root || redeclares || declared.iter().any(|d| d.param.is_some())) {
        return Err("the class defines its own `initialize`".into());
    }
    Ok(declared)
}

fn read_declaration(args: &[Expr], scope: &Scope<'_>) -> Result<Declared, String> {
    let sym = |e: &Expr| match &*e.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.clone()),
        _ => None,
    };
    let (name, rest) = args.split_first().ok_or("no name")?;
    let name = sym(name).ok_or("a name that is not a symbol")?;
    let (positional, options): (Vec<&Expr>, Vec<&Expr>) = rest
        .iter()
        .partition(|a| !matches!(&*a.node, ExprNode::Hash { .. }));
    let mut d = Declared {
        name: name.clone(),
        param: None,
        target: name,
        ty: None,
        default: None,
        optional: false,
        reader: Reader::Public,
    };
    let mut required = false;
    let mut type_expr = match positional.as_slice() {
        [] => None,
        [ty] => Some(*ty),
        _ => return Err("more than a type".into()),
    };
    for options in options {
        let ExprNode::Hash { entries, .. } = &*options.node else {
            unreachable!()
        };
        for (key, value) in entries {
            let key = sym(key).ok_or("a computed option")?;
            match key.as_str() {
                "type" if type_expr.is_none() => type_expr = Some(value),
                "type" => return Err("two types".into()),
                "optional" => match &*value.node {
                    ExprNode::Lit {
                        value: Literal::Bool { value },
                    } => d.optional = *value,
                    _ => return Err("`optional:` that is not true or false".into()),
                },
                "default" => {
                    d.default = Some(
                        callable_body(value)
                            .ok_or("`default:` that is not a block")?
                            .1,
                    )
                }
                "as" => d.target = sym(value).ok_or("`as:` that is not a symbol")?,
                "reader" => {
                    d.reader = match &*value.node {
                        ExprNode::Lit {
                            value: Literal::Bool { value: false },
                        } => Reader::None,
                        ExprNode::Lit {
                            value: Literal::Bool { value: true },
                        } => Reader::Public,
                        ExprNode::Lit {
                            value: Literal::Sym { value },
                        } => match value.as_str() {
                            "public" => Reader::Public,
                            "private" => Reader::Private,
                            "protected" => Reader::Protected,
                            _ => return Err("an unknown `reader:`".into()),
                        },
                        _ => return Err("a computed `reader:`".into()),
                    }
                }
                "required" => match &*value.node {
                    ExprNode::Lit {
                        value: Literal::Bool { value },
                    } => required = *value,
                    _ => return Err("`required:` that is not true or false".into()),
                },
                // The gem ignores keys it has no dispatcher for.
                _ => {}
            }
        }
    }
    if let Some(ty) = type_expr {
        d.ty = Some(match callable_body(ty) {
            // `proc { |v| ... }`, `->(v) { ... }`: called with the value. It
            // was written in the class body, whose self the helper keeps.
            Some((Some(param), body)) => DryType::of(Base::Constructor {
                param,
                body,
                then: Box::new(DryType::of(Base::Nominal)),
            }),
            Some((None, _)) => return Err("a type block without a parameter".into()),
            None => dry_type(ty, scope)
                .ok_or_else(|| format!("type `{}`", crate::emit::ruby::emit_expr(ty)))?,
        });
        // A dry type's own `.default` answers only an absent input, which
        // the gem never hands it: a given value goes through, a missing
        // one is the option's business.
        if let Some(ty) = d.ty.as_mut() {
            ty.default = None;
        }
    }
    // `required: true` overrides `default:` and `optional:`.
    if required {
        d.default = None;
        d.optional = false;
    }
    Ok(d)
}

/// What `chain` (root first) declares, a later option replacing an
/// earlier one of the same name where it stood.
fn effective<'a>(chain: &[String], read: &'a HashMap<String, Vec<Declared>>) -> Vec<&'a Declared> {
    let mut out: Vec<&Declared> = Vec::new();
    for d in chain.iter().filter_map(|n| read.get(n)).flatten() {
        match out.iter_mut().find(|o| o.name == d.name) {
            Some(slot) => *slot = d,
            None => out.push(d),
        }
    }
    out
}

/// `-> { }`, `proc { }`, `lambda { }`: its one parameter, if any, and its
/// body. None for anything else.
fn callable_body(expr: &Expr) -> Option<(Option<Symbol>, Expr)> {
    let lambda = match &*expr.node {
        ExprNode::Lambda { .. } => expr,
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: Some(block),
            ..
        } if args.is_empty() && matches!(method.as_str(), "proc" | "lambda") => block,
        _ => return None,
    };
    let ExprNode::Lambda { params, body, .. } = &*lambda.node else {
        return None;
    };
    match params.as_slice() {
        [] => Some((None, body.clone())),
        [param] => Some((Some(param.clone()), body.clone())),
        _ => None,
    }
}

/// `inherited` counts the params the ancestors declare.
fn synthesized_source(
    owner: &str,
    declared: &[Declared],
    inherited: usize,
    is_root: bool,
    sentinel: bool,
) -> String {
    let mut extra = Gen {
        owner: owner.to_string(),
        coercion_error: "::Dry::Types::CoercionError",
        constraint_error: "::Dry::Types::ConstraintError",
        ..Gen::default()
    };
    let own = declared.iter().filter(|d| d.param.is_some()).count();
    let params = |n: usize| -> String {
        (0..n)
            .map(|i| format!("dry_initializer_param_{i}, "))
            .collect()
    };
    let mut assign = String::new();
    if !is_root {
        assign.push_str(&format!("    super({}options)\n", params(inherited)));
    }
    for (i, d) in declared.iter().enumerate() {
        let key = d.name.as_str();
        let typed = |extra: &mut Gen, value: &str| match &d.ty {
            Some(ty) => coerced(extra, ty, value, &format!(":{key}")),
            None => value.to_string(),
        };
        if let Some(index) = d.param {
            let given = typed(&mut extra, &format!("dry_initializer_param_{index}"));
            assign.push_str(&format!("    @{} = {given}\n", d.target.as_str()));
            continue;
        }
        let given = typed(&mut extra, &format!("options[:{key}]"));
        let present = format!("options.key?(:{key})");
        let missing = match (&d.default, d.optional) {
            // The block on the instance, then through the type.
            (Some(body), _) => {
                let local = format!("dry_initializer_default_{i}");
                let checked = typed(&mut extra, &local);
                format!(
                    "{local} = (begin\n{}\nend)\n      {checked}",
                    crate::emit::ruby::emit_expr(body)
                )
            }
            (None, true) if sentinel => "::Dry::Initializer::UNDEFINED".to_string(),
            (None, true) => "nil".to_string(),
            (None, false) => {
                format!("raise(KeyError, \"#{{self.class}}: option '{key}' is required\")")
            }
        };
        assign.push_str(&format!(
            "    @{target} = if {present}\n      {given}\n    else\n      {missing}\n    end\n",
            target = d.target.as_str()
        ));
    }
    let mut out = format!("class {}\n", owner.rsplit("::").next().unwrap_or(owner));
    for constant in &extra.constants {
        out.push_str(&format!("  {constant}\n"));
    }
    for helper in &extra.helpers {
        out.push_str(helper);
        out.push('\n');
    }
    // A class adding params needs its own arity; the rest inherit it.
    if is_root || own > 0 {
        let all = params(inherited + own);
        out.push_str(&format!(
            "  def initialize({all}options = {{}})\n    dry_initializer_assign({all}options)\n  end\n\n"
        ));
    }
    for reader in [Reader::Public, Reader::Protected, Reader::Private] {
        let names: Vec<&Declared> = declared.iter().filter(|d| d.reader == reader).collect();
        if names.is_empty() {
            continue;
        }
        match reader {
            Reader::Protected => out.push_str("  protected\n\n"),
            Reader::Private => out.push_str("  private\n\n"),
            _ => {}
        }
        for d in names {
            let target = d.target.as_str();
            if sentinel && d.optional && d.default.is_none() && d.param.is_none() {
                out.push_str(&format!(
                    "  def {target}\n    @{target} unless ::Dry::Initializer::UNDEFINED == @{target}\n  end\n\n"
                ));
            } else {
                out.push_str(&format!("  def {target}\n    @{target}\n  end\n\n"));
            }
        }
    }
    out.push_str(&format!(
        "  private\n\n  def dry_initializer_assign({}options)\n",
        params(inherited + own)
    ));
    out.push_str(&assign);
    out.push_str("    self\n  end\nend\n");
    out
}

/// dry-types' error classes, which the lowered constructors raise and an
/// application may rescue, once nothing else of the gem is left; and the
/// gem's `UNDEFINED`, when read.
fn error_classes(sentinel: bool) -> Vec<LibraryClass> {
    let mut source = String::from(
        "module Dry\n  module Types\n    class CoercionError < StandardError\n    end\n\n    class ConstraintError < CoercionError\n    end\n  end\nend\n",
    );
    if sentinel {
        // A class: as unique as the gem's `Object.new`, and resolvable
        // from app source, which a synthesized value constant is not.
        source.push_str(
            "\nmodule Dry\n  module Initializer\n    class UNDEFINED\n    end\n  end\nend\n",
        );
    }
    crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_initializer>")
        .expect("the dry-types error stand-ins parse")
}
