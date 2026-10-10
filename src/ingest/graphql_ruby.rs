//! graphql-ruby object types, read for the analyzer.
//!
//! A `field :posted_by, UserType, null: false, method: :user` resolves
//! at request time: graphql-ruby calls the type's own `posted_by` if the
//! class defines one, and otherwise `object.user` on the record the type
//! wraps (`Schema::Field#resolve`, the same order in 1.10 and 2.x). What
//! `object` is comes from the parent field: the `User` a `posted_by`
//! returns is the object a `UserType` wraps.
//!
//! This pass writes that down as ordinary Ruby on each object type:
//! `__gql_value_<field>` is the value graphql-ruby would resolve, and,
//! for a field whose type is another object type, `__gql_wrap_<field>`
//! constructs that type around it. Inference then carries the record
//! class from the root (`query Types::QueryType`, whose object is nil)
//! down every edge, cycles included, with no annotation. The methods
//! are for the analyzer only: `GraphqlObjectType::synthesized` names
//! them, and lowering removes them, so no target emits them. The
//! `field` calls themselves stay in `unknown_calls`, as before.
//!
//! Arguments are passed the way graphql-ruby passes them, as keywords:
//! each `argument` (in the field's block, or a resolver's class body)
//! becomes a typed, body-less method in `rbs_signatures`
//! (`__gql_arg_<field>_<name>`; `String`/`ID` a String, `Int` an
//! Integer, an enum its value's name, an input object its class, with
//! a reader per argument; nilable unless required; `loads:`/`prepare:`
//! and unknown types `untyped`), and the call reads them. A parameter
//! no argument fills, or an argument no parameter takes, is a call
//! graphql-ruby would fail; it is recorded as
//! `GraphqlResolution::Arguments` and not made.
//!
//! A `resolver:`/`mutation:` class is followed through its `resolve`,
//! or, for search_object, its `scope { … }` block; its `type`
//! declaration types the field.
//!
//! What else is not modeled is recorded on the field as `Skipped`,
//! never guessed: a type with an include out of sight, `hash_key:`/
//! `dig:`, connections, a field block holding more than `argument`s,
//! and other options that move where the value comes from.

use std::collections::{HashMap, HashSet};

use crate::dialect::{
    GraphqlField, GraphqlObjectType, GraphqlResolution, LibraryClass, MethodDef, MethodReceiver,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

const OBJECT_BASE: &str = "GraphQL::Schema::Object";
const SCHEMA_BASE: &str = "GraphQL::Schema";
const RESOLVER_BASES: &[&str] = &[
    "GraphQL::Schema::Resolver",
    "GraphQL::Schema::Mutation",
    "GraphQL::Schema::RelayClassicMutation",
];

const ENUM_BASE: &str = "GraphQL::Schema::Enum";
const INPUT_BASE: &str = "GraphQL::Schema::InputObject";

/// Root operation types a schema names (`query Types::QueryType`).
const ROOT_DECLARATIONS: &[&str] = &["query", "mutation", "subscription"];

pub(super) fn lower_graphql_types(app: &mut crate::App) {
    let parents = resolved_parents(&app.library_classes);
    let objects = descendants_of(&app.library_classes, &parents, &[OBJECT_BASE]);
    if objects.is_empty() {
        return;
    }
    let schemas = descendants_of(&app.library_classes, &parents, &[SCHEMA_BASE]);
    let resolvers = descendants_of(&app.library_classes, &parents, RESOLVER_BASES);
    let roots = root_types(&app.library_classes, &schemas, &objects);
    let by_name: HashMap<ClassId, usize> = app
        .library_classes
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.clone(), i))
        .collect();
    let names: HashSet<ClassId> = by_name.keys().cloned().collect();
    let kinds = ArgKinds {
        enums: plain_enums(
            &app.library_classes,
            &descendants_of(&app.library_classes, &parents, &[ENUM_BASE]),
        ),
        inputs: descendants_of(&app.library_classes, &parents, &[INPUT_BASE]),
    };
    // Typed, body-less methods the synthesized calls read their
    // arguments from, declared the way a `sig/` sidecar would be.
    let mut signatures: Vec<(ClassId, Symbol, Ty)> = Vec::new();
    let ctx = Ctx {
        classes: &app.library_classes,
        names: &names,
        parents: &parents,
        by_name: &by_name,
    };

    // Resolver and mutation classes first: a field that names one takes
    // its value, and its declared `type`, from what is read here.
    let mut modeled: HashMap<ClassId, ResolverModel> = HashMap::new();
    let mut unmodeled: HashMap<ClassId, &str> = HashMap::new();
    let mut synthesized_sources: Vec<(
        ClassId,
        String,
        HashMap<Symbol, Span>,
        Vec<Symbol>,
        Option<GraphqlObjectType>,
    )> = Vec::new();
    let mut sorted_resolvers: Vec<&ClassId> = resolvers.iter().collect();
    sorted_resolvers.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    for class_id in sorted_resolvers {
        let chain = ctx.chain(class_id);
        let mut source = String::new();
        let mut synthesized = Vec::new();
        if plumbing(&chain, &mut source, &mut synthesized).is_none() {
            unmodeled.insert(
                class_id.clone(),
                "resolver defines `initialize` or `object`",
            );
            continue;
        }
        let arguments = class_arguments(&chain);
        let value = match ctx.defined(&chain, "resolve") {
            Some(m) => {
                let Some(call) = keyword_call(m, &arguments, "resolve") else {
                    unmodeled.insert(
                        class_id.clone(),
                        "resolver `resolve` parameters do not match its arguments",
                    );
                    continue;
                };
                for a in &arguments {
                    signatures.push((
                        class_id.clone(),
                        arg_method("resolve", &a.keyword),
                        arg_ty(a, class_id, &ctx, &kinds),
                    ));
                }
                source.push_str(&format!(" def __gql_resolve\n  {call}\n end\n"));
                synthesized.push(Symbol::from("__gql_resolve"));
                "__gql_resolve".to_owned()
            }
            None => {
                // search_object: the results are the `scope` block's
                // relation, which every `option` narrows and returns.
                let Some(scope) = chain.iter().find_map(|c| scope_block(c)) else {
                    unmodeled.insert(class_id.clone(), "resolver has no `resolve`");
                    continue;
                };
                source.push_str(&format!(" def __gql_scope\n  {}\n end\n", scope));
                synthesized.push(Symbol::from("__gql_scope"));
                "__gql_scope".to_owned()
            }
        };
        let declared = chain.iter().find_map(|c| {
            c.unknown_calls
                .iter()
                .find_map(|call| type_declaration(call).map(|t| (c, t)))
        });
        let (object_type, list) = match declared {
            Some((c, (path, list))) => (resolve_const(&path, &c.name, &objects), list),
            None => (None, false),
        };
        let nullable = chain
            .iter()
            .find_map(|c| c.unknown_calls.iter().find_map(null_declaration))
            .unwrap_or(true);
        modeled.insert(
            class_id.clone(),
            ResolverModel {
                value,
                object_type,
                list,
                nullable,
            },
        );
        synthesized_sources.push((class_id.clone(), source, HashMap::new(), synthesized, None));
    }

    // A type that another object type extends is a base (`BaseObject`,
    // `BaseNode`): it declares fields for its subclasses but nothing
    // constructs it, so its fields are typed on each subclass instead.
    let bases: HashSet<ClassId> = objects
        .iter()
        .filter_map(|c| parents.get(c).cloned().flatten())
        .filter(|p| objects.contains(p))
        .collect();
    let mut concrete: Vec<&ClassId> = objects.iter().filter(|c| !bases.contains(*c)).collect();
    concrete.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    for class_id in concrete {
        let chain = ctx.chain(class_id);
        let opaque = ctx.opaque(&chain);
        let mut fields = Vec::new();
        // Ancestors first: a subclass's own `field` replaces an
        // inherited one of the same name, as graphql-ruby's does.
        for class in chain.iter().rev() {
            for call in &class.unknown_calls {
                let Some(decl) = field_declaration(call) else {
                    continue;
                };
                fields.retain(|f: &(GraphqlField, String)| f.0.name != decl.name);
                let mut object_type = decl
                    .type_path
                    .as_ref()
                    .and_then(|p| resolve_const(p, &class.name, &objects));
                let mut list = decl.list;
                let mut nullable = decl.nullable;
                let value = method_name("__gql_value_", &decl.name);
                let (resolution, body): (GraphqlResolution, String) =
                    if let Some(path) = &decl.resolver_class {
                        let target = resolve_const(path, &class.name, &resolvers);
                        match target.clone().and_then(|r| modeled.get(&r).map(|m| (r, m))) {
                            Some((r, model)) => {
                                if decl.type_path.is_none() {
                                    object_type = model.object_type.clone();
                                    list = model.list;
                                }
                                if !decl.null_given {
                                    nullable = model.nullable;
                                }
                                (
                                    GraphqlResolution::Value { method: value },
                                    format!("{}.new(object).{}", r.0.as_str(), model.value),
                                )
                            }
                            None => {
                                let reason = target
                                    .and_then(|r| unmodeled.get(&r).copied())
                                    .unwrap_or("resolver class not found");
                                (skipped(reason), String::new())
                            }
                        }
                    } else if let Some(reason) = &decl.skip {
                        (skipped(reason), String::new())
                    } else {
                        match ctx.defined(&chain, decl.resolver_method.as_str()) {
                            Some(m) => match keyword_call(m, &decl.arguments, decl.name.as_str()) {
                                Some(call) => (GraphqlResolution::Value { method: value }, call),
                                None => (
                                    GraphqlResolution::Arguments {
                                        method: m.name.clone(),
                                    },
                                    String::new(),
                                ),
                            },
                            // The method may be in a module out of sight;
                            // `object.<name>` would be a guess.
                            None if opaque.is_some() => {
                                (skipped(opaque.as_deref().unwrap()), String::new())
                            }
                            None => (
                                GraphqlResolution::Value { method: value },
                                format!(
                                    "object.{}{}",
                                    decl.method_sym.as_str(),
                                    keywords(&decl.arguments, decl.name.as_str())
                                ),
                            ),
                        }
                    };
                if matches!(resolution, GraphqlResolution::Value { .. })
                    && decl.resolver_class.is_none()
                {
                    for a in &decl.arguments {
                        signatures.push((
                            class_id.clone(),
                            arg_method(decl.name.as_str(), &a.keyword),
                            arg_ty(a, &class.name, &ctx, &kinds),
                        ));
                    }
                }
                let field = GraphqlField {
                    name: decl.name.clone(),
                    span: call.span,
                    nullable,
                    object_type,
                    list,
                    resolution,
                };
                fields.push((field, body));
            }
        }

        let mut source = String::new();
        let mut synthesized = Vec::new();
        // No modeled `object` (the app overrides `initialize`/`object`)
        // means no type for anything a field reads; leave it unread.
        if plumbing(&chain, &mut source, &mut synthesized).is_none() {
            continue;
        }
        if roots.contains(class_id) {
            source.push_str(" def self.__gql_root\n  new(nil)\n end\n");
            synthesized.push(Symbol::from("__gql_root"));
        }
        let mut spans = HashMap::new();
        for (field, body) in &fields {
            let GraphqlResolution::Value { method } = &field.resolution else {
                continue;
            };
            source.push_str(&format!(" def {}\n  {}\n end\n", method.as_str(), body));
            synthesized.push(method.clone());
            spans.insert(method.clone(), field.span);
            if let Some(target) = &field.object_type {
                let construct = if field.list {
                    format!("v.map {{ |e| {}.new(e) }}", target.0.as_str())
                } else {
                    format!("{}.new(v)", target.0.as_str())
                };
                let wrap = method_name("__gql_wrap_", &field.name);
                source.push_str(&format!(
                    " def {}\n  v = {}\n  if v\n   {}\n  end\n end\n",
                    wrap.as_str(),
                    method.as_str(),
                    construct
                ));
                synthesized.push(wrap.clone());
                spans.insert(wrap, field.span);
            }
        }
        let record = GraphqlObjectType {
            class: class_id.clone(),
            fields: fields.into_iter().map(|(f, _)| f).collect(),
            synthesized: Vec::new(),
            resolver: false,
        };
        synthesized_sources.push((class_id.clone(), source, spans, synthesized, Some(record)));
    }

    // An input object reads its arguments as methods and by key.
    let mut inputs: Vec<&ClassId> = kinds.inputs.iter().collect();
    inputs.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    for input in inputs {
        let chain = ctx.chain(input);
        for a in class_arguments(&chain) {
            let ty = arg_ty(&a, input, &ctx, &kinds);
            signatures.push((input.clone(), a.keyword.clone(), ty));
        }
        signatures.push((input.clone(), Symbol::from("[]"), Ty::Untyped));
        signatures.push((
            input.clone(),
            Symbol::from("to_h"),
            Ty::Hash {
                key: std::sync::Arc::new(Ty::Sym),
                value: std::sync::Arc::new(Ty::Untyped),
            },
        ));
    }
    let mut types = Vec::new();
    let mut synthesized_elsewhere = Vec::new();
    for (class_id, source, spans, synthesized, record) in synthesized_sources {
        let index = by_name[&class_id];
        let Some(mut methods) = synthesize(&app.library_classes[index], &source, &spans) else {
            continue;
        };
        app.library_classes[index].methods.append(&mut methods);
        match record {
            Some(mut record) => {
                record.synthesized = synthesized;
                types.push(record);
            }
            None => synthesized_elsewhere.push(GraphqlObjectType {
                class: class_id,
                fields: Vec::new(),
                synthesized,
                resolver: true,
            }),
        }
    }
    types.extend(synthesized_elsewhere);
    let mut declared = Vec::new();
    for (class, name, ret) in signatures {
        let params = if name.as_str() == "[]" {
            vec![crate::ty::Param {
                name: Symbol::from("key"),
                ty: Ty::Untyped.into(),
                kind: crate::ty::ParamKind::Required,
            }]
        } else {
            Vec::new()
        };
        let table = app.rbs_signatures.entry(class.clone()).or_default();
        // A sidecar the app wrote wins.
        if table.contains_key(&name) {
            continue;
        }
        table.insert(
            name.clone(),
            Ty::Fn {
                params: params.into(),
                block: None,
                ret: std::sync::Arc::new(ret),
                effects: EffectSet::pure(),
            },
        );
        declared.push((class, name));
    }
    app.graphql_types = types;
    app.graphql_signatures = declared;
}

struct Ctx<'a> {
    classes: &'a [LibraryClass],
    names: &'a HashSet<ClassId>,
    parents: &'a HashMap<ClassId, Option<ClassId>>,
    by_name: &'a HashMap<ClassId, usize>,
}

impl<'a> Ctx<'a> {
    /// The class itself, then each app-defined ancestor, each followed
    /// by the app modules it includes (last included first), the order
    /// Ruby looks a method up in.
    fn chain(&self, class: &ClassId) -> Vec<&'a LibraryClass> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = Some(class.clone());
        while let Some(c) = cursor {
            let Some(&i) = self.by_name.get(&c) else {
                break;
            };
            if !seen.insert(c.clone()) {
                break;
            }
            let class = &self.classes[i];
            out.push(class);
            self.push_modules(class, &mut out, &mut seen);
            cursor = self.parents.get(&c).cloned().flatten();
        }
        out
    }

    fn push_modules(
        &self,
        class: &'a LibraryClass,
        out: &mut Vec<&'a LibraryClass>,
        seen: &mut HashSet<ClassId>,
    ) {
        for include in class.includes.iter().rev() {
            let Some(module) = self.module(include, &class.name) else {
                continue;
            };
            if seen.insert(module.name.clone()) {
                out.push(module);
                self.push_modules(module, out, seen);
            }
        }
    }

    /// An `include`d name, resolved lexically against the app's modules.
    fn module(&self, include: &ClassId, scope: &ClassId) -> Option<&'a LibraryClass> {
        let path: Vec<Symbol> = include.0.as_str().split("::").map(Symbol::from).collect();
        let id = resolve_const(&path, scope, self.names)?;
        Some(&self.classes[self.by_name[&id]]).filter(|c| c.is_module)
    }

    /// Why `chain` may have methods this pass cannot see: an included
    /// module the app does not define, or one computed at load time.
    fn opaque(&self, chain: &[&'a LibraryClass]) -> Option<String> {
        chain.iter().find_map(|class| {
            if class.unknown_calls.iter().any(|call| {
                matches!(&*call.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "include")
            }) {
                return Some("computed include".to_owned());
            }
            class
                .includes
                .iter()
                .find(|i| self.module(i, &class.name).is_none())
                .map(|i| format!("includes `{}`", i.0.as_str()))
        })
    }

    /// The nearest instance method `name` along `chain`.
    fn defined(&self, chain: &[&'a LibraryClass], name: &str) -> Option<&'a MethodDef> {
        chain.iter().find_map(|c| {
            c.methods
                .iter()
                .find(|m| m.name.as_str() == name && matches!(m.receiver, MethodReceiver::Instance))
        })
    }
}

struct ResolverModel {
    /// The method on the resolver that produces the field's value.
    value: String,
    object_type: Option<ClassId>,
    list: bool,
    nullable: bool,
}

/// graphql-ruby's `object` reader, per class, so each class's slot
/// holds only what its own constructions pass. The guard narrows the
/// conservative nil of an ivar read; a root type's object really is
/// nil, and the guard makes a field that reads it unresolvable rather
/// than silently nil. `None` when the app defines either itself.
fn plumbing(
    chain: &[&LibraryClass],
    source: &mut String,
    synthesized: &mut Vec<Symbol>,
) -> Option<()> {
    let defines = |name: &str| {
        chain.iter().any(|c| {
            c.methods
                .iter()
                .any(|m| m.name.as_str() == name && matches!(m.receiver, MethodReceiver::Instance))
        })
    };
    if defines("initialize") || defines("object") {
        return None;
    }
    source.push_str(" def initialize(__gql_object)\n  @object = __gql_object\n end\n");
    source.push_str(" def object\n  @object || raise(\"graphql object\")\n end\n");
    synthesized.push(Symbol::from("initialize"));
    synthesized.push(Symbol::from("object"));
    Some(())
}

/// An `argument :name, Type, required: …` declaration.
#[derive(Clone)]
struct ArgDecl {
    /// The Ruby keyword graphql-ruby passes it as (`as:`, or the name
    /// less `_id` for `loads:`).
    keyword: Symbol,
    type_path: Option<Vec<Symbol>>,
    list: bool,
    /// `required: false` or `:nullable`: the value can be nil.
    nullable: bool,
    /// `loads:` / `prepare:`: the method receives something other than
    /// the declared type.
    transformed: bool,
}

fn argument_declaration(call: &Expr) -> Option<ArgDecl> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "argument" {
        return None;
    }
    let name = match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit {
            value: Literal::Sym { value },
        }) => value.clone(),
        Some(ExprNode::Lit {
            value: Literal::Str { value },
        }) => Symbol::from(value.as_str()),
        _ => return None,
    };
    let mut decl = ArgDecl {
        keyword: name.clone(),
        type_path: None,
        list: false,
        nullable: false,
        transformed: false,
    };
    let mut renamed = None;
    for arg in &args[1..] {
        match &*arg.node {
            ExprNode::Const { path } => decl.type_path = Some(path.clone()),
            ExprNode::Array { elements, .. } => {
                decl.list = true;
                if let Some(ExprNode::Const { path }) = elements.first().map(|e| &*e.node) {
                    decl.type_path = Some(path.clone());
                }
            }
            ExprNode::Hash { entries, .. } => {
                for (key, value) in entries {
                    let ExprNode::Lit {
                        value: Literal::Sym { value: key },
                    } = &*key.node
                    else {
                        return None;
                    };
                    match (key.as_str(), &*value.node) {
                        (
                            "required",
                            ExprNode::Lit {
                                value: Literal::Bool { value },
                            },
                        ) => decl.nullable = !value,
                        (
                            "required",
                            ExprNode::Lit {
                                value: Literal::Sym { value },
                            },
                        ) if value.as_str() == "nullable" => decl.nullable = true,
                        (
                            "as",
                            ExprNode::Lit {
                                value: Literal::Sym { value },
                            },
                        ) => renamed = Some(value.clone()),
                        ("loads", _) => {
                            decl.transformed = true;
                            let n = name.as_str();
                            decl.keyword =
                                Symbol::from(if let Some(base) = n.strip_suffix("_ids") {
                                    format!("{base}s")
                                } else {
                                    n.strip_suffix("_id").unwrap_or(n).to_owned()
                                });
                        }
                        ("prepare", _) => decl.transformed = true,
                        ("type", ExprNode::Const { path }) => decl.type_path = Some(path.clone()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(as_name) = renamed {
        decl.keyword = as_name;
    }
    Some(decl)
}

/// A resolver's or input object's class-body `argument`s, ancestors
/// first.
fn class_arguments(chain: &[&LibraryClass]) -> Vec<ArgDecl> {
    let mut out: Vec<ArgDecl> = Vec::new();
    for class in chain.iter().rev() {
        for a in class.unknown_calls.iter().filter_map(argument_declaration) {
            out.retain(|b| b.keyword != a.keyword);
            out.push(a);
        }
    }
    out
}

fn statements(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

fn arg_method(owner: &str, keyword: &Symbol) -> Symbol {
    Symbol::from(format!("__gql_arg_{owner}_{}", keyword.as_str()))
}

/// `(k: self.__gql_arg_<owner>_k, …)`, or nothing without arguments.
fn keywords(arguments: &[ArgDecl], owner: &str) -> String {
    if arguments.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = arguments
        .iter()
        .map(|a| {
            format!(
                "{}: self.{}",
                a.keyword.as_str(),
                arg_method(owner, &a.keyword).as_str()
            )
        })
        .collect();
    format!("({})", pairs.join(", "))
}

/// `self.<m>(…)` passing the declared arguments the way graphql-ruby
/// does, as keywords, when `m`'s parameters take exactly that call: no
/// required positional, every required keyword declared, every argument
/// one `m` names (or a `**rest`). `None` is a mismatch this pass does
/// not model. Ingest flattens an optional keyword to a positional with
/// a default (`Param::from_keyword`), so those slots are filled in
/// order: the argument, or the default when it is not declared.
fn keyword_call(m: &MethodDef, arguments: &[ArgDecl], owner: &str) -> Option<String> {
    let value = |a: &ArgDecl| format!("self.{}", arg_method(owner, &a.keyword).as_str());
    let named = |name: &Symbol| arguments.iter().find(|a| &a.keyword == name);
    let mut positional = Vec::new();
    let mut keywords = Vec::new();
    let mut used: Vec<&Symbol> = Vec::new();
    let mut bundle = None;
    for p in &m.params {
        if p.forwarding || (p.rest && !p.keyword) {
            continue;
        }
        if p.keyword && p.rest {
            bundle = Some(false);
        } else if p.from_kwrest {
            bundle = Some(true);
        } else if p.keyword {
            match named(&p.name) {
                Some(a) => {
                    keywords.push(format!("{}: {}", p.name.as_str(), value(a)));
                    used.push(&a.keyword);
                }
                None if p.default.is_some() => {}
                None => return None,
            }
        } else if p.from_keyword {
            match named(&p.name) {
                Some(a) => {
                    positional.push(value(a));
                    used.push(&a.keyword);
                }
                None => positional.push(crate::emit::ruby::emit_expr(p.default.as_ref()?)),
            }
        } else {
            // A positional graphql-ruby never fills.
            return None;
        }
    }
    let extras: Vec<String> = arguments
        .iter()
        .filter(|a| !used.contains(&&a.keyword))
        .map(|a| format!("{}: {}", a.keyword.as_str(), value(a)))
        .collect();
    match bundle {
        _ if extras.is_empty() => {}
        Some(false) => keywords.extend(extras),
        Some(true) => positional.push(format!("{{{}}}", extras.join(", "))),
        None => return None,
    }
    positional.extend(keywords);
    let args = if positional.is_empty() {
        String::new()
    } else {
        format!("({})", positional.join(", "))
    };
    Some(format!("self.{}{}", m.name.as_str(), args))
}

struct ArgKinds {
    /// Enums whose values are their names (no `value:`): a String.
    enums: HashSet<ClassId>,
    inputs: HashSet<ClassId>,
}

/// The Ruby value graphql-ruby passes for an argument.
fn arg_ty(a: &ArgDecl, scope: &ClassId, ctx: &Ctx<'_>, kinds: &ArgKinds) -> Ty {
    if a.transformed {
        return Ty::Untyped;
    }
    let Some(path) = &a.type_path else {
        return Ty::Untyped;
    };
    let written = path
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("::");
    let base = match written.strip_prefix("GraphQL::Types::").unwrap_or(&written) {
        "String" | "ID" => Ty::Str,
        "Int" | "Integer" | "BigInt" => Ty::Int,
        "Float" => Ty::Float,
        "Boolean" => Ty::Bool,
        "ISO8601DateTime" => Ty::Time,
        "ISO8601Date" => Ty::Date,
        _ => {
            if let Some(id) = resolve_const(path, scope, ctx.names) {
                if kinds.inputs.contains(&id) {
                    Ty::Class {
                        id,
                        args: Vec::new().into(),
                    }
                } else if kinds.enums.contains(&id) {
                    Ty::Str
                } else {
                    Ty::Untyped
                }
            } else {
                Ty::Untyped
            }
        }
    };
    if matches!(base, Ty::Untyped) {
        return base;
    }
    let base = if a.list {
        Ty::Array {
            elem: std::sync::Arc::new(base),
        }
    } else {
        base
    };
    if a.nullable {
        Ty::Union {
            variants: vec![base, Ty::Nil].into(),
        }
    } else {
        base
    }
}

/// Enums every `value` of which is its own name (no `value:` option).
fn plain_enums(classes: &[LibraryClass], enums: &HashSet<ClassId>) -> HashSet<ClassId> {
    classes
        .iter()
        .filter(|c| enums.contains(&c.name))
        .filter(|c| {
            c.unknown_calls.iter().all(|call| match &*call.node {
                ExprNode::Send { recv: None, method, args, .. } if method.as_str() == "value" => {
                    !args.iter().any(|a| match &*a.node {
                        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
                            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "value")
                        }),
                        _ => false,
                    })
                }
                _ => true,
            })
        })
        .map(|c| c.name.clone())
        .collect()
}

fn skipped(reason: &str) -> GraphqlResolution {
    GraphqlResolution::Skipped {
        reason: reason.to_owned(),
    }
}

fn method_name(prefix: &str, field: &Symbol) -> Symbol {
    Symbol::from(format!("{prefix}{}", field.as_str()))
}

/// search_object's `scope { … }`, as Ruby source.
fn scope_block(class: &LibraryClass) -> Option<String> {
    class.unknown_calls.iter().find_map(|call| {
        let ExprNode::Send {
            recv: None,
            method,
            args,
            block: Some(block),
            ..
        } = &*call.node
        else {
            return None;
        };
        if method.as_str() != "scope" || !args.is_empty() {
            return None;
        }
        let ExprNode::Lambda { extra_params, params, body, .. } = &*block.node else {
            return None;
        };
        if !extra_params.is_empty() {
            return None;
        }
        params
            .is_empty()
            .then(|| crate::emit::ruby::emit_expr(body))
    })
}

/// A resolver's `type Types::LinkType` / `type [Types::LinkType]`.
fn type_declaration(call: &Expr) -> Option<(Vec<Symbol>, bool)> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "type" {
        return None;
    }
    match args.first().map(|a| &*a.node)? {
        ExprNode::Const { path } => Some((path.clone(), false)),
        ExprNode::Array { elements, .. } => match elements.first().map(|e| &*e.node)? {
            ExprNode::Const { path } => Some((path.clone(), true)),
            _ => None,
        },
        _ => None,
    }
}

/// A resolver's class-level `null false`.
fn null_declaration(call: &Expr) -> Option<bool> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "null" {
        return None;
    }
    match args.first().map(|a| &*a.node)? {
        ExprNode::Lit {
            value: Literal::Bool { value },
        } => Some(*value),
        _ => None,
    }
}

/// Parse the synthesized source as a reopening of `class` and return
/// its methods, stamped with the declaring `field` call's span (the
/// class's own span for the plumbing methods). `None` if the source
/// does not ingest cleanly, which leaves the type unread, not guessed.
fn synthesize(
    class: &LibraryClass,
    body: &str,
    spans: &HashMap<Symbol, Span>,
) -> Option<Vec<MethodDef>> {
    let source = format!("class {}\n{}end\n", class.name.0.as_str(), body);
    let (parsed, diags) =
        super::prism::scope(|| super::ingest_library_classes(source.as_bytes(), "<graphql>"));
    let mut parsed = match parsed {
        Ok(p) if diags.is_empty() && p.len() == 1 => p,
        Ok(_) => return None,
        Err(err) => {
            super::survey::record(&err);
            return None;
        }
    };
    fn mark(expr: &mut Expr, span: Span) {
        // The method was parsed from <graphql>, not this source location.
        // Its constants name exact classes selected by the field declaration;
        // the borrowed span only locates diagnostics, never Rubydex answers.
        if matches!(&*expr.node, ExprNode::Const { .. }) {
            expr.decisions |= crate::expr::GENERATED_CONST_REF;
        }
        expr.span = span;
        expr.node.for_each_child_mut(&mut |child| mark(child, span));
    }
    let fallback = class
        .methods
        .first()
        .map(|m| m.name_span)
        .unwrap_or_else(Span::synthetic);
    let mut methods = parsed.remove(0).methods;
    for method in &mut methods {
        let span = spans.get(&method.name).copied().unwrap_or(fallback);
        mark(&mut method.body, span);
        method.name_span = span;
    }
    Some(methods)
}

struct FieldDecl {
    name: Symbol,
    type_path: Option<Vec<Symbol>>,
    list: bool,
    nullable: bool,
    method_sym: Symbol,
    resolver_method: Symbol,
    /// `resolver:` / `mutation:` class, as written.
    resolver_class: Option<Vec<Symbol>>,
    null_given: bool,
    /// The `argument`s its block declares.
    arguments: Vec<ArgDecl>,
    skip: Option<String>,
}

/// `field :name, Type, null: false, method: :x` → its parts. Options
/// that change where the value comes from and that this pass does not
/// model set `skip`; documentation-only options are ignored.
fn field_declaration(call: &Expr) -> Option<FieldDecl> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "field" {
        return None;
    }
    let name = match args.first().map(|a| &*a.node) {
        Some(ExprNode::Lit {
            value: Literal::Sym { value },
        }) => value.clone(),
        Some(ExprNode::Lit {
            value: Literal::Str { value },
        }) => Symbol::from(value.as_str()),
        _ => return None,
    };
    let mut decl = FieldDecl {
        name: name.clone(),
        type_path: None,
        list: false,
        nullable: true,
        method_sym: name.clone(),
        resolver_method: name,
        resolver_class: None,
        null_given: false,
        arguments: Vec::new(),
        skip: None,
    };
    let mut skipped: Option<String> = None;
    let mut skip = |reason: &str| {
        skipped.get_or_insert_with(|| reason.to_owned());
    };
    for arg in &args[1..] {
        match &*arg.node {
            ExprNode::Const { path } => decl.type_path = Some(path.clone()),
            ExprNode::Array { elements, .. } => {
                decl.list = true;
                if let Some(ExprNode::Const { path }) = elements.first().map(|e| &*e.node) {
                    decl.type_path = Some(path.clone());
                }
            }
            ExprNode::Hash { entries, .. } => {
                for (key, value) in entries {
                    let ExprNode::Lit {
                        value: Literal::Sym { value: key },
                    } = &*key.node
                    else {
                        skip("non-symbol option");
                        continue;
                    };
                    match (key.as_str(), &*value.node) {
                        (
                            "null",
                            ExprNode::Lit {
                                value: Literal::Bool { value },
                            },
                        ) => {
                            decl.nullable = *value;
                            decl.null_given = true;
                        }
                        (
                            "method",
                            ExprNode::Lit {
                                value: Literal::Sym { value },
                            },
                        ) => decl.method_sym = value.clone(),
                        (
                            "resolver_method",
                            ExprNode::Lit {
                                value: Literal::Sym { value },
                            },
                        ) => decl.resolver_method = value.clone(),
                        (
                            "description" | "deprecation_reason" | "camelize" | "complexity"
                            | "max_page_size" | "default_page_size" | "broadcastable",
                            _,
                        ) => {}
                        ("resolver" | "mutation", ExprNode::Const { path }) => {
                            decl.resolver_class = Some(path.clone())
                        }
                        ("resolver" | "mutation" | "subscription", _) => skip("resolver class"),
                        ("hash_key" | "dig", _) => skip("hash lookup"),
                        ("connection", _) => skip("connection"),
                        ("extras", _) => skip("extras"),
                        (other, _) => skip(&format!("`{other}:` option")),
                    }
                }
            }
            _ => skip("computed field type"),
        }
    }
    // A block declares the field's `argument`s; anything else in it
    // (an extension, a `|field|` form) is not read.
    if let Some(block) = block {
        match &*block.node {
            ExprNode::Lambda { params, extra_params, body, .. } if params.is_empty() && extra_params.is_empty() => {
                for stmt in statements(body) {
                    match &*stmt.node {
                        ExprNode::Send {
                            recv: None, method, ..
                        } if method.as_str() == "argument" => match argument_declaration(stmt) {
                            Some(a) => decl.arguments.push(a),
                            None => skip("argument form"),
                        },
                        ExprNode::Send {
                            recv: None, method, ..
                        } if matches!(method.as_str(), "description" | "deprecation_reason") => {}
                        _ => skip("field block"),
                    }
                }
            }
            _ => skip("field block"),
        }
    }
    if decl
        .type_path
        .as_ref()
        .and_then(|p| p.last())
        .is_some_and(|last| last.as_str().ends_with("Connection"))
    {
        skip("connection");
    }
    decl.skip = skipped;
    Some(decl)
}

/// Each library class's parent, resolved the way Ruby would look the
/// constant up from inside the class's namespace (`class LinkType <
/// BaseNode` in `module Types` is `Types::BaseNode`). A parent the app
/// does not define stays as written (`GraphQL::Schema::Object`).
fn resolved_parents(classes: &[LibraryClass]) -> HashMap<ClassId, Option<ClassId>> {
    let names: HashSet<&str> = classes.iter().map(|c| c.name.0.as_str()).collect();
    classes
        .iter()
        .map(|c| {
            let parent = c.parent.as_ref().map(|p| {
                let segments: Vec<&str> = c.name.0.as_str().split("::").collect();
                (0..segments.len())
                    .rev()
                    .map(|n| {
                        let mut path = segments[..n].join("::");
                        if !path.is_empty() {
                            path.push_str("::");
                        }
                        path.push_str(p.0.as_str());
                        path
                    })
                    .find(|candidate| {
                        names.contains(candidate.as_str()) && candidate != c.name.0.as_str()
                    })
                    .map(|found| ClassId(Symbol::from(found)))
                    .unwrap_or_else(|| p.clone())
            });
            (c.name.clone(), parent)
        })
        .collect()
}

/// Library classes whose parent chain reaches `base`.
fn descendants_of(
    classes: &[LibraryClass],
    parents: &HashMap<ClassId, Option<ClassId>>,
    bases: &[&str],
) -> HashSet<ClassId> {
    classes
        .iter()
        .filter(|c| !c.is_module)
        .filter(|c| {
            let mut seen = HashSet::new();
            let mut cursor = parents.get(&c.name).cloned().flatten();
            while let Some(p) = cursor {
                if bases.contains(&p.0.as_str()) {
                    return true;
                }
                if !seen.insert(p.clone()) {
                    return false;
                }
                cursor = parents.get(&p).cloned().flatten();
            }
            false
        })
        .map(|c| c.name.clone())
        .collect()
}

/// Object types a schema class names as a root operation type.
fn root_types(
    classes: &[LibraryClass],
    schemas: &HashSet<ClassId>,
    objects: &HashSet<ClassId>,
) -> HashSet<ClassId> {
    classes
        .iter()
        .filter(|c| schemas.contains(&c.name))
        .flat_map(|c| c.unknown_calls.iter().map(move |call| (c, call)))
        .filter_map(|(c, call)| {
            let ExprNode::Send {
                recv: None,
                method,
                args,
                ..
            } = &*call.node
            else {
                return None;
            };
            if !ROOT_DECLARATIONS.contains(&method.as_str()) {
                return None;
            }
            let ExprNode::Const { path } = &*args.first()?.node else {
                return None;
            };
            resolve_const(path, &c.name, objects)
        })
        .collect()
}

/// A constant written inside `scope`, resolved against `known` the way
/// Ruby's lexical lookup would find it (innermost namespace first).
fn resolve_const(path: &[Symbol], scope: &ClassId, known: &HashSet<ClassId>) -> Option<ClassId> {
    let written: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    let segments: Vec<&str> = scope.0.as_str().split("::").collect();
    (0..=segments.len()).rev().find_map(|n| {
        let mut candidate: Vec<&str> = segments[..n].to_vec();
        candidate.extend(&written);
        let id = ClassId(Symbol::from(candidate.join("::")));
        known.contains(&id).then_some(id)
    })
}
