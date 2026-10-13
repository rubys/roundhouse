//! dry-types, read: what a `Types::...` expression means, and the Ruby
//! that coerces or checks a value as it says. Shared by the lowerings of
//! the gems that take dry types (`ingest::dry_struct`).
//!
//! Types mean what dry-types makes them, checked against the gem: a bare
//! name is strict under `Dry.Types()`; `Coercible::`, `Strict::`,
//! `Params::` and `JSON::` scalars, dates and decimals; `Array.of`,
//! `Hash.schema`, `Instance(X)`, a struct class, `A | B`,
//! `Constructor(K) { }` and `.constructor { }`, `.optional`, `.enum`,
//! `.constrained`, `.fallback`, `.meta`, `.default`; a constant holding
//! any of these.

use std::collections::{HashMap, HashSet};

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// What a `Types::...` expression checks or coerces.
#[derive(Clone, Debug)]
pub(super) enum Base {
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
    /// A class where a type belongs, as `Array.of(Invoice)`: dry-types
    /// calls `call_unsafe` on it, which a class does not answer.
    Uncallable(String),
}

/// A `.default`: a value dry-types evaluates once, where the type is
/// written, and hands out every time; or a block it calls each time.
#[derive(Clone, Debug)]
pub(super) enum DefaultValue {
    Value(Expr),
    Block(Expr),
}

impl DryType {
    /// The struct classes this type builds, which must be lowered too.
    pub(super) fn structs(&self, out: &mut Vec<String>) {
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
pub(super) struct Scope<'a> {
    pub(super) types_modules: &'a [(String, BareNames)],
    pub(super) owner: &'a str,
    pub(super) names: &'a HashSet<String>,
    pub(super) structs: &'a HashMap<String, Option<String>>,
    /// Every class-body constant, by full name, with its value.
    pub(super) constants: &'a HashMap<String, Expr>,
    /// The app's models and other classes, by full name: what an
    /// `Array.of(X)` names when `X` is not a type.
    pub(super) classes: &'a HashSet<String>,
    /// Constants followed so far, against a cycle.
    pub(super) depth: usize,
}

#[derive(Clone, Debug)]
pub(super) struct DryType {
    pub(super) base: Base,
    pub(super) optional: bool,
    pub(super) default: Option<DefaultValue>,
    /// `.enum(...)`: the values the coerced value must be one of.
    pub(super) enumeration: Option<Vec<Expr>>,
    /// `.constrained(gt: 0, ...)`: dry-logic predicates and their
    /// arguments, checked on the coerced value.
    pub(super) constraints: Vec<(Symbol, Expr)>,
    /// `.fallback(v)`, `.fallback { |input| ... }`: what any failure of
    /// the type gives instead.
    pub(super) fallback: Option<Fallback>,
    /// `.meta(omittable: true)`: in a `Hash.schema`, the key may be absent.
    pub(super) omittable: bool,
}

impl DryType {
    pub(super) fn of(base: Base) -> DryType {
        DryType {
            base,
            optional: false,
            default: None,
            enumeration: None,
            constraints: Vec::new(),
            fallback: None,
            omittable: false,
        }
    }
}

/// A `.fallback`: a value evaluated once where written, or a block
/// called with the input.
#[derive(Clone, Debug)]
pub(super) enum Fallback {
    Value(Expr),
    Block { param: Option<Symbol>, body: Expr },
}

/// The dry-logic predicates `.constrained` is read with: unary ones take
/// `true`.
fn predicate(name: &str) -> Option<bool> {
    match name {
        "gt" | "gteq" | "lt" | "lteq" | "min_size" | "max_size" | "size" | "format" | "included_in"
        | "excluded_from" | "eql" => Some(false),
        "filled" => Some(true),
        _ => None,
    }
}

/// A `Types` module and what its bare names (`Types::String`) mean:
/// `Dry.Types()` makes them strict, `Dry.Types(default: :nominal)`
/// nominal. Any other arguments leave them unread.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum BareNames {
    Strict,
    Nominal,
    Unknown,
}

/// Modules that `include Dry.Types()`: the `Types::` namespace. Read
/// from the source, since a module holding nothing else ingests to no
/// class at all.
pub(super) fn types_modules(sources: &[crate::span::SourceFile]) -> Vec<(String, BareNames)> {
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
pub(super) fn resolve_in(owner: &str, name: &str, names: &HashSet<String>) -> Option<String> {
    let own = format!("{owner}::{name}");
    if names.contains(&own) { Some(own) } else { resolve(owner, name, names) }
}

/// `parent` as Ruby resolves it from inside `owner`: the innermost
/// enclosing namespace that defines it, then the top level.
pub(super) fn resolve(owner: &str, parent: &str, names: &HashSet<String>) -> Option<String> {
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

/// The type a `Types::...` expression names, when it is one modeled here.
pub(super) fn dry_type(expr: &Expr, scope: &Scope<'_>) -> Option<DryType> {
    let plain = |base| Some(DryType::of(base));
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
                ["Coercible" | "Params", "Float"] => Base::Coerce("Float"),
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
                ["Strict", t @ ("String" | "Integer" | "Float" | "Hash" | "Array" | "Symbol" | "Range")] => {
                    Base::Strict(match *t {
                        "String" => "String",
                        "Integer" => "Integer",
                        "Float" => "Float",
                        "Hash" => "Hash",
                        "Symbol" => "Symbol",
                        "Range" => "Range",
                        _ => "Array",
                    })
                }
                ["Strict", "Bool"] => Base::StrictBool,
                ["Strict", "Time"] => Base::Instance("::Time".into()),
                ["Strict", "DateTime"] => Base::Instance("::DateTime".into()),
                ["Strict", "Date"] => Base::Instance("::Date".into()),
                ["Any"] => Base::Nominal,
                ["Nominal", t]
                    if matches!(*t, "String" | "Integer" | "Float" | "Bool" | "Hash" | "Array" | "Any" | "Symbol" | "Range") =>
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
        // `.fallback { |input| ... }`: called with the input whenever the
        // type fails. Only a block that reads no `self`, as a constructor.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "fallback" && args.is_empty() =>
        {
            let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
            let param = match params.as_slice() {
                [] => None,
                [param] => Some(param.clone()),
                _ => return None,
            };
            if reads_self(body) {
                return None;
            }
            let mut ty = dry_type(recv, scope)?;
            if ty.fallback.is_some() {
                return None;
            }
            ty.fallback = Some(Fallback::Block { param, body: next_to_return(body) });
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
                DryType::of(Base::Nominal)
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
                // Literal values, or a splatted list read without `self`.
                // A Hash makes a mapping enum, which this does not model.
                if !args.iter().all(|a| {
                    (literal(a) && !matches!(&*a.node, ExprNode::Hash { .. }))
                        || matches!(&*a.node, ExprNode::Splat { value } if !reads_self(value))
                }) {
                    return None;
                }
                let mut ty = dry_type(recv, scope)?;
                if ty.enumeration.is_some() || ty.fallback.is_some() {
                    return None;
                }
                ty.enumeration = Some(args.clone());
                Some(ty)
            }
            // `.constrained(gt: 0, format: RE)`: arguments read without `self`,
            // checked after the coercion, as dry-types does.
            "constrained" => {
                let [ExprNode::Hash { entries, .. }] = args.iter().map(|a| &*a.node).collect::<Vec<_>>()[..]
                else {
                    return None;
                };
                let mut ty = dry_type(recv, scope)?;
                if ty.optional || ty.fallback.is_some() || ty.default.is_some() {
                    return None;
                }
                for (key, value) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*key.node else { return None };
                    let unary = predicate(name.as_str())?;
                    if unary {
                        if !matches!(&*value.node, ExprNode::Lit { value: Literal::Bool { value: true } }) {
                            return None;
                        }
                    } else if reads_self(value) {
                        return None;
                    }
                    ty.constraints.push((name.clone(), value.clone()));
                }
                Some(ty)
            }
            "fallback" => {
                let value = match args.as_slice() {
                    [value] => value,
                    [value, options] if shared_option(options) => value,
                    _ => return None,
                };
                if reads_self(value) {
                    return None;
                }
                let mut ty = dry_type(recv, scope)?;
                if ty.fallback.is_some() {
                    return None;
                }
                ty.fallback = Some(Fallback::Value(value.clone()));
                Some(ty)
            }
            // `Types.Array(T)`: `Strict(Array).of(T)`.
            "Array" => {
                let [member] = args.as_slice() else { return None };
                let ExprNode::Const { path } = &*recv.node else { return None };
                if !types_path(path, scope.owner, scope.types_modules, scope.names)?.is_empty() {
                    return None;
                }
                let member = dry_type(member, scope)?;
                plain(Base::ArrayOf { strict: true, member: Box::new(member) })
            }
            "of" => {
                let [member] = args.as_slice() else { return None };
                let ExprNode::Const { path } = &*recv.node else { return None };
                let strict = match types_path(path, scope.owner, scope.types_modules, scope.names)?.as_slice() {
                    ["Nominal", "Array"] => false,
                    ["Strict", "Array"] => true,
                    _ => return None,
                };
                let member = dry_type(member, scope).or_else(|| uncallable(member, scope))?;
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

/// An app class written where a type belongs: not a `Types` path, a
/// struct, or a constant.
fn uncallable(expr: &Expr, scope: &Scope<'_>) -> Option<DryType> {
    let ExprNode::Const { path } = &*expr.node else { return None };
    if types_module_of(path, scope.owner, scope.types_modules, scope.names).is_some() {
        return None;
    }
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    let name = written.join("::");
    let full = if path.first().is_some_and(|s| s.as_str().is_empty()) {
        scope.classes.contains(&name).then_some(name)?
    } else {
        resolve_in(scope.owner, &name, scope.classes)?
    };
    if scope.structs.contains_key(&full) || scope.constants.contains_key(&full) {
        return None;
    }
    Some(DryType::of(Base::Uncallable(full)))
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
pub(super) fn types_path<'a>(
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
pub(super) fn types_module_of<'a>(
    path: &'a [Symbol],
    owner: &str,
    types_modules: &[(String, BareNames)],
    names: &HashSet<String>,
) -> Option<(String, BareNames, Vec<&'a str>)> {
    let absolute = path.first().is_some_and(|s| s.as_str().is_empty());
    let segments: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    // Inside a `Types` module's own body, a bare `String` is the one its
    // `include Dry.Types()` brings in.
    let Some(at) = segments.iter().rposition(|s| *s == "Types") else {
        let (module, bare) = types_modules.iter().find(|(m, _)| m == owner).filter(|_| !absolute)?;
        return Some((module.clone(), *bare, segments));
    };
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
pub(super) fn next_to_return(expr: &Expr) -> Expr {
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
pub(super) fn holds_dry_type(
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

/// The `Types` modules' own constants that build no type, an alias of
/// one (`ALIAS = Types::LABEL`) included: round by round.
pub(super) fn plain_constants(
    classes: &[crate::dialect::LibraryClass],
    types_modules: &[(String, BareNames)],
    names: &HashSet<String>,
) -> HashSet<String> {
    let mut plain: HashSet<String> = HashSet::new();
    loop {
        let found: Vec<String> = classes
            .iter()
            .filter(|lc| types_modules.iter().any(|(m, _)| m == lc.name.0.as_str()))
            .flat_map(|lc| {
                let holder = lc.name.0.as_str();
                lc.constants.iter().map(move |(n, v)| (format!("{holder}::{}", n.as_str()), v, holder))
            })
            .filter(|(full, v, holder)| {
                !plain.contains(full) && !holds_dry_type(v, holder, types_modules, names, &plain)
            })
            .map(|(full, _, _)| full)
            .collect();
        if found.is_empty() {
            return plain;
        }
        plain.extend(found);
    }
}

/// `shared: true`, the only `.default` option.
pub(super) fn shared_option(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Hash { entries, .. } if entries.len() == 1 && entries.iter().all(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "shared")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { .. } })
    }))
}

/// Whether `expr` reads the receiver it runs on: a bare call, an ivar,
/// `self`. Kernel's conversion functions are bare calls that read none.
pub(super) fn reads_self(expr: &Expr) -> bool {
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
pub(super) fn literal(expr: &Expr) -> bool {
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
pub(super) struct Gen {
    /// The struct class being generated, by full name.
    pub(super) owner: String,
    pub(super) constants: Vec<String>,
    pub(super) helpers: Vec<String>,
    /// Locals handed out so far; each holds one value, of one type.
    pub(super) temps: usize,
    /// What a value that cannot be coerced raises, and what one that
    /// fails a check raises: `Dry::Struct::Error` for both inside a
    /// struct, dry-types' `CoercionError` and its `ConstraintError`
    /// subclass where the gem lets them through.
    pub(super) coercion_error: &'static str,
    pub(super) constraint_error: &'static str,
}

impl Gen {
    /// The expression a missing key takes: the shared value, through a
    /// constant unless it already is one, or the block's body.
    pub(super) fn missing_value(&mut self, default: &DefaultValue) -> String {
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
pub(super) fn coerced(extra: &mut Gen, ty: &DryType, value: &str, key: &str) -> String {
    let fail = format!(
        "raise({}, \"[#{{self.class}}.new] {key} has an invalid value\")",
        extra.constraint_error
    );
    let fail_coerce = format!(
        "raise({}, \"[#{{self.class}}.new] {key} cannot be coerced\")",
        extra.coercion_error
    );
    let error = extra.coercion_error;
    let inner = match &ty.base {
        Base::Coerce(kernel) => {
            format!("(begin\n  {kernel}({value})\nrescue ArgumentError, TypeError\n  {fail_coerce}\nend)")
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
            "(begin\n  {value}.is_a?(String) ? Integer({value}, 10) : Integer({value})\nrescue ArgumentError, TypeError\n  {fail_coerce}\nend)"
        ),
        Base::ParamsBool => format!(
            "(%w[1 on On ON t true True TRUE T y yes Yes YES Y].include?({value}.to_s) ? true : (%w[0 off Off OFF f false False FALSE F n no No NO N].include?({value}.to_s) ? false : {fail_coerce}))"
        ),
        // `to_d` as bigdecimal/util defines it, which the tree does not load.
        Base::ParamsDecimal => format!(
            "(begin\n  Float({value})\n  case {value}\n  when Float then BigDecimal({value}, 0)\n  when String then BigDecimal.interpret_loosely({value})\n  when BigDecimal then {value}\n  else BigDecimal({value})\n  end\nrescue ArgumentError, TypeError\n  {fail_coerce}\nend)"
        ),
        Base::Parse(class) => format!(
            "({value}.respond_to?(:to_str) ? (begin\n  ::{class}.parse({value})\nrescue ArgumentError, RangeError\n  {fail_coerce}\nend) : ({value}.is_a?(::{class}) ? {value} : {fail_coerce}))"
        ),
        Base::CoerceSymbol => {
            format!("(begin\n  {value}.to_sym\nrescue NoMethodError\n  {fail_coerce}\nend)")
        }
        Base::Instance(class) => format!("({value}.is_a?({class}) ? {value} : {fail})"),
        Base::Struct(class) => format!(
            "({value}.is_a?(::{class}) ? {value} : ({value}.is_a?(Hash) ? ::{class}.new({value}) : {fail}))"
        ),
        Base::Nominal => value.to_string(),
        Base::Uncallable(class) => {
            format!("raise(NoMethodError, \"undefined method 'call_unsafe' for class {class}\")")
        }
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
                format!("(begin\n  {first}\nrescue {error}\n  {rest}\nend)")
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
    let temp = format!("dry_struct_enum_{}", extra.temps);
    let mut checks = Vec::new();
    if let Some(values) = &ty.enumeration {
        let list = values.iter().map(crate::emit::ruby::emit_expr).collect::<Vec<_>>().join(", ");
        checks.push(format!("[{list}].include?({temp})"));
    }
    for (name, arg) in &ty.constraints {
        let arg = crate::emit::ruby::emit_expr(arg);
        checks.push(match name.as_str() {
            "gt" => format!("{temp} > ({arg})"),
            "gteq" => format!("!({temp} < ({arg}))"),
            "lt" => format!("{temp} < ({arg})"),
            "lteq" => format!("!({temp} > ({arg}))"),
            "min_size" => format!("{temp}.size >= ({arg})"),
            "max_size" => format!("{temp}.size <= ({arg})"),
            "size" => format!(
                "(dry_logic_size = ({arg})\ndry_logic_size.is_a?(Integer) ? dry_logic_size.equal?({temp}.size) : dry_logic_size.include?({temp}.size))"
            ),
            "format" => format!("(!{temp}.nil? && ({arg}).match?({temp}))"),
            "included_in" => format!("({arg}).include?({temp})"),
            "excluded_from" => format!("!({arg}).include?({temp})"),
            "eql" => format!("({arg}).eql?({temp})"),
            // `filled: true`.
            _ => format!(
                "!(case {temp}\nwhen ::String, ::Array, ::Hash then {temp}.empty?\nwhen nil then true\nelse false\nend)"
            ),
        });
    }
    let inner = if checks.is_empty() {
        inner
    } else {
        extra.temps += 1;
        format!("({temp} = {inner}\n({}) ? {temp} : {fail})", checks.join(") && ("))
    };
    let inner = if ty.optional {
        format!("({value}.nil? ? nil : ({inner}))")
    } else {
        inner
    };
    // Any failure of the type, a coercion or a check, gives the fallback.
    match &ty.fallback {
        None => inner,
        Some(fallback) => {
            let instead = match fallback {
                Fallback::Value(v) => extra.missing_value(&DefaultValue::Value(v.clone())),
                Fallback::Block { param, body } => {
                    let name = format!("dry_struct_fallback_{}", extra.helpers.len());
                    extra.helpers.push(format!(
                        "  def self.{name}({})\n{}\n  end\n",
                        param.as_ref().map_or("_input", |p| p.as_str()),
                        crate::emit::ruby::emit_expr(body),
                    ));
                    format!("::{}.{name}({value})", extra.owner)
                }
            };
            format!("(begin\n  {inner}\nrescue {error}\n  {instead}\nend)")
        }
    }
}

