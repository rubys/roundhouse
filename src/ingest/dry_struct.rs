//! `Dry::Struct` classes lowered into the plain Ruby they stand for.
//!
//! ```ruby
//! class CreateRefund < BaseResponse          # BaseResponse < Dry::Struct
//!   attribute :id, Types::Coercible::String
//!   attribute? :description, Types::Strict::String.optional
//! end
//! ```
//!
//! Like `T::Struct`, a `Dry::Struct` is a class generator: `attribute`
//! is the constructor and the reader. The emitted tree has no dry-struct,
//! so each class becomes a reader per attribute and an
//! `initialize(attributes = {})` that does what `Dry::Struct.new` does
//! with a Hash: transform its keys by the nearest `transform_keys`, take
//! each attribute's key, coerce or check it by its type, raise
//! `Dry::Struct::Error` when a required key is missing or a value does
//! not fit. A subclass's own attributes come after its parent's, as the
//! schema inherits.
//!
//! Types mean what dry-types makes them, checked against the gem: a bare
//! name is strict under `Dry.Types()`; `Coercible::`, `Strict::`,
//! `Params::` and `JSON::` scalars, dates and decimals; `Array.of`,
//! `Hash.schema`, `Instance(X)`, another struct class, `A | B`,
//! `Constructor(K) { }` and `.constructor { }`, `.optional`, `.enum`,
//! `.meta`, and `.default` (a value shared from where it is written, or
//! a block called each time); a constant holding any of these. A nested
//! `attribute ... do` becomes the `Owner::Name` struct dry-struct defines,
//! `attributes_from` inlines another struct's attributes, and
//! `attribute :name?` is `attribute? :name`.
//!
//! A class with anything else (`transform_types`, an app's own
//! `attribute` override) is left as it was and ledgered, and so is every
//! class that shares its `Dry::Struct` root or builds one that does: a
//! struct is all its attributes or none, and a hierarchy all its structs
//! or none. The date and decimal coercions need stdlib only the CRuby and
//! JRuby trees load; `project` reports them for any other target.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::{IngestError, survey};

/// What a `Types::...` expression checks or coerces.
#[derive(Clone, Debug)]
enum Base {
    /// `Coercible::String` etc.: `Kernel#String(v)`.
    Coerce(&'static str),
    /// `Coercible::Symbol`: `v.to_sym`.
    CoerceSymbol,
    /// `Params::Integer`: base-10 for a String, `Integer(v)` otherwise.
    ParamsInteger,
    /// `Params::Bool`: dry-types' true and false spellings.
    ParamsBool,
    /// `Strict::String` etc.: the value must already be one.
    Strict(&'static str),
    StrictBool,
    /// `Types.Instance(X)`: the value must be an `X`, as written.
    Instance(String),
    /// Another struct class: an instance passes, a Hash builds one.
    Struct(String),
    /// Nominal `String`, `Bool`, `Any`, ...: no check at all.
    Nominal,
    /// `Array.of(T)`, `Strict::Array.of(T)`: each member through `T`.
    ArrayOf { strict: bool, member: Box<DryType> },
    /// `Hash.schema(k: T, o?: T)`: a Hash of just those keys, each
    /// through its type; `o?` may be absent and is then left out.
    HashSchema(Vec<(String, bool, DryType)>),
    /// `A | B`: the first that accepts the value.
    Sum(Vec<DryType>),
    /// `Types.Constructor(K) { |v| ... }`, `T.constructor { |v| ... }`:
    /// the block's value, then through `then`.
    Constructor { param: Symbol, body: Expr, then: Box<DryType> },
    /// `Params::`/`JSON::` `Date`, `DateTime`, `Time`: a String parsed,
    /// an instance passed through.
    Parse(&'static str),
    /// `Params::Decimal`: a value `Float()` accepts, as `to_d` makes it.
    ParamsDecimal,
}

/// A `.default`: a value dry-types evaluates once, where the type is
/// written, and hands out every time; or a block it calls each time.
#[derive(Clone, Debug)]
enum DefaultValue {
    Value(Expr),
    Block(Expr),
}

impl DryType {
    /// The struct classes this type builds, which must be lowered too.
    fn structs(&self, out: &mut Vec<String>) {
        match &self.base {
            Base::Struct(name) => out.push(name.clone()),
            Base::ArrayOf { member, .. } => member.structs(out),
            Base::HashSchema(keys) => keys.iter().for_each(|(_, _, t)| t.structs(out)),
            Base::Sum(types) => types.iter().for_each(|t| t.structs(out)),
            Base::Constructor { then, .. } => then.structs(out),
            _ => {}
        }
    }
}

/// Where a type expression is read from: the `Types` modules, and the
/// struct classes a constant may name from inside `owner`.
struct Scope<'a> {
    types_modules: &'a [(String, BareNames)],
    owner: &'a str,
    names: &'a HashSet<String>,
    structs: &'a HashMap<String, Option<String>>,
    /// Every class-body constant, by full name, with its value.
    constants: &'a HashMap<String, Expr>,
    /// Constants followed so far, against a cycle.
    depth: usize,
}

#[derive(Clone, Debug)]
struct DryType {
    base: Base,
    optional: bool,
    default: Option<DefaultValue>,
    /// `.enum(...)`: the values the coerced value must be one of.
    enumeration: Option<Vec<Expr>>,
    /// `.meta(omittable: true)`: in a `Hash.schema`, the key may be absent.
    omittable: bool,
}

struct Attribute {
    name: Symbol,
    /// `attribute?`: the key may be absent.
    omittable: bool,
    ty: DryType,
}

struct StructClass {
    attributes: Vec<Attribute>,
    /// The `transform_keys` call this class declares, if any.
    transform_keys: Option<Expr>,
}

impl StructClass {
    fn structs(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.attributes {
            a.ty.structs(&mut out);
        }
        out
    }
}

pub(super) fn lower_dry_structs(app: &mut App, sources: &[crate::span::SourceFile]) {
    let types_modules = types_modules(sources);
    let mut parent_of = struct_parents(&app.library_classes);
    if parent_of.is_empty() {
        return;
    }
    let nested_owner = expand_nested(app, &mut parent_of, &types_modules);
    inline_attributes_from(app, &parent_of);
    let mut names: HashSet<String> =
        app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    let constants: HashMap<String, Expr> = app
        .library_classes
        .iter()
        .flat_map(|lc| {
            lc.constants
                .iter()
                .map(move |(n, v)| (format!("{}::{}", lc.name.0.as_str(), n.as_str()), v.clone()))
        })
        .collect();
    names.extend(constants.keys().cloned());

    // Read every struct's declarations and build its methods; a class
    // either part fails is refused here, before any hierarchy is lowered.
    let mut read: HashMap<String, StructClass> = HashMap::new();
    let mut built: HashMap<String, (Vec<crate::dialect::MethodDef>, Vec<(Symbol, Expr)>)> = HashMap::new();
    let mut refused: HashSet<String> = HashSet::new();
    // What each refused class names as a struct type.
    let mut refused_names: Vec<String> = Vec::new();
    for lc in &app.library_classes {
        let name = lc.name.0.as_str();
        if !parent_of.contains_key(name) {
            continue;
        }
        let scope = Scope {
            types_modules: &types_modules,
            owner: name,
            names: &names,
            structs: &parent_of,
            constants: &constants,
            depth: 0,
        };
        let s = match read_struct(lc, &scope) {
            Ok(s) => s,
            Err(reason) => {
                survey::record(&IngestError::Unsupported {
                    file: name.to_string(),
                    message: format!("Dry::Struct not lowered: {reason}"),
                });
                refused.insert(name.to_string());
                refused_names.extend(named_structs(lc, &scope));
                continue;
            }
        };
        let source = synthesized_source(name, &s, parent_of[name].is_none());
        let (parsed, diags) = crate::ingest::prism::scope(|| {
            crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_struct>")
        });
        match parsed {
            Ok(classes) if diags.is_empty() => {
                let mut methods = Vec::new();
                let mut generated = Vec::new();
                for c in classes {
                    methods.extend(c.methods);
                    generated.extend(c.constants);
                }
                built.insert(name.to_string(), (methods, generated));
            }
            Ok(_) => {
                survey::record_synthesis_failure(name.to_string(), "Dry::Struct lowering", &diags);
                refused.insert(name.to_string());
                refused_names.extend(s.structs());
            }
            Err(err) => {
                survey::record(&err);
                refused.insert(name.to_string());
                refused_names.extend(s.structs());
            }
        }
        read.insert(name.to_string(), s);
    }
    // All or nothing per hierarchy. Lowering a root makes it an ordinary
    // known class, so a refused descendant would no longer reach the
    // unknown `Dry::Struct` and its attribute calls would read as
    // missing methods rather than as the gap they are. A struct that
    // builds another struct needs that one's hierarchy lowered too, and
    // a nested struct goes with its owner: an owner kept on the gem names
    // it as a type, which it must then still be.
    let root_of = |name: &str| -> String {
        let mut cur = name.to_string();
        while let Some(Some(parent)) = parent_of.get(&cur) {
            cur = parent.clone();
        }
        cur
    };
    let mut refused_roots: HashSet<String> =
        refused.iter().chain(&refused_names).map(|n| root_of(n)).collect();
    loop {
        let before = refused_roots.len();
        for (name, s) in &read {
            if s.structs().iter().any(|dep| refused_roots.contains(&root_of(dep))) {
                refused_roots.insert(root_of(name));
            }
        }
        for (nested, owner) in &nested_owner {
            if refused_roots.contains(&root_of(owner)) {
                refused_roots.insert(root_of(nested));
            }
        }
        if refused_roots.len() == before {
            break;
        }
    }
    let lowered = |name: &str| -> bool { !refused_roots.contains(&root_of(name)) };

    let mut any = false;
    for lc in &mut app.library_classes {
        let name = lc.name.0.as_str().to_string();
        if !parent_of.contains_key(&name) || !lowered(&name) {
            continue;
        }
        let Some((mut methods, generated)) = built.remove(&name) else { continue };
        lc.unknown_calls.retain(|call| !is_struct_declaration(call));
        // Constants holding dry types stay while anything is left on the
        // gem, which may name them; they go app-wide below once nothing is.
        lc.constants.extend(generated);
        lc.origin = Some(crate::dialect::LibraryClassOrigin::DryStruct);
        methods.append(&mut lc.methods);
        lc.methods = methods;
        // The parent by the name it resolved to, not as written: `Base`
        // inside `module Shop` is `Shop::Base`, which is how every later
        // pass looks a class up.
        lc.parent = parent_of[&name].clone().map(|p| crate::ident::ClassId(Symbol::from(p.as_str())));
        any = true;
    }
    // The stand-in makes `Dry::Struct` a known class. With a struct left
    // unlowered that would end its ancestry at a class with no methods,
    // and its attribute calls would read as missing; the gem keeps it.
    if any && refused_roots.is_empty() {
        app.library_classes.extend(error_classes());
        // Nothing left needs dry-types, and the `Types` modules are not
        // emitted: a constant still building a type would fail the load.
        // The `Types` modules' own constants that build no type, an alias
        // of one (`ALIAS = Types::LABEL`) included: round by round.
        let mut plain: HashSet<String> = HashSet::new();
        loop {
            let found: Vec<String> = app
                .library_classes
                .iter()
                .filter(|lc| types_modules.iter().any(|(m, _)| m == lc.name.0.as_str()))
                .flat_map(|lc| {
                    let holder = lc.name.0.as_str();
                    lc.constants
                        .iter()
                        .map(move |(n, v)| (format!("{holder}::{}", n.as_str()), v, holder))
                })
                .filter(|(full, v, holder)| {
                    !plain.contains(full) && !holds_dry_type(v, holder, &types_modules, &names, &plain)
                })
                .map(|(full, _, _)| full)
                .collect();
            if found.is_empty() {
                break;
            }
            plain.extend(found);
        }
        for lc in &mut app.library_classes {
            let holder = lc.name.0.as_str().to_string();
            lc.constants.retain(|(_, value)| !holds_dry_type(value, &holder, &types_modules, &names, &plain));
        }
    }
}

/// Each struct class and the struct parent it inherits from (None for a
/// direct `Dry::Struct` subclass).
fn struct_parents(classes: &[LibraryClass]) -> HashMap<String, Option<String>> {
    let names: HashSet<String> = classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    loop {
        let before = parent_of.len();
        for lc in classes {
            let name = lc.name.0.as_str();
            if parent_of.contains_key(name) || lc.is_module {
                continue;
            }
            let Some(parent) = &lc.parent else { continue };
            let parent = parent.0.as_str().trim_start_matches("::");
            if parent == "Dry::Struct" {
                parent_of.insert(name.to_string(), None);
            } else if let Some(resolved) = resolve(name, parent, &names) {
                if parent_of.contains_key(&resolved) {
                    parent_of.insert(name.to_string(), Some(resolved));
                }
            }
        }
        if parent_of.len() == before {
            break;
        }
    }
    parent_of
}

/// `attribute :amount do ... end` defines `Owner::Amount`, a direct
/// `Dry::Struct` subclass that keeps the owner's key transform, and the
/// attribute takes that class; with `Types::Array` the class is singular
/// (`attribute :items, Types::Array do` defines `Owner::Item`) and the
/// attribute is an Array of it. Each nested class is a struct like any
/// other, so its own blocks nest the same way.
fn expand_nested(
    app: &mut App,
    parent_of: &mut HashMap<String, Option<String>>,
    types_modules: &[(String, BareNames)],
) -> HashMap<String, String> {
    let mut owner_of: HashMap<String, String> = HashMap::new();
    let mut queue: Vec<String> = parent_of.keys().cloned().collect();
    queue.sort();
    while let Some(owner) = queue.pop() {
        let Some(index) = app.library_classes.iter().position(|lc| lc.name.0.as_str() == owner) else {
            continue;
        };
        let transform = effective_transform(&app.library_classes, parent_of, &owner);
        let existing: HashSet<String> =
            app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
        let mut created = Vec::new();
        for call in &mut app.library_classes[index].unknown_calls {
            let ExprNode::Send { recv: None, method, args, block, .. } = &mut *call.node else { continue };
            if !matches!(method.as_str(), "attribute" | "attribute?") {
                continue;
            }
            let Some(ExprNode::Lambda { body, .. }) = block.as_ref().map(|b| &*b.node) else { continue };
            let (array, attr) = match args.as_slice() {
                [name] => (None, name),
                [name, ty] => match &*ty.node {
                    ExprNode::Const { path }
                        if types_path(path, &owner, types_modules, &existing)
                            .is_some_and(|rest| matches!(rest.as_slice(), [_, "Array"] | ["Array"])) =>
                    {
                        (Some(ty.clone()), name)
                    }
                    _ => continue,
                },
                _ => continue,
            };
            let ExprNode::Lit { value: Literal::Sym { value: attr } } = &*attr.node else { continue };
            let const_name = if array.is_some() {
                crate::naming::camelize(&crate::naming::singularize(attr.as_str()))
            } else {
                crate::naming::camelize(attr.as_str())
            };
            let full = format!("{owner}::{const_name}");
            if existing.contains(&full) || created.iter().any(|c: &LibraryClass| c.name.0.as_str() == full) {
                continue;
            }
            let mut unknown_calls: Vec<Expr> = match &*body.node {
                ExprNode::Seq { exprs } => exprs.clone(),
                _ => vec![body.clone()],
            };
            if let Some(t) = &transform {
                unknown_calls.insert(0, t.clone());
            }
            created.push(LibraryClass {
                name: crate::ident::ClassId(Symbol::from(full.as_str())),
                is_module: false,
                parent: Some(crate::ident::ClassId(Symbol::from("Dry::Struct"))),
                parent_span: crate::span::Span::synthetic(),
                includes: Vec::new(),
                methods: Vec::new(),
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                unknown_calls,
                class_ivar_initializers: Vec::new(),
            });
            let span = call.span;
            let class_ref = Expr::new(
                span,
                ExprNode::Const {
                    path: std::iter::once(Symbol::from(""))
                        .chain(full.split("::").map(Symbol::from))
                        .collect(),
                },
            );
            let ty = match array {
                Some(array) => Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(array),
                        method: Symbol::from("of"),
                        args: vec![class_ref],
                        block: None,
                        parenthesized: true,
                    },
                ),
                None => class_ref,
            };
            *args = vec![args[0].clone(), ty];
            *block = None;
        }
        for c in created {
            let name = c.name.0.as_str().to_string();
            parent_of.insert(name.clone(), None);
            owner_of.insert(name.clone(), owner.clone());
            queue.push(name);
            app.library_classes.push(c);
        }
    }
    owner_of
}

/// `attributes_from X` declares `X`'s attributes, inherited ones first,
/// in its place. Inlined when every constant they name is spelled from
/// the top (`::...`), so it reads the same from the new class; otherwise
/// it stays, and the class is refused for it.
fn inline_attributes_from(app: &mut App, parent_of: &HashMap<String, Option<String>>) {
    let names: HashSet<String> = app.library_classes.iter().map(|lc| lc.name.0.as_str().to_string()).collect();
    // Sources first, round by round; a cycle is left to refuse.
    loop {
        let calls_of = |name: &str| -> Option<Vec<Expr>> {
            let mut chain = Vec::new();
            let mut cur = Some(name.to_string());
            while let Some(n) = cur {
                chain.push(n.clone());
                cur = parent_of.get(&n).cloned().flatten();
            }
            let mut out = Vec::new();
            for n in chain.iter().rev() {
                let lc = app.library_classes.iter().find(|lc| lc.name.0.as_str() == n)?;
                for call in &lc.unknown_calls {
                    let ExprNode::Send { recv: None, method, .. } = &*call.node else { continue };
                    match method.as_str() {
                        "attribute" | "attribute?" => out.push(call.clone()),
                        // Not inlined yet: wait for it, so its keys come along.
                        "attributes_from" => return None,
                        _ => {}
                    }
                }
            }
            out.iter().all(absolute_constants).then_some(out)
        };
        let mut rewrites: Vec<(usize, usize, Vec<Expr>)> = Vec::new();
        for (ci, lc) in app.library_classes.iter().enumerate() {
            let owner = lc.name.0.as_str();
            if !parent_of.contains_key(owner) {
                continue;
            }
            for (ui, call) in lc.unknown_calls.iter().enumerate() {
                let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else { continue };
                if method.as_str() != "attributes_from" {
                    continue;
                }
                let [ExprNode::Const { path }] = args.iter().map(|a| &*a.node).collect::<Vec<_>>()[..] else { continue };
                let written = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                let Some(source) = resolve_in(owner, &written, &names).filter(|n| parent_of.contains_key(n)) else {
                    continue;
                };
                if let Some(calls) = calls_of(&source) {
                    rewrites.push((ci, ui, calls));
                }
            }
        }
        if rewrites.is_empty() {
            break;
        }
        for (ci, ui, calls) in rewrites.into_iter().rev() {
            app.library_classes[ci].unknown_calls.splice(ui..=ui, calls);
        }
    }
}

/// Every constant in `expr` is spelled from the top level.
fn absolute_constants(expr: &Expr) -> bool {
    let own = match &*expr.node {
        ExprNode::Const { path } => path.first().is_some_and(|s| s.as_str().is_empty()),
        _ => true,
    };
    let mut ok = own;
    expr.node.for_each_child(&mut |c| ok &= absolute_constants(c));
    ok
}

/// The `transform_keys` call that applies to `name`: its own, or its
/// nearest struct ancestor's.
fn effective_transform(
    classes: &[LibraryClass],
    parent_of: &HashMap<String, Option<String>>,
    name: &str,
) -> Option<Expr> {
    let mut cur = Some(name.to_string());
    while let Some(n) = cur {
        let own = classes.iter().find(|lc| lc.name.0.as_str() == n).and_then(|lc| {
            lc.unknown_calls.iter().rev().find(|call| {
                matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "transform_keys")
            })
        });
        if let Some(call) = own {
            return Some(call.clone());
        }
        cur = parent_of.get(&n).cloned().flatten();
    }
    None
}

/// `Dry::Struct::Error`, which the lowered constructors raise and an
/// application may rescue. Nothing else of dry-struct is left.
fn error_classes() -> Vec<LibraryClass> {
    let source = "module Dry\n  class Struct\n    class Error < TypeError\n    end\n  end\nend\n";
    crate::ingest::ingest_library_classes(source.as_bytes(), "<dry_struct>")
        .expect("the Dry::Struct::Error stand-in parses")
}

/// A `Types` module and what its bare names (`Types::String`) mean:
/// `Dry.Types()` makes them strict, `Dry.Types(default: :nominal)`
/// nominal. Any other arguments leave them unread.
#[derive(Clone, Copy, PartialEq)]
enum BareNames {
    Strict,
    Nominal,
    Unknown,
}

/// Modules that `include Dry.Types()`: the `Types::` namespace. Read
/// from the source, since a module holding nothing else ingests to no
/// class at all.
fn types_modules(sources: &[crate::span::SourceFile]) -> Vec<(String, BareNames)> {
    struct Finder {
        nesting: Vec<String>,
        found: Vec<(String, BareNames)>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Finder {
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            let name = String::from_utf8_lossy(node.constant_path().location().as_slice());
            self.nesting.push(name.trim_start_matches("::").to_string());
            ruby_prism::visit_module_node(self, node);
            self.nesting.pop();
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let dry_types = (node.receiver().is_none() && node.name().as_slice() == b"include")
                .then(|| node.arguments())
                .flatten()
                .and_then(|args| {
                    args.arguments().iter().find_map(|arg| {
                        let call = arg.as_call_node()?;
                        let text = call.receiver()?.location().as_slice().to_vec();
                        (call.name().as_slice() == b"Types" && (text == b"Dry" || text == b"::Dry"))
                            .then_some(call)
                    })
                });
            if let Some(call) = dry_types
                && !self.nesting.is_empty()
            {
                let args = call
                    .arguments()
                    .map(|a| String::from_utf8_lossy(a.location().as_slice()).replace(' ', ""))
                    .unwrap_or_default();
                let bare = match args.as_str() {
                    "" | "default::strict" => BareNames::Strict,
                    "default::nominal" => BareNames::Nominal,
                    _ => BareNames::Unknown,
                };
                self.found.push((self.nesting.join("::"), bare));
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let mut finder = Finder { nesting: Vec::new(), found: Vec::new() };
    for source in sources.iter().filter(|s| s.text.contains("Dry.Types")) {
        let parsed = ruby_prism::parse(source.text.as_bytes());
        ruby_prism::Visit::visit(&mut finder, &parsed.node());
    }
    finder.found
}

/// `name` as Ruby resolves it in `owner`'s body: `owner` itself first,
/// then outward.
fn resolve_in(owner: &str, name: &str, names: &HashSet<String>) -> Option<String> {
    let own = format!("{owner}::{name}");
    if names.contains(&own) { Some(own) } else { resolve(owner, name, names) }
}

/// `parent` as Ruby resolves it from inside `owner`: the innermost
/// enclosing namespace that defines it, then the top level.
fn resolve(owner: &str, parent: &str, names: &HashSet<String>) -> Option<String> {
    let mut scope: Vec<&str> = owner.split("::").collect();
    scope.pop();
    while !scope.is_empty() {
        let candidate = format!("{}::{parent}", scope.join("::"));
        if names.contains(&candidate) {
            return Some(candidate);
        }
        scope.pop();
    }
    names.contains(parent).then(|| parent.to_string())
}

fn is_struct_declaration(call: &Expr) -> bool {
    matches!(&*call.node, ExprNode::Send { recv: None, method, .. }
        if matches!(method.as_str(), "attribute" | "attribute?" | "transform_keys"))
}

/// `Dry::Struct` class methods the lowering does not model.
const DRY_STRUCT_DSL: &[&str] = &[
    "attributes",
    "attributes_from",
    "transform_types",
    "schema",
    "abstract",
    "input",
    "constructor_type",
    "load",
];

/// The struct classes a declaration names as types, read loosely: every
/// constant anywhere in its `attribute` arguments that resolves to one.
/// Used for a refused class, which the gem keeps, so whatever it names
/// as a struct type must stay a `Dry::Struct` too.
fn named_structs(lc: &LibraryClass, scope: &Scope<'_>) -> Vec<String> {
    fn walk(expr: &Expr, scope: &Scope<'_>, out: &mut Vec<String>) {
        if let ExprNode::Const { path } = &*expr.node {
            let written: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
            let name = written.iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("::");
            let full = if written.first() == Some(&"") {
                scope.names.contains(&name).then_some(name)
            } else {
                resolve_in(scope.owner, &name, scope.names)
            };
            match full {
                Some(full) if scope.structs.contains_key(&full) => out.push(full),
                // A constant holding a type names what its value names.
                Some(full) if scope.depth <= 8 => {
                    if let Some(value) = scope.constants.get(&full) {
                        let holder = full.rsplit_once("::").map_or("", |(h, _)| h);
                        walk(value, &Scope { owner: holder, depth: scope.depth + 1, ..*scope }, out);
                    }
                }
                _ => {}
            }
        }
        expr.node.for_each_child(&mut |c| walk(c, scope, out));
    }
    let mut out = Vec::new();
    for call in &lc.unknown_calls {
        if let ExprNode::Send { recv: None, method, args, .. } = &*call.node
            && matches!(method.as_str(), "attribute" | "attribute?")
        {
            for arg in args {
                walk(arg, scope, &mut out);
            }
        }
    }
    out
}

fn read_struct(lc: &LibraryClass, scope: &Scope<'_>) -> Result<StructClass, String> {
    let mut attributes = Vec::new();
    let mut transform_keys = None;
    for call in &lc.unknown_calls {
        let ExprNode::Send { recv: None, method, args, block, .. } = &*call.node else { continue };
        match method.as_str() {
            "transform_keys" => {
                if !args.is_empty() || block.is_none() {
                    return Err("transform_keys without a block".into());
                }
                transform_keys = Some(call.clone());
            }
            "attribute" | "attribute?" => {
                if block.is_some() {
                    return Err("a nested `attribute ... do` struct".into());
                }
                let [name, ty] = args.as_slice() else {
                    return Err("an attribute with options".into());
                };
                let ExprNode::Lit { value: Literal::Sym { value: name } } = &*name.node else {
                    return Err("an attribute named by an expression".into());
                };
                let ty = dry_type(ty, scope).ok_or_else(|| {
                    format!("attribute `{}` type `{}`", name.as_str(), crate::emit::ruby::emit_expr(ty))
                })?;
                // `attribute :name?` is dry-struct's other spelling of
                // `attribute? :name`.
                let (name, suffixed) = match name.as_str().strip_suffix('?') {
                    Some(bare) => (Symbol::from(bare), true),
                    None => (name.clone(), false),
                };
                attributes.push(Attribute { name, omittable: suffixed || method.as_str() == "attribute?", ty });
            }
            // The rest of dry-struct's class DSL changes the schema or the
            // constructor; dropping it would lower a different struct.
            other if DRY_STRUCT_DSL.contains(&other) => {
                return Err(format!("`{other}` in the class body"));
            }
            _ => {}
        }
    }
    Ok(StructClass { attributes, transform_keys })
}

/// The type a `Types::...` expression names, when it is one modeled here.
fn dry_type(expr: &Expr, scope: &Scope<'_>) -> Option<DryType> {
    let plain = |base| Some(DryType { base, optional: false, default: None, enumeration: None, omittable: false });
    match &*expr.node {
        ExprNode::Const { path } => {
            let Some(rest) = types_path(path, scope.owner, scope.types_modules, scope.names) else {
                // Another struct class, or a constant holding a type, as
                // Ruby resolves the name here.
                let written = path.iter().map(|s| s.as_str()).collect::<Vec<_>>();
                let name = written.iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("::");
                let full = if written.first() == Some(&"") {
                    scope.names.contains(&name).then_some(name)
                } else {
                    resolve_in(scope.owner, &name, scope.names)
                }?;
                if scope.structs.contains_key(&full) {
                    return plain(Base::Struct(full));
                }
                let value = scope.constants.get(&full)?;
                if scope.depth > 8 {
                    return None;
                }
                let holder = full.rsplit_once("::").map_or("", |(h, _)| h);
                return dry_type(value, &Scope { owner: holder, depth: scope.depth + 1, ..*scope });
            };
            let base = match rest.as_slice() {
                ["Coercible", "String"] => Base::Coerce("String"),
                ["Coercible", "Integer"] => Base::Coerce("Integer"),
                ["Coercible", "Float"] => Base::Coerce("Float"),
                ["Coercible", "Hash"] => Base::Coerce("Hash"),
                ["Coercible", "Symbol"] => Base::CoerceSymbol,
                ["Params", "Integer"] => Base::ParamsInteger,
                ["Params", "Bool"] => Base::ParamsBool,
                ["Params", "Decimal"] => Base::ParamsDecimal,
                ["Coercible", "Decimal"] => Base::Coerce("BigDecimal"),
                ["JSON", "Hash"] => Base::Strict("Hash"),
                ["JSON", "Array"] => Base::Strict("Array"),
                ["Params" | "JSON", "Date"] => Base::Parse("Date"),
                ["Params" | "JSON", "DateTime"] => Base::Parse("DateTime"),
                ["Params" | "JSON", "Time"] => Base::Parse("Time"),
                ["Strict", t @ ("String" | "Integer" | "Float" | "Hash" | "Array" | "Symbol")] => {
                    Base::Strict(match *t {
                        "String" => "String",
                        "Integer" => "Integer",
                        "Float" => "Float",
                        "Hash" => "Hash",
                        "Symbol" => "Symbol",
                        _ => "Array",
                    })
                }
                ["Strict", "Bool"] => Base::StrictBool,
                ["Strict", "Time"] => Base::Instance("::Time".into()),
                ["Strict", "DateTime"] => Base::Instance("::DateTime".into()),
                ["Strict", "Date"] => Base::Instance("::Date".into()),
                ["Any"] => Base::Nominal,
                ["Nominal", t]
                    if matches!(*t, "String" | "Integer" | "Float" | "Bool" | "Hash" | "Array" | "Any" | "Symbol") =>
                {
                    Base::Nominal
                }
                // A constant of the app's own in a `Types` module.
                _ => {
                    let name = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::");
                    let full = resolve_in(scope.owner, &name, scope.names)?;
                    let value = scope.constants.get(&full)?;
                    if scope.depth > 8 {
                        return None;
                    }
                    let holder = full.rsplit_once("::").map_or("", |(h, _)| h);
                    return dry_type(value, &Scope { owner: holder, depth: scope.depth + 1, ..*scope });
                }
            };
            plain(base)
        }
        // `.default { ... }`: dry-types calls the block whenever the key
        // is absent, which is where the constructor evaluates its body.
        // Only a body that reads no `self`: in the class body the block's
        // self is the class, in the constructor it is the instance.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "default" && args.is_empty() =>
        {
            let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
            if !params.is_empty() || reads_self(body) {
                return None;
            }
            let mut ty = dry_type(recv, scope)?;
            ty.default = Some(DefaultValue::Block(body.clone()));
            Some(ty)
        }
        // `Types.Constructor(K) { |v| ... }`, `T.constructor { |v| ... }`.
        // The block runs as a class method of the struct, so only one that
        // reads no `self`.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if matches!(method.as_str(), "Constructor" | "constructor") =>
        {
            let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
            let [param] = params.as_slice() else { return None };
            if reads_self(body) {
                return None;
            }
            let then = if method.as_str() == "Constructor" {
                let ExprNode::Const { path } = &*recv.node else { return None };
                if !types_path(path, scope.owner, scope.types_modules, scope.names)?.is_empty() || args.len() != 1 {
                    return None;
                }
                DryType { base: Base::Nominal, optional: false, default: None, enumeration: None, omittable: false }
            } else {
                if !args.is_empty() {
                    return None;
                }
                dry_type(recv, scope)?
            };
            plain(Base::Constructor { param: param.clone(), body: next_to_return(body), then: Box::new(then) })
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } => match method.as_str() {
            "optional" if args.is_empty() => {
                let mut ty = dry_type(recv, scope)?;
                ty.optional = true;
                Some(ty)
            }
            // Evaluated once where written, as dry-types does; `shared:`
            // only silences its warning about exactly that.
            "default" => {
                let value = match args.as_slice() {
                    [value] => value,
                    [value, options] if shared_option(options) => value,
                    _ => return None,
                };
                let mut ty = dry_type(recv, scope)?;
                ty.default = Some(DefaultValue::Value(value.clone()));
                Some(ty)
            }
            "|" => {
                let [right] = args.as_slice() else { return None };
                let mut members = Vec::new();
                for side in [recv, right] {
                    let ty = dry_type(side, scope)?;
                    if ty.default.is_some() {
                        return None;
                    }
                    match ty.base {
                        Base::Sum(inner) if !ty.optional && ty.enumeration.is_none() => members.extend(inner),
                        _ => members.push(ty),
                    }
                }
                plain(Base::Sum(members))
            }
            // Metadata, but `omittable: true` makes a schema key optional.
            "meta" => {
                let mut ty = dry_type(recv, scope)?;
                if let [ExprNode::Hash { entries, .. }] = args.iter().map(|a| &*a.node).collect::<Vec<_>>()[..] {
                    ty.omittable |= entries.iter().any(|(k, v)| {
                        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "omittable")
                            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
                    });
                }
                Some(ty)
            }
            "schema" => {
                let [ExprNode::Hash { entries, .. }] = args.iter().map(|a| &*a.node).collect::<Vec<_>>()[..]
                else {
                    return None;
                };
                let ExprNode::Const { path } = &*recv.node else { return None };
                if !matches!(types_path(path, scope.owner, scope.types_modules, scope.names)?.as_slice(), ["Strict" | "Nominal", "Hash"]) {
                    return None;
                }
                let mut keys = Vec::new();
                for (key, ty) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*key.node else { return None };
                    let (name, omittable) = match key.as_str().strip_suffix('?') {
                        Some(name) => (name.to_string(), true),
                        None => (key.as_str().to_string(), false),
                    };
                    let ty = dry_type(ty, scope)?;
                    keys.push((name, omittable || ty.omittable, ty));
                }
                plain(Base::HashSchema(keys))
            }
            "enum" if !args.is_empty() => {
                // Literal values, or a splatted constant list.
                // A Hash makes a mapping enum, which this does not model.
                if !args.iter().all(|a| {
                    (literal(a) && !matches!(&*a.node, ExprNode::Hash { .. }))
                        || matches!(&*a.node, ExprNode::Splat { value }
                            if matches!(&*value.node, ExprNode::Const { .. }))
                }) {
                    return None;
                }
                let mut ty = dry_type(recv, scope)?;
                if ty.enumeration.is_some() {
                    return None;
                }
                ty.enumeration = Some(args.clone());
                Some(ty)
            }
            "of" => {
                let [member] = args.as_slice() else { return None };
                let ExprNode::Const { path } = &*recv.node else { return None };
                let strict = match types_path(path, scope.owner, scope.types_modules, scope.names)?.as_slice() {
                    ["Nominal", "Array"] => false,
                    ["Strict", "Array"] => true,
                    _ => return None,
                };
                let member = dry_type(member, scope)?;
                plain(Base::ArrayOf { strict, member: Box::new(member) })
            }
            // `Types.Instance(X)`, `Types::Instance(X)`.
            "Instance" => {
                let [class] = args.as_slice() else { return None };
                let ExprNode::Const { path } = &*recv.node else { return None };
                if !types_path(path, scope.owner, scope.types_modules, scope.names)?.is_empty() {
                    return None;
                }
                matches!(&*class.node, ExprNode::Const { .. })
                    .then(|| plain(Base::Instance(crate::emit::ruby::emit_expr(class))))
                    .flatten()
            }
            _ => None,
        },
        _ => None,
    }
}

/// The part of `path` after a `Types` module: `Coercible::String` from
/// `::Randewoo::Types::Coercible::String`. The `...::Types` prefix has to
/// name a module that includes `Dry.Types()` as Ruby resolves it from
/// `owner`: spelled from the top, exactly; otherwise in `owner`, then
/// outward, and the first module found decides. An app's own
/// `Billing::Types` is not dry-types because some other `Types` is.
///
/// A bare name is given the namespace the module defaults it to:
/// `Types::String` is `Strict::String` under `Dry.Types()`. `Any` is
/// nominal whatever the default.
fn types_path<'a>(
    path: &'a [Symbol],
    owner: &str,
    types_modules: &[(String, BareNames)],
    names: &HashSet<String>,
) -> Option<Vec<&'a str>> {
    let (_, bare, rest) = types_module_of(path, owner, types_modules, names)?;
    Some(match (rest.as_slice(), bare) {
        ([name], _) if *name == "Any" => rest,
        ([name], BareNames::Strict) => vec!["Strict", name],
        ([name], BareNames::Nominal) => vec!["Nominal", name],
        ([name], BareNames::Unknown) => vec!["?", name],
        _ => rest,
    })
}

/// The `Types` module `path` reads, its bare names, and the segments
/// after it as written.
fn types_module_of<'a>(
    path: &'a [Symbol],
    owner: &str,
    types_modules: &[(String, BareNames)],
    names: &HashSet<String>,
) -> Option<(String, BareNames, Vec<&'a str>)> {
    let absolute = path.first().is_some_and(|s| s.as_str().is_empty());
    let segments: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    let at = segments.iter().rposition(|s| *s == "Types")?;
    let prefix = segments[..=at].join("::");
    let find = |full: &str| types_modules.iter().find(|(m, _)| m == full);
    let (module, bare) = if absolute {
        find(&prefix)?
    } else {
        let mut scope: Vec<&str> = owner.split("::").filter(|s| !s.is_empty()).collect();
        loop {
            let candidate =
                if scope.is_empty() { prefix.clone() } else { format!("{}::{prefix}", scope.join("::")) };
            if let Some(found) = find(&candidate) {
                // Past the lexical scopes Ruby searches the owner's
                // ancestors before the top level, and any `X::Types` off
                // that path may be one of them. Only a different meaning
                // for bare names matters; that is refused, not guessed.
                // ponytail: no ancestor walk here; read the ancestry if a
                // real app nests `Types` modules with differing defaults.
                if scope.is_empty()
                    && types_modules.iter().any(|(m, b)| {
                        *b != found.1 && m.ends_with(&format!("::{prefix}"))
                    })
                {
                    return None;
                }
                break found;
            }
            if names.contains(&candidate) || scope.is_empty() {
                return None;
            }
            scope.pop();
        }
    };
    Some((module.clone(), *bare, segments[at + 1..].to_vec()))
}

/// A block body as a method body: its own `next` becomes `return`. A
/// `next` in a block nested inside it belongs to that block and stays.
fn next_to_return(expr: &Expr) -> Expr {
    fn walk(expr: &mut Expr) {
        if matches!(&*expr.node, ExprNode::Lambda { .. }) {
            return;
        }
        if let ExprNode::Next { value } = &*expr.node {
            let value = value
                .clone()
                .unwrap_or_else(|| Expr::new(expr.span, ExprNode::Lit { value: Literal::Nil }));
            *expr.node = ExprNode::Return { value };
        }
        expr.node.for_each_child_mut(&mut |c| walk(c));
    }
    let mut body = expr.clone();
    walk(&mut body);
    body
}

/// Whether `expr` builds a dry type: it reads a `Types` module, other
/// than a `plain` constant declared there (`Types::LABEL = "x"`).
fn holds_dry_type(
    expr: &Expr,
    holder: &str,
    types_modules: &[(String, BareNames)],
    names: &HashSet<String>,
    plain: &HashSet<String>,
) -> bool {
    let own = matches!(&*expr.node, ExprNode::Const { path }
        if types_module_of(path, holder, types_modules, names)
            .is_some_and(|(module, _, rest)| !plain.contains(&format!("{module}::{}", rest.join("::")))));
    let mut found = own;
    expr.node.for_each_child(&mut |c| found |= holds_dry_type(c, holder, types_modules, names, plain));
    found
}

/// `shared: true`, the only `.default` option.
fn shared_option(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Hash { entries, .. } if entries.len() == 1 && entries.iter().all(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "shared")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { .. } })
    }))
}

/// Whether `expr` reads the receiver it runs on: a bare call, an ivar,
/// `self`. Kernel's conversion functions are bare calls that read none.
fn reads_self(expr: &Expr) -> bool {
    let own = match &*expr.node {
        ExprNode::Send { recv: None, method, .. } => {
            !matches!(method.as_str(), "Array" | "Integer" | "Float" | "String" | "Hash" | "BigDecimal")
        }
        ExprNode::Ivar { .. } | ExprNode::SelfRef => true,
        _ => false,
    };
    let mut found = own;
    expr.node.for_each_child(&mut |c| found |= reads_self(c));
    found
}

/// A default the constructor can carry as written: a literal or a
/// constant, no call but `.freeze`, no block.
fn literal(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } | ExprNode::Const { .. } => true,
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if method.as_str() == "freeze" && args.is_empty() =>
        {
            literal(recv)
        }
        ExprNode::Array { elements, .. } => elements.iter().all(literal),
        ExprNode::Hash { entries, .. } => entries.iter().all(|(k, v)| literal(k) && literal(v)),
        _ => false,
    }
}

/// What the generated class carries besides its constructor: a constant
/// per shared default, a class method per constructor block.
#[derive(Default)]
struct Gen {
    /// The struct class being generated, by full name.
    owner: String,
    constants: Vec<String>,
    helpers: Vec<String>,
    /// Locals handed out so far; each holds one value, of one type.
    temps: usize,
}

impl Gen {
    /// The expression a missing key takes: the shared value, through a
    /// constant unless it already is one, or the block's body.
    fn missing_value(&mut self, default: &DefaultValue) -> String {
        match default {
            DefaultValue::Value(value) if matches!(&*value.node, ExprNode::Const { .. }) => {
                crate::emit::ruby::emit_expr(value)
            }
            DefaultValue::Value(value) => {
                let name = format!("DRY_STRUCT_DEFAULT_{}", self.constants.len());
                self.constants.push(format!("{name} = {}", crate::emit::ruby::emit_expr(value)));
                name
            }
            DefaultValue::Block(body) => format!("(begin\n{}\nend)", crate::emit::ruby::emit_expr(body)),
        }
    }
}

/// `value` coerced or checked as `ty` says, raising `Dry::Struct::Error`.
fn coerced(extra: &mut Gen, ty: &DryType, value: &str, key: &str) -> String {
    let fail = format!("raise(Dry::Struct::Error, \"[#{{self.class}}.new] {key} has an invalid value\")");
    let inner = match &ty.base {
        Base::Coerce(kernel) => {
            format!("(begin\n  {kernel}({value})\nrescue ArgumentError, TypeError\n  {fail}\nend)")
        }
        Base::Strict(class) => format!("({value}.is_a?({class}) ? {value} : {fail})"),
        Base::StrictBool => format!("({value} == true || {value} == false ? {value} : {fail})"),
        Base::HashSchema(keys) => {
            let mut required = Vec::new();
            let mut optional = String::new();
            for (k, omittable, t) in keys {
                let read = format!("{value}[:{k}]");
                let each = coerced(extra, t, &read, &format!(":{k}"));
                match (&t.default, omittable) {
                    (Some(default), _) => {
                        let missing = extra.missing_value(default);
                        required.push(format!("{k}: ({value}.key?(:{k}) ? {each} : {missing})"));
                    }
                    (None, true) => {
                        optional.push_str(&format!(".merge({value}.key?(:{k}) ? {{ {k}: {each} }} : {{}})"));
                    }
                    (None, false) => required.push(format!("{k}: ({value}.key?(:{k}) ? {each} : {fail})")),
                }
            }
            format!("({value}.is_a?(Hash) ? {{ {} }}{optional} : {fail})", required.join(", "))
        }
        Base::ParamsInteger => format!(
            "(begin\n  {value}.is_a?(String) ? Integer({value}, 10) : Integer({value})\nrescue ArgumentError, TypeError\n  {fail}\nend)"
        ),
        Base::ParamsBool => format!(
            "(%w[1 on On ON t true True TRUE T y yes Yes YES Y].include?({value}.to_s) ? true : (%w[0 off Off OFF f false False FALSE F n no No NO N].include?({value}.to_s) ? false : {fail}))"
        ),
        // `to_d` as bigdecimal/util defines it, which the tree does not load.
        Base::ParamsDecimal => format!(
            "(begin\n  Float({value})\n  case {value}\n  when Float then BigDecimal({value}, 0)\n  when String then BigDecimal.interpret_loosely({value})\n  when BigDecimal then {value}\n  else BigDecimal({value})\n  end\nrescue ArgumentError, TypeError\n  {fail}\nend)"
        ),
        Base::Parse(class) => format!(
            "({value}.respond_to?(:to_str) ? (begin\n  ::{class}.parse({value})\nrescue ArgumentError, RangeError\n  {fail}\nend) : ({value}.is_a?(::{class}) ? {value} : {fail}))"
        ),
        Base::CoerceSymbol => {
            format!("(begin\n  {value}.to_sym\nrescue NoMethodError\n  {fail}\nend)")
        }
        Base::Instance(class) => format!("({value}.is_a?({class}) ? {value} : {fail})"),
        Base::Struct(class) => format!(
            "({value}.is_a?(::{class}) ? {value} : ({value}.is_a?(Hash) ? ::{class}.new({value}) : {fail}))"
        ),
        Base::Nominal => value.to_string(),
        Base::ArrayOf { strict, member } => {
            let each = coerced(extra, member, "member", key);
            let mapped = format!("{value}.map {{ |member| {each} }}");
            if *strict {
                format!("({value}.is_a?(Array) ? {mapped} : {fail})")
            } else {
                mapped
            }
        }
        // Each alternative in turn; a refusal moves on to the next.
        Base::Sum(types) => {
            let mut alternatives = types.iter().map(|t| coerced(extra, t, value, key)).collect::<Vec<_>>();
            let last = alternatives.pop().unwrap_or_else(|| fail.clone());
            alternatives.into_iter().rev().fold(last, |rest, first| {
                format!("(begin\n  {first}\nrescue Dry::Struct::Error\n  {rest}\nend)")
            })
        }
        // The block as a class method of its own, where its `next` (now
        // `return`) ends only the block; a second checks its value. Called
        // on the class that defines them: a subclass's helpers of the same
        // name must not answer for a parent's attribute.
        Base::Constructor { param, body, then } => {
            let slot = extra.helpers.len();
            let name = format!("dry_struct_constructor_{slot}");
            extra.helpers.push(String::new());
            let checked = coerced(extra, then, "dry_struct_value", key);
            extra.helpers[slot] = format!(
                "  def self.{name}_block({param})\n{}\n  end\n\n  def self.{name}(value)\n    dry_struct_value = {name}_block(value)\n    {checked}\n  end\n",
                crate::emit::ruby::emit_expr(body),
                param = param.as_str(),
            );
            format!("::{}.{name}({value})", extra.owner)
        }
    };
    // The coerced value once, then checked and returned.
    let inner = match &ty.enumeration {
        Some(values) => {
            let list = values.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join(", ");
            let temp = format!("dry_struct_enum_{}", extra.temps);
            extra.temps += 1;
            format!("({temp} = {inner}\n[{list}].include?({temp}) ? {temp} : {fail})")
        }
        None => inner,
    };
    if ty.optional {
        format!("({value}.nil? ? nil : ({inner}))")
    } else {
        inner
    }
}

fn synthesized_source(owner: &str, s: &StructClass, is_root: bool) -> String {
    let mut extra = Gen { owner: owner.to_string(), ..Gen::default() };
    let mut body = String::new();
    if is_root || s.transform_keys.is_some() {
        let keys = match &s.transform_keys {
            // The declared call, sent to the attributes Hash instead.
            Some(call) => {
                let mut call = call.clone();
                if let ExprNode::Send { recv, .. } = &mut *call.node {
                    *recv = Some(Expr::new(
                        call.span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("attributes") },
                    ));
                }
                crate::emit::ruby::emit_expr(&call)
            }
            None => "attributes".to_string(),
        };
        body.push_str(&format!("  def self.dry_struct_keys(attributes)\n    {keys}\n  end\n\n"));
    }
    if is_root {
        body.push_str(
            "  def initialize(attributes = {})\n    dry_struct_assign(self.class.dry_struct_keys(attributes))\n  end\n\n",
        );
    }
    for a in &s.attributes {
        body.push_str(&format!("  def {0}\n    @{0}\n  end\n\n", a.name.as_str()));
    }
    let mut assign_body = String::new();
    if !is_root {
        assign_body.push_str("    super(attributes)\n");
    }
    for a in &s.attributes {
        let key = a.name.as_str();
        let value = format!("attributes[:{key}]");
        let assign = coerced(&mut extra, &a.ty, &value, &format!(":{key}"));
        let missing = match (&a.ty.default, a.omittable) {
            (Some(default), _) => extra.missing_value(default),
            (None, true) => "nil".to_string(),
            (None, false) => format!(
                "raise(Dry::Struct::Error, \"[#{{self.class}}.new] :{key} is missing in Hash input\")"
            ),
        };
        assign_body.push_str(&format!(
            "    @{key} = if attributes.key?(:{key})\n      {assign}\n    else\n      {missing}\n    end\n"
        ));
    }
    let mut out = String::new();
    let name = owner.rsplit("::").next().unwrap_or(owner);
    out.push_str(&format!("class {name}\n"));
    for constant in &extra.constants {
        out.push_str(&format!("  {constant}\n"));
    }
    if !extra.constants.is_empty() {
        out.push('\n');
    }
    for helper in &extra.helpers {
        out.push_str(helper);
        out.push('\n');
    }
    out.push_str(&body);
    out.push_str("  private\n\n  def dry_struct_assign(attributes)\n");
    out.push_str(&assign_body);
    out.push_str("    self\n  end\nend\n");
    out
}
