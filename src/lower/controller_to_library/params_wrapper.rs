//! Rails' ParamsWrapper, decided per controller when the app is lowered.
//!
//! Rails wraps a JSON request body under the controller's model name
//! before the action's callbacks run (actionpack
//! `ActionController::ParamsWrapper`): a client posting
//! `{"title": …, "body": …}` to `ArticlesController#create` gets
//! `params[:article] = {"title" => …, "body" => …}` beside the top-level
//! keys, so the scaffold's `params.expect(article: …)` finds them.
//!
//! Everything Rails decides at runtime here is decidable from the source:
//! whether wrapping is on (the app default, then each `wrap_parameters`
//! call down the controller's ancestry), the key (an explicit name, else
//! the model's, else the controller's singular name), and which body
//! keys are copied (an explicit `include:`, else the model's attribute
//! names, else every key but `exclude:` and Rails' own three). The
//! controller lowering turns the answer into one call at the head of
//! `process_action`; `Params.wrap` (runtime/ruby/params.rb) does the
//! per-request part: is the body JSON, is the key already there.

use crate::dialect::{Controller, ControllerBodyItem, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::ClassId;
use crate::naming::{camelize, singularize, snake_case};
use crate::schema::Schema;

/// The wrapping a controller's requests get.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct WrapperSpec {
    /// The key the body is copied under (`"article"`).
    pub key: String,
    /// Copy only these body keys (`include:`, or the model's attribute
    /// names). `None` copies every key but `exclude`.
    pub include: Option<Vec<String>>,
    /// Keys left out when `include` is `None`, beside Rails' own
    /// `authenticity_token _method utf8`.
    pub exclude: Vec<String>,
}

/// One `wrap_parameters` call's options, as Rails' `Options.from_hash`
/// holds them.
#[derive(Debug, Clone, Default)]
struct Options {
    json: bool,
    name: Option<String>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    model: Option<ClassId>,
}

/// The wrapping for `controller`, or `None` when Rails would not wrap
/// its JSON requests — or when a `wrap_parameters` call is in a form
/// read here not at all and no later call makes the JSON format known
/// again (a child `wrap_parameters false` or `format:` replaces the
/// ancestor's options in Rails, so that controller stays decidable).
pub(super) fn wrapper_spec(
    controller: &Controller,
    ancestors: &[&Controller],
    models: &[Model],
    schema: Option<&Schema>,
    app_default: bool,
) -> Option<WrapperSpec> {
    let mut opts = Options { json: app_default, ..Options::default() };
    // An unreadable call leaves the effective format unknown until a
    // later `false` or `format:` replaces it (Rails' own carry rule).
    let mut json_known = true;
    for c in ancestors.iter().copied().chain(std::iter::once(controller)) {
        for item in &c.body {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
                continue;
            };
            if method.as_str() != "wrap_parameters" {
                continue;
            }
            let Some(next) = apply_call(&opts, args) else {
                json_known = false;
                continue;
            };
            if call_sets_json_format(args) {
                json_known = true;
            }
            opts = next;
        }
    }
    if !json_known {
        return None;
    }
    if !opts.json {
        return None;
    }
    let class_name = controller.name.0.as_str();
    let model = opts
        .model
        .clone()
        .and_then(|id| models.iter().find(|m| m.name == id))
        .or_else(|| default_model(class_name, models));
    let key = match (&opts.name, model) {
        (Some(name), _) => name.clone(),
        (None, Some(m)) => snake_case(m.name.0.as_str().rsplit("::").next().unwrap_or("")),
        (None, None) => singularize(&controller_name(class_name)),
    };
    let include = match (&opts.include, &opts.exclude, model) {
        (Some(list), _, _) => Some(list.clone()),
        (None, None, Some(m)) => {
            let names = attribute_names(m, schema);
            (!names.is_empty()).then_some(names)
        }
        _ => None,
    };
    Some(WrapperSpec { key, include, exclude: opts.exclude.clone().unwrap_or_default() })
}

/// Whether a `wrap_parameters(...)` call's arguments are a form this
/// module reads, so `check` does not list it as an unrecognized macro.
pub fn is_recognized_wrap_parameters_call(args: &[Expr]) -> bool {
    apply_call(&Options::default(), args).is_some()
}

/// Whether this call replaces the JSON-format half of the options in
/// force: `wrap_parameters false`, or any form that names `format:`.
/// A bare name / model / `include:` leaves the prior format (or the
/// unknown state after an unreadable ancestor) alone.
fn call_sets_json_format(args: &[Expr]) -> bool {
    let Some((first, rest)) = args.split_first() else {
        return false;
    };
    if matches!(&*first.node, ExprNode::Lit { value: Literal::Bool { value: false } }) {
        return true;
    }
    let entries = match &*first.node {
        ExprNode::Hash { entries, .. } => Some(entries.as_slice()),
        _ => match rest {
            [extra] => match &*extra.node {
                ExprNode::Hash { entries, .. } => Some(entries.as_slice()),
                _ => None,
            },
            _ => None,
        },
    };
    entries.is_some_and(|entries| {
        entries.iter().any(|(key, _)| {
            matches!(
                &*key.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "format"
            )
        })
    })
}

/// `wrap_parameters(...)` — Rails' `ClassMethods#wrap_parameters`: only
/// the format carries over from the options in force; a name, a model,
/// `include:` and `exclude:` are this call's alone.
fn apply_call(prev: &Options, args: &[Expr]) -> Option<Options> {
    let mut next = Options { json: prev.json, ..Options::default() };
    let (first, rest) = args.split_first()?;
    match &*first.node {
        ExprNode::Lit { value: Literal::Bool { value: false } } => next.json = false,
        ExprNode::Hash { entries, .. } => read_options(&mut next, entries)?,
        ExprNode::Lit { value: Literal::Sym { value } } => next.name = Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => next.name = Some(value.clone()),
        ExprNode::Const { path } => {
            next.model = Some(ClassId(crate::ident::Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::").as_str(),
            )))
        }
        _ => return None,
    }
    match rest {
        [] => {}
        [extra] => {
            let ExprNode::Hash { entries, .. } = &*extra.node else { return None };
            read_options(&mut next, entries)?;
        }
        _ => return None,
    }
    Some(next)
}

fn read_options(opts: &mut Options, entries: &[(Expr, Expr)]) -> Option<()> {
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match key.as_str() {
            "format" => opts.json = names_of(v)?.iter().any(|f| f == "json"),
            "name" => opts.name = Some(names_of(v)?.into_iter().next()?),
            "include" => opts.include = Some(names_of(v)?),
            "exclude" => opts.exclude = Some(names_of(v)?),
            _ => return None,
        }
    }
    Some(())
}

/// A Symbol, a String, or an Array of them, as Strings.
fn names_of(e: &Expr) -> Option<Vec<String>> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![value.as_str().to_string()]),
        ExprNode::Lit { value: Literal::Str { value } } => Some(vec![value.clone()]),
        ExprNode::Array { elements, .. } => {
            elements.iter().map(|el| names_of(el).and_then(|v| v.into_iter().next())).collect()
        }
        _ => None,
    }
}

/// Rails' `_default_wrap_model`: `Admin::ArticlesController` tries
/// `Admin::Article`, then drops the namespace part before the class
/// (`Article`), until a model is found or nothing is left to drop.
fn default_model<'a>(class_name: &str, models: &'a [Model]) -> Option<&'a Model> {
    let base = class_name.strip_suffix("Controller")?;
    let mut parts: Vec<String> = base.split("::").map(|s| s.to_string()).collect();
    let last = parts.pop()?;
    parts.push(camelize(&singularize(&snake_case(&last))));
    let mut model_name = parts.join("::");
    loop {
        if let Some(m) = models.iter().find(|m| m.name.0.as_str() == model_name) {
            return Some(m);
        }
        let mut namespaces: Vec<&str> = model_name.split("::").collect();
        if namespaces.len() >= 2 {
            let at = namespaces.len() - 2;
            namespaces.remove(at);
        }
        let next = namespaces.join("::");
        if next == model_name {
            return None;
        }
        model_name = next;
    }
}

/// `controller_name`: the class name without its namespace and
/// `Controller` suffix, underscored.
fn controller_name(class_name: &str) -> String {
    let leaf = class_name.rsplit("::").next().unwrap_or(class_name);
    snake_case(leaf.strip_suffix("Controller").unwrap_or(leaf))
}

/// The model's `attribute_names` plus what Rails' ParamsWrapper adds to
/// them: store accessors, attribute aliases, and `<assoc>_attributes`
/// for nested attributes. The columns come from the schema, in its
/// order; the rest from the class body's literal declarations.
fn attribute_names(model: &Model, schema: Option<&Schema>) -> Vec<String> {
    let mut names: Vec<String> = schema
        .and_then(|s| s.tables.get(&model.table.0))
        .map(|t| t.columns.iter().map(|c| c.name.as_str().to_string()).collect())
        .unwrap_or_default();
    if names.is_empty() {
        return names;
    }
    let mut stored = Vec::new();
    let mut aliases = Vec::new();
    let mut nested = Vec::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        let syms: Vec<String> = args
            .iter()
            .filter_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                _ => None,
            })
            .collect();
        match method.as_str() {
            "store_accessor" => stored.extend(syms.into_iter().skip(1)),
            "store" => {
                for a in args {
                    let ExprNode::Hash { entries, .. } = &*a.node else { continue };
                    for (k, v) in entries {
                        if matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "accessors") {
                            stored.extend(names_of(v).unwrap_or_default());
                        }
                    }
                }
            }
            "alias_attribute" => aliases.extend(syms.into_iter().take(1)),
            "accepts_nested_attributes_for" => {
                nested.extend(syms.into_iter().map(|s| format!("{s}_attributes")))
            }
            _ => {}
        }
    }
    names.extend(stored);
    names.extend(aliases);
    names.extend(nested);
    names
}

/// `@params = Params.wrap(@params, request, "<key>", <include?>,
/// [<include>], [<exclude>])` - the head of the generated
/// `process_action`. The runtime half decides per request (a JSON body,
/// the key not already present) and copies from the request's BODY
/// params, never the query string or the path.
pub(super) fn wrap_statement(spec: &WrapperSpec) -> Expr {
    use crate::ident::Symbol;
    let span = crate::span::Span::synthetic();
    let str_lit = |v: &str| Expr::new(span, ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
    let str_array = |items: &[String]| {
        Expr::new(
            span,
            ExprNode::Array {
                elements: items.iter().map(|s| str_lit(s)).collect(),
                style: crate::expr::ArrayStyle::default(),
            },
        )
    };
    let params_ivar = || Expr::new(span, ExprNode::Ivar { name: Symbol::from("params") });
    let request = Expr::new(
        span,
        ExprNode::Send { recv: None, method: Symbol::from("request"), args: vec![], block: None, parenthesized: false },
    );
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Params")] })),
            method: Symbol::from("wrap"),
            args: vec![
                params_ivar(),
                request,
                str_lit(&spec.key),
                Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: spec.include.is_some() } }),
                str_array(spec.include.as_deref().unwrap_or(&[])),
                str_array(&spec.exclude),
            ],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Ivar { name: Symbol::from("params") },
            value: call,
        },
    )
}
