//! The `acts_as_tenant` gem (1.0.x), expanded where it is used.
//!
//! The gem is row-level multitenancy built from ordinary Rails pieces, so
//! each macro is *replaced* by the declarations it stands for (the route
//! `discard` and `acts_as_votable` take), and the lowerings those already
//! have carry it to every target:
//!
//! ```ruby
//! acts_as_tenant :account            # in a model, becomes
//! belongs_to :account
//! default_scope { where(account_id: ActsAsTenant.current_tenant.id) if ActsAsTenant.current_tenant }
//! before_validation(on: :create) { self.account_id = ActsAsTenant.current_tenant.id if ActsAsTenant.current_tenant }
//!
//! validates_uniqueness_to_tenant :name   # becomes
//! validates :name, uniqueness: { scope: :account_id }
//! ```
//!
//! That is the gem's `ModelExtensions#acts_as_tenant`: `belongs_to
//! tenant, **options` (required unless `optional: true`); a
//! `default_scope` that filters on the foreign key while a current tenant
//! is set and reads every row otherwise; and a `before_validation ...
//! on: :create` that writes the current tenant's key into the foreign key,
//! over any value the record was given. The foreign key is
//! `foreign_key:` or `"#{tenant}_id"`, the key it holds `primary_key:` or
//! `id`, as in the gem.
//!
//! `ActsAsTenant` itself becomes an `ActiveSupport::CurrentAttributes`
//! class (the gem keeps its state in one, `ActsAsTenant::Current`), so it
//! is per request and per thread, and reset between requests, through the
//! `CurrentAttributes` lowering: `current_tenant`, `current_tenant=`,
//! `with_tenant(t) { }` and `without_tenant { }`, which restore the
//! previous tenant on the way out. In a controller,
//! `set_current_tenant_through_filter` becomes the private
//! `set_current_tenant(t)` it includes, and `current_tenant` (the gem's
//! `TenantHelper`) is defined beside it.
//!
//! # Boundary
//!
//! * The gem must be in `Gemfile.lock`, the tenant class must be a model
//!   of the app, the model's table must carry the foreign key, and every
//!   model must name the same tenant class (the gem keeps one current
//!   tenant). Anything else is refused, and the macro stays.
//! * Options: `foreign_key:`, `primary_key:`, `class_name:`, `optional:`,
//!   and the `belongs_to` options the gem forwards (`inverse_of:`,
//!   `counter_cache:`, `touch:`), as literals. `has_global_records:`,
//!   `through:`, `polymorphic:`, `foreign_type:`, a scope argument and any
//!   other key are refused, as is an `ActsAsTenant.configure` that sets
//!   `require_tenant` or `pkey` (the scope would read rows the gem refuses
//!   to, or the wrong key).
//! * `validates_uniqueness_to_tenant` takes literal `scope:`,
//!   `case_sensitive:`, `allow_nil:`, `allow_blank:` and `message:`;
//!   anything else is refused.
//! * A model that also declares `belongs_to :<tenant>` keeps whichever
//!   declaration comes later, as Rails does; the scope and the stamp use
//!   the gem's key either way. Rails runs both declarations' presence
//!   validations, so it reports `must exist` twice where this reports it
//!   once.
//! * Not modelled: the `TenantIsImmutable` raise when a persisted
//!   record's tenant key is reassigned, the check that a record's other
//!   `belongs_to` targets belong to the current tenant, carrying the
//!   tenant into an Active Job (recorded on the survey ledger when the app
//!   has jobs), `test_tenant`/`default_tenant`/`with_mutable_tenant`,
//!   `current_tenant` in views, and `set_current_tenant_by_subdomain`
//!   (which stays the reported controller-macro gap).

use crate::dialect::{ControllerBodyItem, LibraryClass, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};

use super::{survey, IngestError, IngestResult};

const GEM: &str = "acts_as_tenant";
/// The class the gem's module becomes.
const STATE_CLASS: &str = "ActsAsTenant";

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_const_path(s: &str) -> bool {
    !s.is_empty()
        && s.split("::").all(|seg| {
            let mut chars = seg.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Indices and arguments of the receiverless `name` calls in a model body.
fn macro_calls(model: &Model, name: &str) -> Vec<(usize, Vec<Expr>)> {
    model
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
                return None;
            };
            (method.as_str() == name).then(|| (i, args.clone()))
        })
        .collect()
}

/// A literal option value, as Ruby source.
fn literal_source(expr: &Expr) -> Option<String> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } if is_ident(value.as_str()) => Some(format!(":{}", value.as_str())),
        ExprNode::Lit { value: Literal::Str { value } } => Some(ruby_string(value)),
        ExprNode::Lit { value: Literal::Bool { value } } => Some(value.to_string()),
        _ => None,
    }
}

/// `s` as a double-quoted Ruby literal that reads back as exactly `s`.
/// Rust's `{:?}` is not one: it leaves `#` alone, so a single-quoted
/// `'taken #{x}'` (literal text, which Rails keeps verbatim) would come
/// back as an interpolation. `#` is escaped, which covers `#{`, `#@` and
/// `#$`.
fn ruby_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A symbol or string literal's text.
fn name_of(expr: &Expr) -> Option<String> {
    match &*expr.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.to_string()),
        _ => None,
    }
}

/// `key: value` pairs of a trailing keyword hash; `None` when a key is
/// not a literal symbol.
fn options(arg: &Expr) -> Option<Vec<(String, Expr)>> {
    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    entries
        .iter()
        .map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some((value.as_str().to_string(), v.clone())),
            _ => None,
        })
        .collect()
}

/// One `acts_as_tenant` call, read.
struct Tenancy {
    /// The association name (`:account`).
    tenant: String,
    /// The tenant model (`class_name:` or the association name camelized).
    class: String,
    fkey: String,
    pkey: String,
    /// The `belongs_to` options, as Ruby source (`, optional: true`).
    belongs_to_options: String,
}

fn read_tenancy(args: &[Expr]) -> Result<Tenancy, String> {
    let (positional, opts): (&[Expr], Vec<(String, Expr)>) = match args.split_last() {
        Some((last, rest)) if matches!(&*last.node, ExprNode::Hash { .. }) => {
            (rest, options(last).ok_or("an option key is not a literal symbol")?)
        }
        _ => (args, Vec::new()),
    };
    let tenant = match positional {
        [] => "account".to_string(),
        [t] => match &*t.node {
            ExprNode::Lit { value: Literal::Sym { value } } if is_ident(value.as_str()) => value.as_str().to_string(),
            _ => return Err("the tenant must be a literal symbol".into()),
        },
        _ => return Err("a scope argument to the tenant association is not modelled".into()),
    };
    let mut fkey = format!("{tenant}_id");
    let mut pkey = "id".to_string();
    let mut class = crate::naming::camelize(&tenant);
    let mut belongs_to_options = String::new();
    for (key, value) in &opts {
        match key.as_str() {
            "foreign_key" | "primary_key" | "class_name" => {
                let name = name_of(value).ok_or(format!("`{key}:` must be a literal"))?;
                let ok = if key == "class_name" { is_const_path(&name) } else { is_ident(&name) };
                if !ok {
                    return Err(format!("`{key}: {name:?}` is not a name"));
                }
                match key.as_str() {
                    "foreign_key" => fkey = name.clone(),
                    "primary_key" => pkey = name.clone(),
                    _ => class = name.clone(),
                }
                belongs_to_options.push_str(&format!(", {key}: {name:?}"));
            }
            "optional" | "inverse_of" | "counter_cache" | "touch" => {
                let src = literal_source(value).ok_or(format!("`{key}:` must be a literal"))?;
                belongs_to_options.push_str(&format!(", {key}: {src}"));
            }
            "has_global_records" | "through" | "polymorphic" | "foreign_type" => {
                return Err(format!("`{key}:` is not modelled"));
            }
            other => return Err(format!("`{other}:` is not an option this expansion reproduces")),
        }
    }
    Ok(Tenancy { tenant, class, fkey, pkey, belongs_to_options })
}

fn model_source(t: &Tenancy) -> String {
    let Tenancy { tenant, fkey, pkey, belongs_to_options, .. } = t;
    let current = format!("{STATE_CLASS}.current_tenant");
    format!(
        "class ActsAsTenantSynth < ApplicationRecord\n\
         \x20 belongs_to :{tenant}{belongs_to_options}\n\
         \x20 default_scope {{ where({fkey}: {current}.{pkey}) if {current} }}\n\
         \x20 before_validation(on: :create) {{ self.{fkey} = {current}.{pkey} if {current} }}\n\
         end\n"
    )
}

/// `validates_uniqueness_to_tenant :f, **opts` as the `validates` call
/// the gem makes (`validates_uniqueness_of f, scope: [*scope, fkey]`).
fn uniqueness_source(args: &[Expr], fkey: &str) -> Result<String, String> {
    let (fields, opts): (&[Expr], Vec<(String, Expr)>) = match args.split_last() {
        Some((last, rest)) if matches!(&*last.node, ExprNode::Hash { .. }) => {
            (rest, options(last).ok_or("an option key is not a literal symbol")?)
        }
        _ => (args, Vec::new()),
    };
    let mut names: Vec<String> = Vec::new();
    for f in fields {
        match &*f.node {
            ExprNode::Lit { value: Literal::Sym { value } } if is_ident(value.as_str()) => names.push(value.as_str().to_string()),
            ExprNode::Array { elements, .. } => {
                for e in elements {
                    match &*e.node {
                        ExprNode::Lit { value: Literal::Sym { value } } if is_ident(value.as_str()) => names.push(value.as_str().to_string()),
                        _ => return Err("the attributes must be literal symbols".into()),
                    }
                }
            }
            _ => return Err("the attributes must be literal symbols".into()),
        }
    }
    if names.is_empty() {
        return Err("no attribute is named".into());
    }
    let mut scope: Vec<String> = Vec::new();
    let mut rest = String::new();
    for (key, value) in &opts {
        match key.as_str() {
            "scope" => {
                let items: Vec<&Expr> = match &*value.node {
                    ExprNode::Array { elements, .. } => elements.iter().collect(),
                    _ => vec![value],
                };
                for item in items {
                    match &*item.node {
                        ExprNode::Lit { value: Literal::Sym { value } } if is_ident(value.as_str()) => scope.push(value.as_str().to_string()),
                        _ => return Err("`scope:` must be literal symbols".into()),
                    }
                }
            }
            "case_sensitive" | "allow_nil" | "allow_blank" | "message" => {
                let src = literal_source(value).ok_or(format!("`{key}:` must be a literal"))?;
                rest.push_str(&format!(", {key}: {src}"));
            }
            other => return Err(format!("`{other}:` is not an option this expansion reproduces")),
        }
    }
    scope.push(fkey.to_string());
    let scope_src = match scope.as_slice() {
        [one] => format!(":{one}"),
        many => format!("[{}]", many.iter().map(|s| format!(":{s}")).collect::<Vec<_>>().join(", ")),
    };
    let attrs = names.iter().map(|n| format!(":{n}")).collect::<Vec<_>>().join(", ");
    Ok(format!(
        "class ActsAsTenantSynth < ApplicationRecord\n  validates {attrs}, uniqueness: {{ scope: {scope_src}{rest} }}\nend\n"
    ))
}

fn synthesize(src: &str, what: &str, app: &crate::App) -> Option<Model> {
    let (parsed, diags) = super::prism::scope(|| {
        super::model::ingest_model(src.as_bytes(), "<acts_as_tenant>", &app.schema, &Default::default())
    });
    match parsed {
        Ok(Some(m)) if diags.is_empty() => Some(m),
        Ok(_) => {
            survey::record_synthesis_failure("<acts_as_tenant>", what, &diags);
            None
        }
        Err(err) => {
            survey::record(&err);
            None
        }
    }
}

/// Put `synth`'s items where the macro at `at` stood, and drop the macro.
///
/// The gem's `belongs_to` is a second declaration when the model declares
/// the association itself, and in Rails the later one's reflection wins
/// (the scope and the stamp keep the gem's key either way): a model's own
/// `belongs_to` after the macro is kept and the gem's dropped; one before
/// it is replaced, in place so no later index moves, by the gem's.
fn splice(model: &mut Model, synth: Model, at: usize) {
    let own = |body: &[ModelBodyItem], name: &str| -> Vec<usize> {
        body.iter()
            .enumerate()
            .filter(|(_, it)| matches!(it, ModelBodyItem::Association { assoc, .. } if assoc.name().as_str() == name))
            .map(|(i, _)| i)
            .collect()
    };
    let mut additions: Vec<ModelBodyItem> = Vec::new();
    for item in synth.body {
        let ModelBodyItem::Association { assoc, .. } = &item else {
            additions.push(item);
            continue;
        };
        let declared = own(&model.body, assoc.name().as_str());
        if declared.iter().any(|&i| i > at) {
            continue;
        }
        match declared.last() {
            Some(&i) => model.body[i] = item,
            None => additions.push(item),
        }
    }
    model.body.remove(at);
    let pos = at.min(model.body.len());
    model.body.splice(pos..pos, additions);
}

/// `ActsAsTenant`: the gem's per-request state, as a `CurrentAttributes`
/// class the `current_attributes` lowering turns into real methods.
fn state_source(tenant_class: &str) -> String {
    let swap = |value: &str| {
        format!(
            "    old_tenant = {STATE_CLASS}.current_tenant\n\
             \x20   {STATE_CLASS}.current_tenant = {value}\n\
             \x20   begin\n\
             \x20     yield\n\
             \x20   ensure\n\
             \x20     {STATE_CLASS}.current_tenant = old_tenant\n\
             \x20   end\n"
        )
    };
    format!(
        "class {STATE_CLASS} < ActiveSupport::CurrentAttributes\n\
         \x20 attribute :current_tenant #: {tenant_class}\n\
         \n\
         \x20 def self.with_tenant(tenant)\n{}  end\n\
         \n\
         \x20 def self.without_tenant\n{}  end\n\
         end\n",
        swap("tenant"),
        swap("nil"),
    )
}

const CONTROLLER_METHODS: &str = "  def set_current_tenant(current_tenant_object)\n    ActsAsTenant.current_tenant = current_tenant_object\n  end\n";
const HELPER_METHOD: &str = "  def current_tenant\n    ActsAsTenant.current_tenant\n  end\n";

/// Expand the gem's macros. `initializers` are `config/initializers`'
/// `(path, text)`, read for an `ActsAsTenant.configure` this does not model.
pub(crate) fn expand_acts_as_tenant(app: &mut crate::App, initializers: &[(String, String)]) -> IngestResult<()> {
    if !app.gem_lock.as_ref().is_some_and(|l| l.has(GEM)) {
        return Ok(());
    }
    let uses_macro = app.models.iter().any(|m| !macro_calls(m, "acts_as_tenant").is_empty());
    if !uses_macro {
        return Ok(());
    }

    // `ActsAsTenant.configure` settings that change what the scope reads.
    for (path, text) in initializers.iter().filter(|(_, t)| t.contains("ActsAsTenant")) {
        for setting in ["require_tenant", "pkey"] {
            if text.contains(&format!("{setting} =")) || text.contains(&format!("{setting}=")) {
                survey::continue_or_fail(IngestError::Unsupported {
                    file: path.clone(),
                    message: format!("ActsAsTenant setting `{setting}` is not modelled (acts_as_tenant)"),
                })?;
            }
        }
    }

    let mut tenant_class: Option<String> = None;
    // Models whose `acts_as_tenant` was read (expanded or refused): their
    // `validates_uniqueness_to_tenant` calls are answered there.
    let mut scoped: Vec<usize> = Vec::new();
    for mi in 0..app.models.len() {
        let name = app.models[mi].name.0.as_str().to_string();
        let refuse = |why: String| {
            survey::continue_or_fail(IngestError::Unsupported {
                file: name.clone(),
                message: format!("`acts_as_tenant` on {name} is not supported: {why}"),
            })
        };
        let calls = macro_calls(&app.models[mi], "acts_as_tenant");
        let Some((at, args)) = calls.first().cloned() else { continue };
        scoped.push(mi);
        if calls.len() > 1 {
            refuse("`acts_as_tenant` is called more than once".into())?;
            continue;
        }
        let t = match read_tenancy(&args) {
            Ok(t) => t,
            Err(why) => {
                refuse(why)?;
                continue;
            }
        };
        if !app.models.iter().any(|m| m.name.0.as_str() == t.class) {
            refuse(format!("the tenant class `{}` is not a model of the app", t.class))?;
            continue;
        }
        if let Some(seen) = &tenant_class {
            if *seen != t.class {
                refuse(format!("its tenant `{}` differs from `{seen}`; the gem keeps one current tenant", t.class))?;
                continue;
            }
        }
        let has_fkey = app
            .schema
            .tables
            .get(&app.models[mi].table.0)
            .is_some_and(|tbl| tbl.columns.iter().any(|c| c.name.as_str() == t.fkey));
        if !has_fkey {
            refuse(format!("table `{}` has no column `{}` (the tenant foreign key)", app.models[mi].table.0.as_str(), t.fkey))?;
            continue;
        }

        // Uniqueness validations first, while the macro's index is known.
        let uniques = macro_calls(&app.models[mi], "validates_uniqueness_to_tenant");
        let mut replaced: Vec<(usize, Model)> = Vec::new();
        for (ui, uargs) in uniques {
            match uniqueness_source(&uargs, &t.fkey) {
                Ok(src) => {
                    if let Some(synth) = synthesize(&src, &format!("validates_uniqueness_to_tenant on `{name}`"), app) {
                        replaced.push((ui, synth));
                    }
                }
                Err(why) => refuse(format!("`validates_uniqueness_to_tenant`: {why}"))?,
            }
        }
        let Some(synth) = synthesize(&model_source(&t), &format!("acts_as_tenant on `{name}`"), app) else { continue };
        tenant_class = Some(t.class.clone());
        // Back to front, so each index still names its call.
        replaced.push((at, synth));
        replaced.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
        for (i, synth) in replaced {
            splice(&mut app.models[mi], synth, i);
        }
    }
    // A `validates_uniqueness_to_tenant` in a model with no expanded
    // tenancy: the gem raises `ModelNotScopedByTenant`.
    for (mi, m) in app.models.iter().enumerate() {
        if !scoped.contains(&mi) && !macro_calls(m, "validates_uniqueness_to_tenant").is_empty() {
            let name = m.name.0.as_str().to_string();
            survey::continue_or_fail(IngestError::Unsupported {
                file: name.clone(),
                message: format!("`validates_uniqueness_to_tenant` on {name} is not supported: the model is not scoped by an expanded `acts_as_tenant`"),
            })?;
        }
    }

    let Some(tenant_class) = tenant_class else { return Ok(()) };
    if !app.library_classes.iter().any(|lc| lc.name.0.as_str() == STATE_CLASS) {
        let src = state_source(&tenant_class);
        let (parsed, diags) = super::prism::scope(|| crate::ingest::ingest_library_classes(src.as_bytes(), "<acts_as_tenant>"));
        match parsed {
            Ok(classes) if diags.is_empty() => {
                app.library_classes.extend(classes);
                super::current_attributes::lower_current_attributes(app);
            }
            Ok(_) => survey::record_synthesis_failure("<acts_as_tenant>", "the ActsAsTenant state class", &diags),
            Err(err) => survey::record(&err),
        }
    }

    // `set_current_tenant_through_filter`: the gem's `Filter` mixin and
    // its `TenantHelper`.
    for ci in 0..app.controllers.len() {
        let pos = app.controllers[ci].body.iter().position(|item| {
            let ControllerBodyItem::Unknown { expr, .. } = item else { return false };
            matches!(&*expr.node, ExprNode::Send { recv: None, method, args, block: None, .. }
                if method.as_str() == "set_current_tenant_through_filter" && args.is_empty())
        });
        let Some(pos) = pos else { continue };
        let defines = |c: &crate::dialect::Controller, n: &str| {
            c.body.iter().any(|item| matches!(item, ControllerBodyItem::Action { action, .. } if action.name.as_str() == n))
        };
        let mut src = String::new();
        if !defines(&app.controllers[ci], "set_current_tenant") {
            src.push_str(CONTROLLER_METHODS);
        }
        if !defines(&app.controllers[ci], "current_tenant") {
            src.push_str(HELPER_METHOD);
        }
        let controller = &mut app.controllers[ci];
        if src.is_empty() || super::controller_macro_synth::append_private_actions(controller, "<acts_as_tenant>", &src) {
            controller.body.remove(pos);
        }
    }

    // Active Job: the gem carries the current tenant into a job.
    let has_jobs = app.library_classes.iter().any(|lc: &LibraryClass| {
        lc.parent.as_ref().is_some_and(|p| matches!(p.0.as_str(), "ApplicationJob" | "ActiveJob::Base"))
    });
    if has_jobs {
        survey::record(&IngestError::Unsupported {
            file: "Gemfile.lock".into(),
            message: "acts_as_tenant: the current tenant is not carried into Active Job jobs (a job runs with no tenant)".into(),
        });
    }
    Ok(())
}
