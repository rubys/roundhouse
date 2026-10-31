//! Which partial `json.partial!(record, …)` renders.
//!
//! Jbuilder hands a record to Action View, which renders
//! `record.to_partial_path` (`gadgets/_gadget` for a `Gadget`) under the
//! controller's namespace, with the record bound to `as:` or to the
//! model's singular name. The class of the record decides the template;
//! the name of the local holding it does not (`|g|`, `@item`,
//! `current_widget`, `@gadget.widget` all render their model's partial).
//!
//! The lowering has the class only statically, so this resolves the
//! argument's static type: the analyzer's type when it typed the
//! expression (ivars from the controller, helper methods, finders,
//! association readers), else the template's own evidence — a block
//! parameter over a collection of a model, a local named after a model,
//! an association reader on a resolved record, a finder on a model
//! constant. A record of a closed set of models — a union of model
//! types, a polymorphic reader with the targets the IR records — is
//! rendered by a run-time switch on its class, one branch per model's
//! partial. A record whose class is not known — an unknown type, a
//! union with a member that is no model, a model with STI subclasses,
//! a polymorphic reader with no recorded targets — is refused with a
//! reason instead of being rendered as nothing.

use crate::det::HashMap;

use crate::dialect::{Association, Model};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// Why a record's partial cannot be named statically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unresolved {
    /// No model type for the expression.
    Unknown,
    /// The expression may be records of more than one class.
    Union,
    /// The model has STI subclasses, each with its own partial.
    Sti(String),
    /// A polymorphic association reader.
    Polymorphic,
    /// `as:` names a local other than the model's singular name.
    As { model: String, as_name: Symbol },
    /// The partial reads a local the call passes besides the record.
    ReadsLocal { partial: String, local: Symbol },
    /// One record of a closed set of models (a union's members, a
    /// polymorphic reader's targets): rendered by a run-time switch
    /// on its class, so a site never reports it; the resolution of a
    /// receiver maps it to `Union`.
    Set(Vec<String>),
}

impl Unresolved {
    pub(super) fn detail(&self) -> String {
        match self {
            Unresolved::Unknown => {
                "the record's class is not statically known, so the partial Action View would pick (`to_partial_path`) cannot be named".to_string()
            }
            Unresolved::Union => {
                "the record may be of more than one class, and each class renders its own partial".to_string()
            }
            Unresolved::Sti(m) => format!(
                "`{m}` has STI subclasses, and a subclass row renders the subclass's partial"
            ),
            Unresolved::Polymorphic => {
                "a polymorphic association's record renders the partial of whatever class the row names".to_string()
            }
            Unresolved::As { model, as_name } => format!(
                "`as: :{as_name}` binds the `{model}` record under a name its partial is not lowered to take"
            ),
            Unresolved::ReadsLocal { partial, local } => format!(
                "the partial `{partial}` reads the local `{local}` the call passes, and its lowered method takes no parameter for it"
            ),
            Unresolved::Set(models) => format!("the record is one of {}", models.join(", ")),
        }
    }
}

/// The models a record may be, and whether its class is checked.
#[derive(Clone, Debug)]
pub(super) struct RecordModels {
    pub names: Vec<String>,
    /// The record is of a closed set (a union, a polymorphic reader),
    /// rendered by a switch on its class that raises for any other,
    /// even when the set has one member.
    pub switched: bool,
}

pub(super) type RecordSites = HashMap<Span, Result<RecordModels, Unresolved>>;

/// The models the record of every `json.partial!(record, …)` site in
/// a template body may be, keyed by the record argument's span: one
/// model, or the closed set of a union or a polymorphic reader, which
/// the lowering renders by a switch on the record's class. Block
/// parameters over a collection are bound to its element model as the
/// walk enters the block.
pub(super) fn record_sites(body: &Expr, models: &[Model]) -> RecordSites {
    let mut out = HashMap::default();
    let mut env = HashMap::default();
    walk(body, models, &mut env, &mut out);
    out.into_iter()
        .map(|(span, r)| {
            let r = match r {
                Ok(m) => Ok(RecordModels { names: vec![m], switched: false }),
                // A closed set is switched on the record's class even
                // with one member: a polymorphic reader with one
                // declared target still holds a row of any class.
                Err(Unresolved::Set(set)) => Ok(RecordModels { names: set, switched: true }),
                Err(why) => Err(why),
            };
            (span, r)
        })
        .collect()
}

/// `<collection>/<element>` for `model`, under the namespace of the
/// template directory `prefix` as Action View merges it
/// (`PartialRenderer#merge_prefix_into_object_path`): the prefix's
/// leading directories up to the first one the object path repeats.
pub(super) fn model_partial_path(model: &str, prefix: &str) -> (String, Symbol) {
    let segs: Vec<&str> = model.split("::").collect();
    let (last, ns) = segs.split_last().expect("a class name");
    let element = crate::naming::snake_case(last);
    let mut parts: Vec<String> = ns.iter().map(|s| crate::naming::snake_case(s)).collect();
    parts.push(crate::naming::pluralize_snake(last));
    parts.push(element.clone());
    let object_path = parts.join("/");
    let path = match prefix.rsplit_once('/') {
        Some((dirname, _)) => {
            let object_dirs: Vec<&str> = object_path.split('/').collect();
            let object_dirs = &object_dirs[..object_dirs.len().saturating_sub(2)];
            let mut prefixes: Vec<&str> = Vec::new();
            for (i, dir) in dirname.split('/').enumerate() {
                if object_dirs.get(i) == Some(&dir) {
                    break;
                }
                prefixes.push(dir);
            }
            prefixes.push(&object_path);
            prefixes.join("/")
        }
        None => object_path.clone(),
    };
    (path, Symbol::from(element))
}

type Env = HashMap<Symbol, String>;

fn walk(e: &Expr, models: &[Model], env: &mut Env, out: &mut HashMap<Span, Result<String, Unresolved>>) {
    match &*e.node {
        ExprNode::Seq { exprs } => {
            for x in exprs {
                walk(x, models, env, out);
            }
            return;
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            walk(value, models, env, out);
            match resolve(value, models, env) {
                Ok(m) => {
                    env.insert(name.clone(), m);
                }
                Err(_) => {
                    env.remove(name);
                }
            }
            return;
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            if let Some(arg) = record_arg(e) {
                if !arg.span.is_synthetic() {
                    out.insert(arg.span, resolve(arg, models, env));
                }
            }
            if let Some(r) = recv {
                walk(r, models, env, out);
            }
            for a in args {
                walk(a, models, env, out);
            }
            if let Some(b) = block {
                let mut inner = env.clone();
                if let ExprNode::Lambda { params, body, .. } = &*b.node {
                    let collection = match recv.as_ref() {
                        Some(r) if super::is_json_receiver(r) => args.first(),
                        Some(r) if ITERATORS.contains(&method.as_str()) => Some(r),
                        _ => None,
                    };
                    if let Some(p) = params.first() {
                        match collection.and_then(|c| element_model(c, models, env)) {
                            Some(m) => {
                                inner.insert(p.clone(), m);
                            }
                            None => {
                                inner.remove(p);
                            }
                        }
                    }
                    for p in params.iter().skip(1) {
                        inner.remove(p);
                    }
                    walk(body, models, &mut inner, out);
                } else {
                    walk(b, models, &mut inner, out);
                }
            }
            return;
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| {
        let mut inner = env.clone();
        walk(c, models, &mut inner, out)
    });
}

const ITERATORS: &[&str] = &["each", "map", "flat_map", "each_with_index", "filter_map", "select", "reject", "collect"];

/// Finders on a model class that answer one record of it.
const FINDERS: &[&str] = &[
    "find", "find_by", "find_by!", "first", "first!", "last", "last!", "take", "take!", "sole", "find_sole_by",
    "new", "create", "create!", "find_or_create_by", "find_or_create_by!", "find_or_initialize_by",
];

/// Relation steps that keep the collection's model.
const RELATION_STEPS: &[&str] = &["to_a", "all", "order", "where", "includes", "preload", "eager_load", "limit", "reorder", "distinct", "not"];

/// The record of `json.partial!(record, …)`: a first positional that
/// is neither a path String nor the options Hash.
fn record_arg(e: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "partial!" || !super::is_json_receiver(recv) {
        return None;
    }
    let first = args.first()?;
    is_record_position(first).then_some(first)
}

/// Whether the first positional of `json.partial!` is a record rather
/// than a partial name (a literal) or the options Hash.
pub(super) fn is_record_position(first: &Expr) -> bool {
    !matches!(
        &*first.node,
        ExprNode::Lit { .. } | ExprNode::Hash { .. } | ExprNode::StringInterp { .. }
    )
}

fn model<'a>(name: &str, models: &'a [Model]) -> Option<&'a Model> {
    models.iter().find(|m| m.name.0.as_str() == name)
}

/// A single model class, refused when it has STI subclasses.
fn checked(name: &str, models: &[Model]) -> Option<Result<String, Unresolved>> {
    let m = model(name, models)?;
    if m.sti_subclass_names.is_empty() {
        Some(Ok(name.to_string()))
    } else {
        Some(Err(Unresolved::Sti(name.to_string())))
    }
}

/// `Set` of `names` when they are two or more models each rendering
/// its own partial (no STI subclasses), else `None`: an open set, or
/// one whose members are not all models, is not switched over.
fn closed_set(names: &[&str], models: &[Model]) -> Option<Unresolved> {
    let mut set: Vec<String> = Vec::new();
    for n in names {
        match checked(n, models)? {
            Ok(m) if !set.contains(&m) => set.push(m),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    match set.len() {
        0 => None,
        _ => Some(Unresolved::Set(set)),
    }
}

/// The model a type says the value is, when it says: one model class,
/// possibly nil. `None` when the type carries no class (unknown,
/// a type variable), so the caller falls back to the template's own
/// evidence.
fn model_of_ty(ty: &Ty, models: &[Model]) -> Option<Result<String, Unresolved>> {
    match ty {
        Ty::Class { id, .. } => match checked(id.0.as_str(), models) {
            Some(r) => Some(r),
            None => Some(Err(Unresolved::Unknown)),
        },
        Ty::Union { variants } => {
            let non_nil: Vec<&Ty> = variants.iter().filter(|t| !matches!(t, Ty::Nil)).collect();
            match non_nil.as_slice() {
                [one] => model_of_ty(one, models),
                [] => None,
                many => {
                    let names: Option<Vec<&str>> = many
                        .iter()
                        .map(|t| match t {
                            Ty::Class { id, .. } => Some(id.0.as_str()),
                            _ => None,
                        })
                        .collect();
                    names.map(|names| Err(closed_set(&names, models).unwrap_or(Unresolved::Union)))
                }
            }
        }
        Ty::Str | Ty::Int | Ty::Float | Ty::Bool | Ty::Sym | Ty::Array { .. } | Ty::Hash { .. } | Ty::Relation { .. } => {
            Some(Err(Unresolved::Unknown))
        }
        _ => None,
    }
}

fn resolve(arg: &Expr, models: &[Model], env: &Env) -> Result<String, Unresolved> {
    // The template's own binding first: a block parameter's type is the
    // analyzer's type variable, and a local the template assigned is
    // what it assigned.
    if let Some(name) = bare_name(arg) {
        if let Some(m) = env.get(&name) {
            return checked(m, models).unwrap_or(Err(Unresolved::Unknown));
        }
    }
    // A polymorphic reader's type is `Target | nil` for one declared
    // target, the same as a plain `belongs_to`'s, so the type cannot
    // say the row may be of another class: the reader's declaration can.
    if matches!(&*arg.node, ExprNode::Send { recv: Some(_), .. } | ExprNode::BoolOp { .. }) {
        if let Err(set @ Unresolved::Set(_)) = structural(arg, models, env) {
            return Err(set);
        }
    }
    if let Some(r) = arg.ty.as_ref().and_then(|t| model_of_ty(t, models)) {
        return r;
    }
    structural(arg, models, env)
}

fn bare_name(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.clone()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => Some(method.clone()),
        _ => None,
    }
}

fn structural(arg: &Expr, models: &[Model], env: &Env) -> Result<String, Unresolved> {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            named_model(name, models, env)
        }
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            named_model(method, models, env)
        }
        // `a&.b` arrives as `a && a.b`, or `(__safe_nav = a) && __safe_nav.b`
        // for a receiver that is not a local: the record is the right
        // side.
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } => {
            resolve(right, models, &safe_nav_env(left, models, env))
        }
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } => {
            if let ExprNode::Const { path } = &*recv.node {
                let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                if model(&name, models).is_some() && FINDERS.contains(&method.as_str()) {
                    return checked(&name, models).unwrap_or(Err(Unresolved::Unknown));
                }
                return Err(Unresolved::Unknown);
            }
            if !args.is_empty() {
                return Err(Unresolved::Unknown);
            }
            // The reader of a record of a set has no one owner model.
            let owner = resolve(recv, models, env).map_err(|why| match why {
                Unresolved::Set(_) => Unresolved::Union,
                why => why,
            })?;
            association_target(&owner, method, models)
        }
        _ => Err(Unresolved::Unknown),
    }
}

/// `env` with the temporary a safe navigation binds on its left side
/// (`(__safe_nav = a) && __safe_nav.b`), bound to `a`'s model.
fn safe_nav_env(left: &Expr, models: &[Model], env: &Env) -> Env {
    let mut env = env.clone();
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*left.node {
        match resolve(value, models, &env) {
            Ok(m) => {
                env.insert(name.clone(), m);
            }
            Err(_) => {
                env.remove(name);
            }
        }
    }
    env
}

fn named_model(name: &Symbol, models: &[Model], env: &Env) -> Result<String, Unresolved> {
    if let Some(m) = env.get(name) {
        return checked(m, models).unwrap_or(Err(Unresolved::Unknown));
    }
    let class = crate::naming::camelize(name.as_str());
    checked(&class, models).unwrap_or(Err(Unresolved::Unknown))
}

/// The record a one-record association reader on `owner` answers.
fn association_target(owner: &str, reader: &Symbol, models: &[Model]) -> Result<String, Unresolved> {
    let Some(m) = model(owner, models) else {
        return Err(Unresolved::Unknown);
    };
    for assoc in m.associations() {
        match assoc {
            Association::BelongsTo { name, target, polymorphic, polymorphic_targets, .. } if name == reader => {
                if *polymorphic {
                    let names: Vec<&str> = polymorphic_targets.iter().map(|t| t.0.as_str()).collect();
                    return Err(closed_set(&names, models).unwrap_or(Unresolved::Polymorphic));
                }
                return checked(target.0.as_str(), models).unwrap_or(Err(Unresolved::Unknown));
            }
            Association::HasOne { name, target, .. } if name == reader => {
                return checked(target.0.as_str(), models).unwrap_or(Err(Unresolved::Unknown));
            }
            _ => {}
        }
    }
    Err(Unresolved::Unknown)
}

/// The model of each element of a collection expression.
fn element_model(coll: &Expr, models: &[Model], env: &Env) -> Option<String> {
    let from_ty = coll.ty.as_ref().and_then(|t| element_of_ty(t, models));
    if from_ty.is_some() {
        return from_ty;
    }
    match &*coll.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            // A local the template bound to one record is no collection.
            if env.contains_key(name) { None } else { plural_model(name, models) }
        }
        ExprNode::BoolOp { op: crate::expr::BoolOpKind::And, left, right, .. } => {
            element_model(right, models, &safe_nav_env(left, models, env))
        }
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            plural_model(method, models)
        }
        ExprNode::Send { recv: Some(recv), method, block: None, .. } => {
            // A relation step on the model constant (`Widget.all`) is
            // where the chain starts: the constant itself answers no
            // collection, so it is checked before recursing.
            if let ExprNode::Const { path } = &*recv.node {
                let name = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
                return (model(&name, models).is_some() && RELATION_STEPS.contains(&method.as_str()))
                    .then_some(name);
            }
            if RELATION_STEPS.contains(&method.as_str()) {
                return element_model(recv, models, env);
            }
            let owner = resolve(recv, models, env).ok()?;
            let m = model(&owner, models)?;
            m.associations().find_map(|a| match a {
                Association::HasMany { name, target, .. } | Association::HasAndBelongsToMany { name, target, .. }
                    if name == method =>
                {
                    Some(target.0.as_str().to_string())
                }
                _ => None,
            })
        }
        _ => None,
    }
}

fn element_of_ty(ty: &Ty, models: &[Model]) -> Option<String> {
    let class = |t: &Ty| match t {
        Ty::Class { id, .. } if model(id.0.as_str(), models).is_some() => Some(id.0.as_str().to_string()),
        _ => None,
    };
    match ty {
        Ty::Array { elem } => class(elem),
        Ty::Relation { of } => model(of.0.as_str(), models).map(|_| of.0.as_str().to_string()),
        Ty::Class { args, .. } if args.len() == 1 => class(&args[0]),
        Ty::Union { variants } => {
            let non_nil: Vec<&Ty> = variants.iter().filter(|t| !matches!(t, Ty::Nil)).collect();
            match non_nil.as_slice() {
                [one] => element_of_ty(one, models),
                _ => None,
            }
        }
        _ => None,
    }
}

fn plural_model(name: &Symbol, models: &[Model]) -> Option<String> {
    let class = crate::naming::camelize(&crate::naming::singularize(name.as_str()));
    model(&class, models).map(|_| class)
}

#[cfg(test)]
mod tests {
    use super::{model_partial_path, record_sites};
    use std::path::PathBuf;

    /// The models each `json.partial!(record)` site of `view` may be,
    /// before the analyzer types anything, so only the template's own
    /// evidence (`element_model`, `resolve`) is in play.
    fn sites_of(view: &str) -> Vec<Result<Vec<String>, String>> {
        let files: crate::vfs::FileTree = [
            (
                "db/schema.rb",
                "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
            ),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
            ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
            ("app/views/widgets/_widget.json.jbuilder", "json.name widget.name\n"),
            ("app/views/widgets/index.json.jbuilder", view),
        ]
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
        let app = crate::ingest::ingest_app_from_tree(files).expect("ingest");
        let v = app.views.iter().find(|v| v.name.as_str().ends_with("index")).expect("the view");
        record_sites(&v.body, &app.models)
            .into_values()
            .map(|r| r.map(|m| m.names).map_err(|why| why.detail()))
            .collect()
    }

    #[test]
    fn a_block_over_a_model_constants_finder_binds_its_parameter_to_the_model() {
        for collection in ["Widget.all", "Widget.where(size: 3)", "Widget.includes(:gadgets).to_a"] {
            let sites = sites_of(&format!("json.array!({collection}) do |row|\n  json.partial!(row)\nend\n"));
            assert_eq!(sites, vec![Ok(vec!["Widget".to_string()])], "{collection}");
        }
    }

    #[test]
    fn the_partial_path_takes_the_controller_namespace_as_action_view_merges_it() {
        assert_eq!(model_partial_path("Gadget", "widgets").0, "gadgets/gadget");
        assert_eq!(model_partial_path("Gadget", "api/widgets").0, "api/gadgets/gadget");
        assert_eq!(model_partial_path("Gadget", "api/v1/widgets").0, "api/v1/gadgets/gadget");
        assert_eq!(model_partial_path("Admin::Gadget", "widgets").0, "admin/gadgets/gadget");
        assert_eq!(model_partial_path("Admin::Gadget", "admin/widgets").0, "admin/gadgets/gadget");
        assert_eq!(model_partial_path("Admin::Gadget", "admin/widgets").1.as_str(), "gadget");
    }
}
