//! `t("key", …)` in a template or a controller, and `I18n.t("key", …)`
//! anywhere → the String Rails' I18n answers from the app's locale
//! (`crate::i18n`).
//!
//! The lookup happens at compile time: a literal key (or a view's lazy
//! `.key`), literal `scope:` / `default:`, and interpolation values that
//! are any expression. `count:` keeps the choice between plural forms
//! for run time. A template's `_html` key renders unescaped with its
//! values escaped, as `HtmlSafeTranslation` does. A key the locale lacks
//! answers as Rails does in the running environment
//! (`raise_on_missing_translations`, read per environment at ingest).
//! A controller's `t` is `AbstractController::Translation#translate`: a
//! lazy `.key` reads `"<controller_path>.<action_name>.key"` with
//! `:"<controller_path>.key"` as its first default, `action_name` read
//! when it runs, and the rest is `I18n.t`'s. Its `_html` keys stay an
//! error: Rails returns them html-safe, and no String here carries that
//! mark on to the view that renders them.
//! Anything else (a dynamic key, `locale:`, a setting not readable at
//! compile time) is an error in analyze; only what `parse` and
//! `Catalog::resolve` accept is typed, and only that folds.

use crate::app::App;
use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
use crate::i18n::{Catalog, Fallback, Piece, Translate, Translation, Variant};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// A translate call's lookup, and the expressions its `count:` and
/// interpolation values read.
#[derive(Clone)]
pub(crate) struct Call {
    pub lookup: Translate,
    count: Option<Expr>,
    values: Vec<(String, Expr)>,
    /// `count` and the value names, in source order.
    order: Vec<String>,
    /// A key with run-time text: its parts after the literal prefix
    /// (`lookup.key`), and the literal text it ends in.
    dynamic: Option<(Vec<InterpPart>, String)>,
    /// `locale:`, when the call names one.
    locale: Option<Locale>,
}

/// A `locale:` option: a literal (`:pl`, `"pl"`), or a value read when
/// the call runs.
#[derive(Clone)]
pub(crate) enum Locale {
    Literal(String),
    Dynamic(Expr),
}

/// Most translations one run-time key may reach before folding each
/// stops being worth the branch.
const MAX_REACH: usize = 400;

/// How a translate call folds: one translation, or one per key a
/// run-time key can reach, the subtrees it can name, and what it answers
/// for any other key.
#[derive(Clone, Debug, PartialEq, Hash)]
pub(crate) enum Plan {
    Static(Translation),
    Dynamic { arms: Vec<(String, Translation)>, subtrees: Vec<String>, otherwise: Translation, otherwise_html: bool },
}

/// Every way `call` can be looked up at run time, each with its plan:
/// one per locale it can run in (the `locale:` it names, else each
/// available locale) and per environment whose fallbacks differ.
pub(crate) fn plans(catalog: &Catalog, call: &Call, in_view: bool) -> Result<Vec<(Variant, Plan)>, String> {
    let locales = match &call.locale {
        Some(Locale::Literal(l)) if !catalog.locales().contains(l) => {
            return Err(format!("`locale: :{l}`, which is not an available locale"));
        }
        Some(Locale::Literal(l)) => vec![l.clone()],
        _ => catalog.locales(),
    };
    if locales.len() > 1 {
        if let Some(why) = &catalog.available_unknown {
            return Err(format!("`config.i18n.available_locales`: {why}"));
        }
    }
    if locales.len() > 1 || catalog.fallbacks.app.is_some() || !catalog.fallbacks.envs.is_empty() {
        if let Some(why) = &catalog.fallbacks.unknown {
            return Err(format!("`config.i18n.fallbacks` {why}"));
        }
    }
    catalog
        .variants(&locales)
        .into_iter()
        .map(|v| plan(catalog, &v, call, in_view).map(|p| (v, p)))
        .collect()
}

fn plan(catalog: &Catalog, variant: &Variant, call: &Call, in_view: bool) -> Result<Plan, String> {
    let (locale, fallbacks) = (variant.locale.as_str(), variant.fallbacks.as_deref());
    let Some((_, tail)) = &call.dynamic else {
        return catalog.resolve_in(locale, fallbacks, &call.lookup).map(Plan::Static);
    };
    let base = Translate::full_key(&call.lookup, &call.lookup.key)?;
    let chain = match fallbacks {
        Some(defaults) => crate::i18n::fallback_chain(locale, defaults),
        None => vec![locale.to_string()],
    };
    let (mut keys, mut subtrees) = (Vec::new(), Vec::new());
    for reached in chain.iter().filter_map(|l| catalog.catalog_for(l)) {
        let (k, s) = reached.reach(&base, tail, call.lookup.counted);
        keys.extend(k);
        subtrees.extend(s);
    }
    keys.sort();
    keys.dedup();
    subtrees.sort();
    subtrees.dedup();
    subtrees.retain(|s| !keys.contains(s));
    if keys.len() + subtrees.len() > MAX_REACH {
        return Err(format!("a key computed at run time that can reach {} translations", keys.len() + subtrees.len()));
    }
    // Each arm is the literal key, already scoped, with the call's defaults scoped the same way.
    let mut defaults = Vec::new();
    for d in &call.lookup.defaults {
        defaults.push(match d {
            Fallback::Key(k) => Fallback::Key(Translate::full_key(&call.lookup, k)?),
            Fallback::Text(t) => Fallback::Text(t.clone()),
        });
    }
    let mut arms = Vec::new();
    for key in keys {
        let lookup = Translate { key: key.clone(), view: None, scope: None, defaults: defaults.clone(), ..call.lookup.clone() };
        arms.push((key, catalog.resolve_in(locale, fallbacks, &lookup)?));
    }
    let otherwise = if call.lookup.defaults.is_empty() {
        if let Some(why) = &catalog.missing.unknown {
            return Err(format!("a key computed at run time can be missing ({why})"));
        }
        Translation::Missing { key: String::new(), considered: Vec::new() }
    } else {
        catalog.resolve_in(locale, fallbacks, &Translate { key: "\u{0}".into(), ..call.lookup.clone() })?
    };
    // A found translation is html-safe by the key the call passed, which only its literal tail can tell.
    let otherwise_html = crate::i18n::is_html_safe_key(tail);
    if in_view && !call.lookup.defaults.is_empty() && !otherwise_html && tail.trim_matches('.').is_empty() {
        return Err("a default under a key whose end is computed at run time, which decides whether it is html-safe".into());
    }
    Ok(Plan::Dynamic { arms, subtrees, otherwise, otherwise_html })
}

/// Where a call sits, which decides what a receiver-less `t` is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Site<'a> {
    /// Anywhere else: only `I18n.t` translates.
    Other,
    /// A template (`articles/_form` when known): ActionView's `t`.
    View(Option<&'a str>),
    /// An instance method of an `ActionController::Base` controller,
    /// under its `controller_path` with `/` read as `.` (`admin.users`):
    /// AbstractController::Translation's `t`.
    Controller(&'a str),
}

impl Site<'_> {
    pub(crate) fn in_view(self) -> bool {
        matches!(self, Site::View(_))
    }
}

/// `controller_path` with `/` as `.`, as `AbstractController::Translation`
/// scopes a lazy key: `Admin::UsersController` → `admin.users`.
pub(crate) fn controller_scope(class_name: &str) -> String {
    let name = class_name.strip_suffix("Controller").unwrap_or(class_name);
    crate::naming::underscore(name).replace('/', ".")
}

/// `None` when this is not a translate call; `Err` when it is one whose
/// shape cannot be resolved at compile time.
pub(crate) fn parse(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    site: Site<'_>,
) -> Option<Result<Call, String>> {
    if !matches!(method, "t" | "translate") {
        return None;
    }
    let on_i18n = matches!(recv.map(|r| &*r.node),
        Some(ExprNode::Const { path }) if path.len() == 1 && path[0].as_str() == "I18n");
    let bare = recv.is_none() && !matches!(site, Site::Other);
    if !(on_i18n || bare) {
        return None;
    }
    if block.is_some() {
        return Some(Err("a block on the call".into()));
    }
    Some(match (site, on_i18n) {
        (Site::View(view), false) => parse_args(args, view, true),
        (Site::Controller(scope), false) => parse_controller_args(args, scope),
        _ => parse_args(args, None, false),
    })
}

/// `AbstractController::Translation#translate`: a lazy `.key` becomes
/// `"<scope>.#{action_name}.key"`, with `:"<scope>.key"` put in front of
/// any `default:` (always a list then); an `_html` key is refused.
fn parse_controller_args(args: &[Expr], scope: &str) -> Result<Call, String> {
    let lazy = args.first().and_then(literal_text).and_then(|k| k.strip_prefix('.').map(str::to_string));
    let call = match lazy {
        None => parse_args(args, None, false)?,
        Some(rest) => {
            let span = args[0].span;
            let action_name = Expr::new(span, ExprNode::Send {
                recv: None,
                method: Symbol::from("action_name"),
                args: vec![],
                block: None,
                parenthesized: false,
            });
            let key = Expr::new(span, ExprNode::StringInterp {
                parts: vec![
                    InterpPart::Text { value: format!("{scope}.") },
                    InterpPart::Expr { expr: typed(action_name) },
                    InterpPart::Text { value: format!(".{rest}") },
                ],
            });
            let mut call = parse_args(&std::iter::once(key).chain(args[1..].iter().cloned()).collect::<Vec<_>>(), None, false)?;
            call.lookup.defaults.insert(0, Fallback::Key(format!("{scope}.{rest}")));
            call.lookup.default_list = true;
            call
        }
    };
    let written = match &call.dynamic {
        Some((_, tail)) => tail.clone(),
        None => call.lookup.key.clone(),
    };
    if crate::i18n::is_html_safe_key(&written) {
        return Err("an `_html` key in a controller: Rails returns it html-safe, and that mark does not reach the view here".into());
    }
    Ok(call)
}

fn literal_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::StringInterp { parts } => parts
            .iter()
            .map(|p| match p {
                InterpPart::Text { value } => Some(value.clone()),
                InterpPart::Expr { expr } => interpolated_text(expr),
            })
            .collect(),
        _ => None,
    }
}

/// What `#{expr}` renders for a literal `expr`.
fn interpolated_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => Some(String::new()),
        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
        _ => literal_text(e),
    }
}

/// `"prefix.#{x}.tail"`: the literal text before the first run-time
/// part (which must not be empty), the parts from there on, and the
/// literal text after the last run-time part.
fn dynamic_key(key: &Expr) -> Option<(String, Vec<InterpPart>, String)> {
    let ExprNode::StringInterp { parts } = &*key.node else { return None };
    let literal = |p: &InterpPart| match p {
        InterpPart::Text { value } => Some(value.clone()),
        InterpPart::Expr { expr } => interpolated_text(expr),
    };
    let first = parts.iter().position(|p| literal(p).is_none())?;
    let last = parts.iter().rposition(|p| literal(p).is_none())?;
    let prefix: String = parts[..first].iter().filter_map(literal).collect();
    let tail: String = parts[last + 1..].iter().filter_map(literal).collect();
    (!prefix.is_empty()).then(|| (prefix, parts[first..].to_vec(), tail))
}

/// A key that picks between literals by a condition (`c ? "a" : "b"`,
/// or `"x.#{c ? 'a' : 'b'}"`): the condition, and the arguments with
/// the key each way it can go.
pub(crate) fn split_key(args: &[Expr]) -> Option<(Expr, Vec<Expr>, Vec<Expr>)> {
    let key = args.first()?;
    let with_key = |k: Expr| std::iter::once(k).chain(args[1..].iter().cloned()).collect::<Vec<_>>();
    let (cond, then_key, else_key) = match &*key.node {
        ExprNode::If { cond, then_branch, else_branch } => (cond.clone(), then_branch.clone(), else_branch.clone()),
        ExprNode::StringInterp { parts } => {
            let at = parts.iter().position(|p| match p {
                InterpPart::Expr { expr } => interpolated_text(expr).is_none(),
                InterpPart::Text { .. } => false,
            })?;
            let InterpPart::Expr { expr } = &parts[at] else { unreachable!() };
            let ExprNode::If { cond, then_branch, else_branch } = &*expr.node else { return None };
            let swap = |branch: &Expr| {
                let mut parts = parts.clone();
                parts[at] = InterpPart::Expr { expr: branch.clone() };
                Expr::new(key.span, ExprNode::StringInterp { parts })
            };
            (cond.clone(), swap(then_branch), swap(else_branch))
        }
        _ => return None,
    };
    Some((cond, with_key(then_key), with_key(else_key)))
}

thread_local! {
    static PLANNED: std::cell::RefCell<Option<SharedPlanCache>> =
        const { std::cell::RefCell::new(None) };
}

// Answers shared only within one immutable-catalog phase and its workers.
#[derive(Clone, Default)]
pub(crate) struct SharedPlanCache(std::sync::Arc<std::sync::Mutex<
    crate::det::HashMap<PlanKey, Result<Vec<(Variant, Plan)>, String>>
>>);

pub(crate) fn plan_cache_snapshot() -> Option<SharedPlanCache> {
    PLANNED.with(|c| c.borrow().clone())
}

pub(crate) fn install_plan_cache(cache: Option<SharedPlanCache>) -> impl Drop {
    struct Restore(Option<SharedPlanCache>);
    impl Drop for Restore {
        fn drop(&mut self) { PLANNED.with(|c| *c.borrow_mut() = self.0.take()); }
    }
    Restore(PLANNED.with(|c| c.replace(cache)))
}

/// Planning reads the parsed lookup, the literal locale (if any), the
/// presence/tail of a dynamic key, and view context. Runtime expressions
/// are evaluated by folding, not planning. Spans are not identities:
/// class macros copy them while substituting different option values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PlanKey {
    lookup: Translate,
    locale: Option<String>,
    dynamic_tail: Option<String>,
    in_view: bool,
}

/// Keep plans for one phase over one immutable catalog. Nested analyses
/// start a fresh cache: an outer session may analyze another app/catalog.
pub fn with_plan_cache<T>(f: impl FnOnce() -> T) -> T {
    let _cache = install_plan_cache(Some(Default::default()));
    f()
}

fn plan_key(call: &Call, in_view: bool) -> PlanKey {
    PlanKey {
        lookup: call.lookup.clone(),
        locale: match &call.locale {
            Some(Locale::Literal(locale)) => Some(locale.clone()),
            _ => None,
        },
        dynamic_tail: call.dynamic.as_ref().map(|(_, tail)| tail.clone()),
        in_view,
    }
}

/// `plans` for the call written as `recv.method(args)`, kept for the length of `with_plan_cache`.
fn plans_at(catalog: &Catalog, call: &Call, in_view: bool) -> Result<Vec<(Variant, Plan)>, String> {
    let key = plan_key(call, in_view);
    if let Some(hit) = PLANNED.with(|c| c.borrow().as_ref().and_then(|m| m.0.lock().unwrap().get(&key).cloned())) {
        return hit;
    }
    let out = plans(catalog, call, in_view);
    PLANNED.with(|c| {
        if let Some(m) = c.borrow().as_ref() {
            m.0.lock().unwrap().insert(key, out.clone());
        }
    });
    out
}

/// Whether every key a translate call can look up resolves; `None` when
/// this is not a translate call.
pub(crate) fn resolves(
    catalog: &Catalog,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    site: Site<'_>,
) -> Option<Result<(), String>> {
    resolves_uncached(catalog, recv, method, args, block, site)
}

fn resolves_uncached(
    catalog: &Catalog,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    site: Site<'_>,
) -> Option<Result<(), String>> {
    let parsed = parse(recv, method, args, block, site)?;
    let answer = match (parsed, split_key(args)) {
        (Err(reason), _) if block.is_some() => return Some(Err(reason)),
        (_, Some((_, then_args, else_args))) => [then_args, else_args]
            .iter()
            .try_for_each(|a| resolves(catalog, recv, method, a, block, site).expect("still a translate call")),
        (Ok(call), None) if counted_by_rule(&call, catalog) => Err(COUNT_BY_RULE.to_string()),
        (Ok(call), None) => plans_at(catalog, &call, site.in_view()).map(|_| ()),
        (Err(reason), None) => Err(reason),
    };
    Some(match answer {
        Err(reason) if block.is_none() && runs_at_run_time(&reason, args, catalog) => Ok(()),
        other => other,
    })
}

/// Whether a call refused for `reason` can be answered by
/// `I18n.translate_runtime` instead, which reads the app's whole catalog
/// as the gem does. Not one whose count may meet a locale's plural rule
/// that roundhouse cannot read.
fn runs_at_run_time(reason: &str, args: &[Expr], catalog: &Catalog) -> bool {
    let runtime = reason.starts_with("a key computed at run time")
        || reason == "a scope computed at run time"
        || reason == "a default computed at run time"
        || reason == "options that are not a literal Hash"
        || reason == "a `locale:` read after an option with effects"
        || reason == COUNT_BY_RULE
        || reason.starts_with("missing interpolation argument");
    if !runtime || !matches!(args.len(), 1 | 2) {
        return false;
    }
    if catalog.plurals.values().all(|rule| !rule.is_empty()) {
        return true;
    }
    match args.get(1).map(|o| &*o.node) {
        None => true,
        Some(ExprNode::Hash { entries, .. }) => !entries.iter().any(|(k, _)| {
            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } })
                || matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "count")
        }),
        Some(_) => false,
    }
}

/// `I18n.t("views.feed")` landing on a node of Strings, which I18n
/// answers as a nested Hash of them: its full key, when every locale's
/// lookup lands on such a node. Not from a template's `t`, whose Hash
/// would meet the view's escaping, nor with anything a Hash answer
/// would ignore or need (`count:`, `default:`, interpolation values).
pub(crate) fn tree_answer(catalog: &Catalog, recv: Option<&Expr>, method: &str, args: &[Expr], block: Option<&Expr>) -> Option<String> {
    let on_i18n = matches!(recv.map(|r| &*r.node),
        Some(ExprNode::Const { path }) if path.len() == 1 && path[0].as_str() == "I18n");
    if !on_i18n || block.is_some() || !matches!(method, "t" | "translate") {
        return None;
    }
    let call = parse_args(args, None, false).ok()?;
    if call.dynamic.is_some() || call.lookup.counted || !call.lookup.defaults.is_empty() || !call.lookup.values.is_empty() {
        return None;
    }
    let full = Translate::full_key(&call.lookup, &call.lookup.key).ok()?;
    let lands = |c: &Catalog| c.entries.contains_key(&full) || c.opaque.contains(&full) || c.is_subtree(&full);
    // Most keys are leaves: reject the first variant without building
    // every locale/environment's owned Variant. Repeated locales give
    // the same verdict, so this `all` needs no deduplicated locale list.
    let every = std::iter::once(&catalog.locale)
        .chain(&catalog.available)
        .chain(catalog.others.iter().map(|c| &c.locale))
        .all(|locale| {
            std::iter::once(&catalog.fallbacks.app).chain(catalog.fallbacks.envs.iter().map(|(_, setting)| setting)).all(|fallbacks| {
                let chain = match fallbacks {
                    Some(defaults) => crate::i18n::fallback_chain(locale, defaults),
                    None => vec![locale.clone()],
                };
                chain.iter().filter_map(|l| catalog.catalog_for(l)).find(|c| lands(c)).is_some_and(|c| c.plain_subtree(&full))
            })
        });
    every.then_some(full)
}

/// `t("views.calendar.weekdays")` landing on an Array, a number or a
/// boolean, which I18n answers as it is: its full key and the runtime
/// method answering it, when every locale's lookup lands on a value of
/// one kind (an Array's items Strings in one locale and Symbols in
/// another, as rails-i18n's `date.order` is, are both). Not an `_html`
/// key, whose Array ActionView marks html-safe item by item.
pub(crate) fn value_answer(catalog: &Catalog, recv: Option<&Expr>, method: &str, args: &[Expr], block: Option<&Expr>, site: Site<'_>) -> Option<(String, &'static str, Ty)> {
    use crate::i18n::Opaque;
    let call = parse(recv, method, args, block, site)?.ok()?;
    if call.dynamic.is_some() || call.lookup.counted || !call.lookup.defaults.is_empty() || !call.lookup.values.is_empty() {
        return None;
    }
    let full = Translate::full_key(&call.lookup, &call.lookup.key).ok()?;
    if crate::i18n::is_html_safe_key(&full) {
        return None;
    }
    let mut locales = catalog.locales();
    for other in &catalog.others {
        if !locales.contains(&other.locale) {
            locales.push(other.locale.clone());
        }
    }
    let lands = |c: &Catalog| c.entries.contains_key(&full) || c.opaque.contains(&full) || c.is_subtree(&full);
    let mut found: Vec<&Opaque> = Vec::new();
    for v in catalog.variants(&locales) {
        let chain = match &v.fallbacks {
            Some(defaults) => crate::i18n::fallback_chain(&v.locale, defaults),
            None => vec![v.locale.clone()],
        };
        found.push(chain.iter().filter_map(|l| catalog.catalog_for(l)).find(|c| lands(c))?.values.get(&full)?);
    }
    let first = found.first()?;
    if !found.iter().all(|v| std::mem::discriminant(*v) == std::mem::discriminant(*first)) {
        return None;
    }
    let array = |elem: Ty| Ty::Array { elem: std::sync::Arc::new(elem) };
    Some(match first {
        Opaque::Array(_) => {
            let items = found.iter().flat_map(|v| match v {
                Opaque::Array(items) => items.iter().map(|(sym, _)| *sym).collect(),
                _ => Vec::new(),
            });
            let (strings, symbols) = items.fold((false, false), |(s, y), sym| (s || !sym, y || sym));
            match (strings, symbols) {
                (_, false) => (full, "translate_strings", array(Ty::Str)),
                (false, true) => (full, "translate_symbols", array(Ty::Sym)),
                (true, true) => (full, "translate_items", array(Ty::Union { variants: vec![Ty::Str, Ty::Sym].into() })),
            }
        }
        Opaque::Int(_) => (full, "translate_integer", Ty::Int),
        Opaque::Float(_) => (full, "translate_float", Ty::Float),
        Opaque::Bool(_) => (full, "translate_boolean", Ty::Bool),
    })
}

const COUNT_BY_RULE: &str = "a count whose form a locale's own plural rule picks";

/// A count meets a locale's own rule only when the call runs.
fn counted_by_rule(call: &Call, catalog: &Catalog) -> bool {
    call.lookup.counted && !catalog.plurals.is_empty()
}

fn parse_args(args: &[Expr], view: Option<&str>, template: bool) -> Result<Call, String> {
    let (key, opts) = match args {
        [key] => (key, None),
        [key, opts] => (key, Some(opts)),
        _ => return Err("an argument shape other than (key, **options)".into()),
    };
    let (key, dynamic) = match literal_text(key) {
        Some(key) => (key, None),
        None => {
            let (prefix, rest, tail) = dynamic_key(key).ok_or("a key computed at run time")?;
            (prefix, Some((rest, tail)))
        }
    };
    let mut call = Call {
        lookup: Translate { key, view: view.map(str::to_string), template, ..Default::default() },
        count: None,
        values: Vec::new(),
        order: Vec::new(),
        dynamic,
        locale: None,
    };
    let Some(opts) = opts else { return Ok(call) };
    let ExprNode::Hash { entries, .. } = &*opts.node else {
        return Err("options that are not a literal Hash".into());
    };
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            return Err("an option key that is not a Symbol".into());
        };
        match name.as_str() {
            "scope" => {
                let parts: Option<Vec<String>> = match &*v.node {
                    ExprNode::Array { elements, .. } => elements.iter().map(literal_text).collect(),
                    _ => literal_text(v).map(|s| vec![s]),
                };
                call.lookup.scope = Some(parts.ok_or("a scope computed at run time")?.join("."));
            }
            "default" => {
                let one = |e: &Expr| match &*e.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(Fallback::Key(value.as_str().to_string())),
                    ExprNode::Lit { value: Literal::Str { value } } => Some(Fallback::Text(value.clone())),
                    _ => None,
                };
                let defaults: Option<Vec<Fallback>> = match &*v.node {
                    ExprNode::Array { elements, .. } => elements.iter().map(one).collect(),
                    _ => one(v).map(|d| vec![d]),
                };
                call.lookup.defaults = defaults.ok_or("a default computed at run time")?;
                call.lookup.default_list = matches!(&*v.node, ExprNode::Array { .. });
            }
            "count" => {
                call.lookup.counted = true;
                call.count = Some(v.clone());
                call.order.push("count".into());
            }
            "locale" => {
                call.locale = Some(match literal_text(v) {
                    Some(locale) => Locale::Literal(locale),
                    // Read before the arms that evaluate the other options,
                    // so none before it may have an effect to reorder.
                    None => {
                        let effect = call.order.iter().any(|n| {
                            let value = if n == "count" { call.count.as_ref() } else { call.values.iter().find(|(m, _)| m == n).map(|(_, v)| v) };
                            value.is_some_and(|v| !is_plain(v))
                        });
                        if effect {
                            return Err("a `locale:` read after an option with effects".into());
                        }
                        Locale::Dynamic(v.clone())
                    }
                });
            }
            "raise" | "throw" | "separator" | "exception_handler" | "fallback" => {
                return Err(format!("the `{name}:` option"));
            }
            _ => {
                call.lookup.values.push(name.as_str().to_string());
                call.values.push((name.as_str().to_string(), v.clone()));
                call.order.push(name.as_str().to_string());
            }
        }
    }
    Ok(call)
}

thread_local! {
    /// The sites this lowering wrote to the table, by their records, and the records in order.
    static TABLE: std::cell::RefCell<(crate::det::HashMap<u64, Vec<usize>>, Vec<TableSite>)> =
        std::cell::RefCell::new(Default::default());
}

thread_local! {
    /// The calls this lowering left to `I18n.translate_runtime`, and every call that reads the translation table.
    static RUNTIME_SITES: std::cell::RefCell<(Vec<Span>, Vec<Span>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

/// Fold every resolvable translate call in hook bodies, test bodies and
/// templates. A controller's instance methods (its actions and the
/// private methods they call) take the controller's `t`.
pub fn apply_i18n_translate_lowering(app: &mut App) {
    // This catalog is a new phase, independent of analysis and outer sessions.
    let _cache = install_plan_cache(Some(Default::default()));
    TABLE.with(|t| *t.borrow_mut() = Default::default());
    RUNTIME_SITES.with(|r| *r.borrow_mut() = Default::default());
    let catalog = app.i18n.or_rails_default().clone();
    let controllers: crate::det::HashMap<crate::ident::ClassId, String> = app
        .controllers
        .iter()
        .filter(|c| translates(app, c))
        .map(|c| (c.name.clone(), controller_scope(c.name.0.as_str())))
        .collect();
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        if owner.is_some_and(|o| controllers.contains_key(o)) {
            return;
        }
        rewrite(body, &catalog, Site::Other)
    });
    // An `app/helpers` module's instance methods run in the view: their bare `t` is ActionView's, with no template for a lazy key.
    let helpers: crate::det::HashSet<crate::ident::ClassId> = app.helper_method_index.values().cloned().collect();
    for lc in app.library_classes.iter_mut().filter(|lc| helpers.contains(&lc.name)) {
        for m in lc.methods.iter_mut().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
            for default in m.params.iter_mut().filter_map(|p| p.default.as_mut()) {
                rewrite_default(default, &catalog, Site::View(None));
            }
            rewrite(&mut m.body, &catalog, Site::View(None));
        }
    }
    for controller in &mut app.controllers {
        let Some(scope) = controllers.get(&controller.name) else { continue };
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        rewrite_default(default, &catalog, Site::Controller(scope));
                    }
                    for default in action.kw_params.iter_mut().filter_map(|(_, d)| d.as_mut()) {
                        rewrite_default(default, &catalog, Site::Controller(scope));
                    }
                    rewrite(&mut action.body, &catalog, Site::Controller(scope));
                }
                crate::dialect::ControllerBodyItem::ClassMethod { method, .. } => {
                    for p in &mut method.params {
                        if let Some(default) = &mut p.default {
                            rewrite_default(default, &catalog, Site::Other);
                        }
                    }
                    rewrite(&mut method.body, &catalog, Site::Other);
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => rewrite(expr, &catalog, Site::Other),
                crate::dialect::ControllerBodyItem::Filter { filter, .. } => {
                    for c in [&mut filter.if_cond_expr, &mut filter.unless_cond_expr].into_iter().flatten() {
                        rewrite(c, &catalog, Site::Other);
                    }
                }
                _ => {}
            }
        }
    }
    super::for_each_test_body(app, &mut |body| rewrite(body, &catalog, Site::Other));
    for view in &mut app.views {
        let name = view.name.as_str().to_string();
        rewrite(&mut view.body, &catalog, Site::View(Some(&name)));
    }
    app.i18n_table = TABLE.with(|t| std::mem::take(&mut t.borrow_mut().1));
    (app.i18n_runtime_sites, app.i18n_table_sites) = RUNTIME_SITES.with(|r| std::mem::take(&mut *r.borrow_mut()));
}

/// Whether `controller` has `AbstractController::Translation`'s `t`:
/// it descends from `ActionController::Base` (an `ActionController::API`
/// controller has no `t`) and neither it nor an app ancestor defines its
/// own `t` / `translate`.
fn translates(app: &App, controller: &crate::dialect::Controller) -> bool {
    let by_name: crate::det::HashMap<&str, &crate::dialect::Controller> =
        app.controllers.iter().map(|c| (c.name.0.as_str(), c)).collect();
    let mut current = Some(controller);
    for _ in 0..32 {
        let Some(c) = current else { return false };
        if c.actions().any(|a| matches!(a.name.as_str(), "t" | "translate")) {
            return false;
        }
        let Some(parent) = c.parent.as_ref().map(|p| p.0.as_str()) else { return false };
        if parent == "ActionController::Base" {
            return true;
        }
        // A parent named as written may be relative to the child's namespace.
        current = by_name.get(parent).copied().or_else(|| {
            let ns = c.name.0.as_str().rsplit_once("::").map(|(ns, _)| ns)?;
            by_name.get(format!("{ns}::{parent}").as_str()).copied()
        });
    }
    false
}

// A parameter default has no statement ahead of it to hoist a `Seq` into, so a dynamic key's `__k = …; case __k …` keeps its own `begin`.
fn rewrite_default(expr: &mut Expr, catalog: &Catalog, site: Site<'_>) {
    rewrite(expr, catalog, site);
    if matches!(&*expr.node, ExprNode::Seq { exprs } if exprs.len() > 1) {
        let body = expr.clone();
        *expr = Expr { ty: body.ty.clone(), ..Expr::new(body.span, ExprNode::BeginRescue { body, rescues: Vec::new(), else_branch: None, ensure: None, implicit: false }) };
    }
}

fn rewrite(expr: &mut Expr, catalog: &Catalog, site: Site<'_>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, catalog, site));
    // Folded as the view's `t` it is, without the `raw` a controller body has no helper for: nothing renders this copy.
    if expr.decisions & crate::expr::HELPER_COPY_TRANSLATE != 0 {
        expr.decisions &= !crate::expr::HELPER_COPY_TRANSLATE;
        rewrite_one(expr, catalog, Site::View(None));
        if let ExprNode::Send { recv: None, method, args, .. } = &*expr.node
            && method.as_str() == "raw"
            && args.len() == 1
        {
            *expr = args[0].clone();
        }
        return;
    }
    rewrite_one(expr, catalog, site);
}

fn rewrite_one(expr: &mut Expr, catalog: &Catalog, site: Site<'_>) {
    if let Some(spelled) = with_locale(expr) {
        *expr = spelled;
        return;
    }
    let in_view = site.in_view();
    let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { return };
    let Some(parsed) = parse(recv.as_ref(), method.as_str(), args, block.as_ref(), site) else { return };
    if let Some((cond, then_args, else_args)) = split_key(args) {
        let resolved = resolves(catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), site);
        if !matches!(resolved, Some(Ok(()))) {
            return;
        }
        let branch = |args: Vec<Expr>| {
            let mut e = expr.clone();
            if let ExprNode::Send { args: a, .. } = &mut *e.node {
                *a = args;
            }
            rewrite(&mut e, catalog, site);
            e
        };
        let (then_branch, else_branch) = (branch(then_args), branch(else_args));
        *expr = typed(Expr::new(expr.span, ExprNode::If { cond, then_branch, else_branch }));
        return;
    }
    let runtime = |reason: &str, expr: &mut Expr| {
        let ExprNode::Send { args, block: None, .. } = &*expr.node else { return };
        if runs_at_run_time(reason, args, catalog) {
            *expr = runtime_call(expr.span, args, catalog, site);
        }
    };
    if let Some(full) = tree_answer(catalog, recv.as_ref(), method.as_str(), args, block.as_ref()) {
        let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
        *expr = tree_call(expr.span, "translate_tree", hash_ty, &full, args.get(1));
        return;
    }
    if let Some((full, method, ty)) = value_answer(catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), site) {
        *expr = tree_call(expr.span, method, ty, &full, args.get(1));
        return;
    }
    let call = match parsed {
        Ok(call) => call,
        Err(reason) => return runtime(&reason, expr),
    };
    if counted_by_rule(&call, catalog) {
        return runtime(COUNT_BY_RULE, expr);
    }
    let plans = match plans_at(catalog, &call, in_view) {
        Ok(plans) => plans,
        Err(reason) => return runtime(&reason, expr),
    };
    let Some(folded) = fold_variants(&call, plans, expr.span, in_view, catalog) else { return };
    *expr = folded;
}

/// The call folded once per way it can run, chosen between when it runs:
/// by the locale (`I18n.locale`, or the `locale:` the call reads), then
/// by the environment where `config.i18n.fallbacks` differs between
/// them. Ways that fold the same share one branch, so an app with one
/// locale and one setting folds exactly as before. In a template a
/// branch that is not html-safe is escaped when another one is, and the
/// whole is marked raw.
fn fold_variants(call: &Call, plans: Vec<(Variant, Plan)>, span: Span, in_view: bool, catalog: &Catalog) -> Option<Expr> {
    // Per locale, in order: its plans, the variants that share each.
    let mut locales: Vec<(String, Vec<(Plan, Vec<Variant>)>)> = Vec::new();
    for (variant, plan) in plans {
        let at = match locales.iter().position(|(l, _)| *l == variant.locale) {
            Some(at) => at,
            None => {
                locales.push((variant.locale.clone(), Vec::new()));
                locales.len() - 1
            }
        };
        let groups = &mut locales[at].1;
        match groups.iter_mut().find(|(p, _)| *p == plan) {
            Some((_, variants)) => variants.push(variant),
            None => groups.push((plan, vec![variant])),
        }
    }
    let shared = locales.windows(2).all(|w| w[0].1 == w[1].1);
    if !shared && locales.len() > 1 {
        return table_site(call, &locales, span, in_view, catalog);
    }
    let mut arms: Vec<(String, (Expr, bool))> = Vec::new();
    for (locale, groups) in &locales {
        let mut branches: Vec<(Option<Vec<String>>, (Expr, bool))> = Vec::new();
        for (plan, variants) in groups {
            // The variant that covers every environment not named is the else branch.
            let envs = match variants.iter().any(|v| v.envs.is_none()) {
                true => None,
                false => Some(variants.iter().flat_map(|v| v.envs.clone().unwrap_or_default()).collect()),
            };
            branches.push((envs, fold_plan(call, plan.clone(), locale, span, in_view, catalog)?));
        }
        branches.sort_by_key(|(envs, _)| envs.is_none());
        arms.push((locale.clone(), choose(span, in_view, branches, |envs| {
            let mut test = const_send(span, "I18n", "running_in?", vec![lit(span, envs.join(","))]);
            test.ty = Some(Ty::Bool);
            test
        })));
        if shared {
            break;
        }
    }
    let (whole, html) = if arms.len() == 1 {
        arms.pop()?.1
    } else {
        let scrutinee = match &call.locale {
            Some(Locale::Dynamic(value)) => const_send(span, "I18n", "locale_name", vec![value.clone()]),
            _ => Expr::new(span, ExprNode::Send {
                recv: Some(Expr { ty: Some(Ty::Sym), ..const_send(span, "I18n", "locale", vec![]) }),
                method: Symbol::from("to_s"),
                args: vec![],
                block: None,
                parenthesized: false,
            }),
        };
        // The default locale, first, answers any other.
        let default = arms.remove(0);
        let mut branches: Vec<(Option<String>, (Expr, bool))> = arms.into_iter().map(|(l, e)| (Some(l), e)).collect();
        branches.push((None, default.1));
        let any_html = in_view && branches.iter().any(|(_, (_, h))| *h);
        let mut case_arms = Vec::new();
        for (locale, shown) in branches {
            let body = show(span, shown, any_html);
            case_arms.push(Arm {
                pattern: match locale {
                    Some(l) => Pattern::Lit { value: Literal::Str { value: l } },
                    None => Pattern::Wildcard,
                },
                guard: None,
                body,
            });
        }
        (typed(Expr::new(span, ExprNode::Case { scrutinee: typed(scrutinee), arms: case_arms })), any_html)
    };
    Some(if html { raw(span, whole) } else { whole })
}

/// `I18n.translate_runtime(key, options, lazy_scope, in_view, raises)`
/// for a call written `t(key, options)`: the options as the call passes
/// them, and in a template the answer marked raw, the runtime having
/// escaped what is not html-safe.
/// `I18n.translate_tree(full_key, options)`: the node's Strings as a
/// nested Hash, built from the app's catalog when the call runs; the
/// other `translate_*` answer an opaque value the same way.
fn tree_call(span: Span, method: &str, ty: Ty, full: &str, options: Option<&Expr>) -> Expr {
    RUNTIME_SITES.with(|r| {
        let mut r = r.borrow_mut();
        r.0.push(span);
        r.1.push(span);
    });
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
    let options = match options {
        Some(opts) => {
            let mut opts = opts.clone();
            if let ExprNode::Hash { kwargs, .. } = &mut *opts.node {
                *kwargs = false;
            }
            opts
        }
        None => Expr { ty: Some(hash_ty.clone()), ..Expr::new(span, ExprNode::Hash { entries: Vec::new(), kwargs: false }) },
    };
    let mut call = const_send(span, "I18n", method, vec![lit(span, full.to_string()), options]);
    call.ty = Some(ty);
    call
}

fn runtime_call(span: Span, args: &[Expr], catalog: &Catalog, site: Site<'_>) -> Expr {
    RUNTIME_SITES.with(|r| {
        let mut r = r.borrow_mut();
        r.0.push(span);
        r.1.push(span);
    });
    let options = match args.get(1) {
        Some(opts) => {
            let mut opts = opts.clone();
            // Left as keywords the hash would swallow the arguments that follow it.
            if let ExprNode::Hash { kwargs, .. } = &mut *opts.node {
                *kwargs = false;
            }
            opts
        }
        None => Expr {
            ty: Some(Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) }),
            ..Expr::new(span, ExprNode::Hash { entries: Vec::new(), kwargs: false })
        },
    };
    let lazy = match site {
        Site::View(Some(view)) => lit(span, view.replace("/_", ".").replace('/', ".")),
        Site::Controller(scope) => typed(Expr::new(span, ExprNode::StringInterp {
            parts: vec![
                InterpPart::Text { value: format!("{scope}.") },
                InterpPart::Expr { expr: typed(Expr::new(span, ExprNode::Send {
                    recv: None,
                    method: Symbol::from("action_name"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                })) },
            ],
        })),
        _ => lit(span, String::new()),
    };
    let in_view = site.in_view();
    let mut raises = const_send(span, "I18n", "missing_raises", vec![
        Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: catalog.missing.raises } }),
        lit(span, catalog.missing.flipped.join(",")),
    ]);
    raises.ty = Some(Ty::Bool);
    let flag = Expr { ty: Some(Ty::Bool), ..Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: in_view } }) };
    let call = typed(const_send(span, "I18n", "translate_runtime", vec![args[0].clone(), options, lazy, flag, raises]));
    if in_view { raw(span, call) } else { call }
}

/// A call whose answer differs by locale as `I18n.translate_site(…)`: its
/// entry per locale (and environment, and run-time key) goes to the app's
/// table, which the runtime reads when the call runs.
fn table_site(call: &Call, locales: &[(String, Vec<(Plan, Vec<Variant>)>)], span: Span, in_view: bool, catalog: &Catalog) -> Option<Expr> {
    let static_html = in_view && crate::i18n::is_html_safe_key(&call.lookup.key);
    let plans = || locales.iter().flat_map(|(_, groups)| groups.iter().map(|(plan, _)| plan));
    let translations = |plan: &Plan| -> Vec<Translation> {
        match plan {
            Plan::Static(t) => vec![t.clone()],
            Plan::Dynamic { arms, otherwise, .. } => arms.iter().map(|(_, t)| t.clone()).chain([otherwise.clone()]).collect(),
        }
    };
    if call.count.is_none() && plans().any(|p| translations(p).iter().any(|t| matches!(t, Translation::Plural { .. }))) {
        return None;
    }
    let any_missing = plans().any(|p| matches!(p, Plan::Static(Translation::Missing { .. })));
    // Not written with a separator of the table's in it: the runtime splits its lines on U+001C to U+001F, and the call stays the error it was.
    let separator = |t: &str| t.chars().any(|c| ('\u{1c}'..='\u{1f}').contains(&c));
    let texts = |t: &Translation| -> Vec<String> {
        match t {
            Translation::One(x) => vec![x.clone()],
            Translation::Plural { zero, one, other } => zero.iter().cloned().chain([one.clone(), other.clone()]).collect(),
            Translation::Missing { key, considered } => std::iter::once(key.clone()).chain(considered.iter().cloned()).collect(),
        }
    };
    let keys = |p: &Plan| -> Vec<String> {
        match p {
            Plan::Static(_) => Vec::new(),
            Plan::Dynamic { arms, subtrees, .. } => arms.iter().map(|(k, _)| k.clone()).chain(subtrees.iter().cloned()).collect(),
        }
    };
    if plans().any(|p| translations(p).iter().flat_map(texts).chain(keys(p)).any(|t| separator(&t))) {
        return None;
    }
    // Not the entries spelled out here: only a ruby-family tree writes the table, and a browser transpile to another target would build half a million strings for nothing.
    let full_key = Translate::full_key(&call.lookup, &call.lookup.key).unwrap_or_default();
    let interpolates = call.lookup.interpolates();
    let shape = (locales, full_key.as_str(), static_html, in_view, interpolates);
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        shape.hash(&mut h);
        h.finish()
    };
    RUNTIME_SITES.with(|r| r.borrow_mut().1.push(span));
    let id = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        let (index, sites) = &mut *t;
        let same = |site: &TableSite| (site.locales.as_slice(), site.full_key.as_str(), site.static_html, site.in_view, site.interpolates) == shape;
        if let Some(&at) = index.get(&hash).and_then(|c| c.iter().find(|&&at| same(&sites[at]))) {
            return sites[at].id.clone();
        }
        let id = format!("t{}", sites.len());
        index.entry(hash).or_default().push(sites.len());
        sites.push(TableSite { id: id.clone(), locales: locales.to_vec(), full_key: full_key.clone(), static_html, in_view, interpolates });
        id
    });
    let dynamic = call.dynamic.is_some();
    let mut call = call.clone();
    let bound = bind_all_options(&mut call, span);
    let loc = match &call.locale {
        Some(Locale::Dynamic(value)) => typed(const_send(span, "I18n", "locale_name", vec![value.clone()])),
        _ => typed(Expr::new(span, ExprNode::Send {
            recv: Some(Expr { ty: Some(Ty::Sym), ..const_send(span, "I18n", "locale", vec![]) }),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        })),
    };
    let key = match &call.dynamic {
        Some((parts, _)) => {
            let base = Translate::full_key(&call.lookup, &call.lookup.key).ok()?;
            let text = typed(Expr::new(span, ExprNode::StringInterp {
                parts: std::iter::once(InterpPart::Text { value: base }).chain(parts.iter().cloned()).collect(),
            }));
            typed(const_send(span, "I18n", "normalize_key", vec![text]))
        }
        None => lit(span, String::new()),
    };
    let count = match &call.count {
        Some(c) => c.clone(),
        None => Expr { ty: Some(Ty::Nil), ..Expr::new(span, ExprNode::Lit { value: Literal::Nil }) },
    };
    let names = call.values.iter().map(|(n, _)| lit(span, n.clone())).collect();
    let values = call.values.iter().map(|(_, v)| typed(Expr::new(span, ExprNode::Send {
        recv: Some(v.clone()),
        method: Symbol::from("to_s"),
        args: vec![],
        block: None,
        parenthesized: false,
    }))).collect();
    let str_array = |elements: Vec<Expr>| Expr {
        ty: Some(Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
        ..Expr::new(span, ExprNode::Array { elements, style: Default::default() })
    };
    let details = match in_view && (any_missing || dynamic) {
        true => missing_details(span, &call),
        false => lit(span, String::new()),
    };
    let mut raises = const_send(span, "I18n", "missing_raises", vec![
        Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: catalog.missing.raises } }),
        lit(span, catalog.missing.flipped.join(",")),
    ]);
    raises.ty = Some(Ty::Bool);
    let bool_lit = |value: bool| Expr { ty: Some(Ty::Bool), ..Expr::new(span, ExprNode::Lit { value: Literal::Bool { value } }) };
    let escape_plain = in_view && (dynamic || static_html || any_missing);
    let mut site = typed(const_send(span, "I18n", "translate_site", vec![
        lit(span, id), loc, key, count, str_array(names), str_array(values), details, raises, bool_lit(in_view), bool_lit(escape_plain),
    ]));
    if !bound.is_empty() {
        site = typed(Expr::new(span, ExprNode::Seq { exprs: bound.into_iter().chain([site]).collect() }));
    }
    Some(if escape_plain { raw(span, site) } else { site })
}

/// `translation` as a table entry: its kind, whether it is html-safe, and its fields.
fn table_entry(interpolates: bool, translation: &Translation, key: &str, html: bool) -> Vec<String> {
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();
    let template = |t: &str| -> String {
        let pieces = match interpolates {
            true => crate::i18n::pieces(t).unwrap_or_else(|_| vec![Piece::Text(t.to_string())]),
            false => vec![Piece::Text(t.to_string())],
        };
        pieces
            .into_iter()
            .map(|p| match p {
                Piece::Text(text) => format!("t{text}"),
                Piece::Name(name) => format!("n{name}"),
            })
            .collect::<Vec<_>>()
            .join("\u{1d}")
    };
    match translation {
        Translation::One(t) => vec!["o".into(), flag(html), template(t)],
        Translation::Plural { zero, one, other } => {
            vec!["p".into(), flag(html), flag(zero.is_some()), template(zero.as_deref().unwrap_or("")), template(one), template(other)]
        }
        Translation::Missing { considered, .. } => {
            let rest: String = considered.iter().map(|k| format!("\n- {k}")).collect();
            vec!["m".into(), "1".into(), key.to_string(), rest]
        }
    }
}

/// A `t` whose answer differs by locale, as the translation table keeps it:
/// its plans per locale and what decides each entry's text and escaping.
#[derive(Clone, Debug, PartialEq)]
pub struct TableSite {
    pub(crate) id: String,
    pub(crate) locales: Vec<(String, Vec<(Plan, Vec<Variant>)>)>,
    pub(crate) full_key: String,
    pub(crate) static_html: bool,
    pub(crate) in_view: bool,
    pub(crate) interpolates: bool,
}

impl TableSite {
    /// Each record's key (the site, locale, environment and run-time key) and its entry.
    pub fn records(&self) -> Vec<(String, Vec<String>)> {
        let mut records: Vec<(String, Vec<String>)> = Vec::new();
        for (locale, groups) in &self.locales {
            for (plan, variants) in groups {
                let envs: Vec<String> = match variants.iter().any(|v| v.envs.is_none()) {
                    true => vec![String::new()],
                    false => variants.iter().flat_map(|v| v.envs.clone().unwrap_or_default()).collect(),
                };
                let mut entries: Vec<(String, Vec<String>)> = Vec::new();
                match plan {
                    Plan::Static(translation) => {
                        let key = format!("{locale}.{}", self.full_key);
                        entries.push((String::new(), table_entry(self.interpolates, translation, &key, self.static_html)));
                    }
                    Plan::Dynamic { arms, subtrees, otherwise, otherwise_html } => {
                        for (key, translation) in arms {
                            let html = self.in_view && crate::i18n::is_html_safe_key(key);
                            entries.push((key.clone(), table_entry(self.interpolates, translation, &format!("{locale}.{key}"), html)));
                        }
                        for key in subtrees {
                            entries.push((key.clone(), vec!["s".into(), "0".into()]));
                        }
                        entries.push(("*".into(), table_entry(self.interpolates, otherwise, "", self.in_view && *otherwise_html)));
                    }
                }
                for env in &envs {
                    for (key, entry) in &entries {
                        records.push((format!("{}\u{1c}{locale}\u{1c}{env}\u{1c}{key}", self.id), entry.clone()));
                    }
                }
            }
        }
        // Not a block per locale: one that answers as the default locale's does is left to the runtime's fallback to it.
        let Some(default) = self.locales.first().map(|(l, _)| l.as_str()) else { return records };
        let locale_of = |key: &str| key.split('\u{1c}').nth(1).unwrap_or("").to_string();
        let mut blocks: crate::det::HashMap<String, Vec<(String, &Vec<String>)>> = crate::det::HashMap::default();
        for (key, entry) in &records {
            let rest = key.splitn(3, '\u{1c}').nth(2).unwrap_or("").to_string();
            blocks.entry(locale_of(key)).or_default().push((rest, entry));
        }
        let same: crate::det::HashSet<String> = blocks
            .iter()
            .filter(|(l, b)| l.as_str() != default && blocks.get(default) == Some(*b))
            .map(|(l, _)| l.clone())
            .collect();
        records.retain(|(key, _)| !same.contains(&locale_of(key)));
        records
    }
}

/// The call's options in a template's missing-translation title, as `missing` writes them.
fn missing_details(span: Span, call: &Call) -> Expr {
    let mut parts = Vec::new();
    for name in &call.order {
        let value = match name.as_str() {
            "count" => call.count.clone(),
            _ => call.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
        };
        if let Some(value) = value {
            parts.push(InterpPart::Text { value: format!(", {name}: ") });
            parts.push(InterpPart::Expr { expr: escaped(span, value) });
        }
    }
    if parts.is_empty() { lit(span, String::new()) } else { typed(Expr::new(span, ExprNode::StringInterp { parts })) }
}

/// Every option that is not a literal bound to a local, in source order, once any is not a plain read: the table call reads `count:` apart from the values, and the details again.
fn bind_all_options(call: &mut Call, span: Span) -> Vec<Expr> {
    let missing = Translation::Missing { key: String::new(), considered: Vec::new() };
    bind_options(call, &missing, span)
}

/// `I18n.with_locale(l) { body }`, or `I18n.with_locale(l, &action)` as
/// an `around_action` writes it, spelled out as the gem runs it:
///
/// ```ruby
/// begin
///   i18n_locale_prev = I18n.enter_locale(l)
///   body              # or action.call
/// ensure
///   I18n.leave_locale(i18n_locale_prev)
/// end
/// ```
///
/// The gem returns the block's value, which no runtime signature can
/// type without generics; spelled out, the value is the body's own.
/// `enter_locale(nil)` changes nothing and answers nil, so nothing is
/// restored, as in the gem. A block with parameters, or none, is left
/// as written.
fn with_locale(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block: Some(block), .. } = &*e.node else { return None };
    let on_i18n = matches!(&*r.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "I18n");
    if !on_i18n || method.as_str() != "with_locale" {
        return None;
    }
    let [locale] = args.as_slice() else { return None };
    let span = e.span;
    let body = match &*block.node {
        ExprNode::Lambda { params, rest_param: None, extra_params, block_param: None, body, .. }
            if params.is_empty() && extra_params.is_empty() => body.clone(),
        ExprNode::Var { .. } => {
            let mut call = Expr::new(span, ExprNode::Send {
                recv: Some(block.clone()),
                method: Symbol::from("call"),
                args: vec![],
                block: None,
                parenthesized: false,
            });
            call.ty = Some(Ty::Untyped);
            call
        }
        _ => return None,
    };
    // Every node is stamped: the lowered IR is diagnosed again.
    let prev_ty = Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() };
    // Not underscored: Elixir warns on reading a local whose name starts with `_`
    let prev = Symbol::from(format!("i18n_locale_prev_{}", span.start));
    let read_prev = Expr { ty: Some(prev_ty.clone()), ..Expr::new(span, ExprNode::Var { id: VarId(0), name: prev.clone() }) };
    let enter = Expr { ty: Some(prev_ty.clone()), ..const_send(span, "I18n", "enter_locale", vec![locale.clone()]) };
    let leave = Expr { ty: Some(Ty::Nil), ..const_send(span, "I18n", "leave_locale", vec![read_prev]) };
    let save = Expr { ty: Some(prev_ty), ..Expr::new(span, ExprNode::Assign { target: LValue::Var { id: VarId(0), name: prev }, value: enter }) };
    let body_ty = body.ty.clone();
    // The save inside the `begin`, not a statement before it: a value-position Seq is hoisted ahead of the whole statement (`[t(…), with_locale(…) { … }]`), which ran its siblings in the new locale.
    let body = Expr { ty: body_ty.clone(), ..Expr::new(span, ExprNode::Seq { exprs: vec![save, body] }) };
    Some(Expr {
        ty: body_ty,
        ..Expr::new(span, ExprNode::BeginRescue { body, rescues: Vec::new(), else_branch: None, ensure: Some(leave), implicit: false })
    })
}

/// `branches` (each with the environments it runs in, `None` for every
/// other one, last) as one value: `if <test(envs)> … else … end`.
fn choose(span: Span, in_view: bool, mut branches: Vec<(Option<Vec<String>>, (Expr, bool))>, test: impl Fn(&[String]) -> Expr) -> (Expr, bool) {
    if branches.len() == 1 {
        return branches.pop().expect("one branch").1;
    }
    let any_html = in_view && branches.iter().any(|(_, (_, h))| *h);
    let (_, last) = branches.pop().expect("an else branch");
    let mut out = show(span, last, any_html);
    while let Some((envs, shown)) = branches.pop() {
        let then_branch = show(span, shown, any_html);
        out = typed(Expr::new(span, ExprNode::If { cond: test(&envs.unwrap_or_default()), then_branch, else_branch: out }));
    }
    (out, any_html)
}

/// A branch's value where its siblings may be html-safe: escaped unless
/// it is html-safe itself, when any sibling is.
fn show(span: Span, (folded, html): (Expr, bool), any_html: bool) -> Expr {
    match (any_html, html) {
        (true, false) => typed(Expr::new(span, ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] })),
            method: Symbol::from("html_escape"),
            args: vec![folded],
            block: None,
            parenthesized: true,
        })),
        _ => folded,
    }
}

/// One plan folded for `locale`, with whether the value is html-safe.
fn fold_plan(call: &Call, plan: Plan, locale: &str, span: Span, in_view: bool, catalog: &Catalog) -> Option<(Expr, bool)> {
    match plan {
        Plan::Static(translation) => {
            let key = lit(span, format!("{locale}.{}", Translate::full_key(&call.lookup, &call.lookup.key).unwrap_or_default()));
            fold(call.clone(), translation, span, in_view, catalog, key)
        }
        Plan::Dynamic { arms, subtrees, otherwise, otherwise_html } => {
            dynamic_fold(call, arms, subtrees, otherwise, otherwise_html, span, in_view, catalog, locale).map(|e| (e, in_view))
        }
    }
}

/// `__k = I18n.normalize_key("<scoped prefix>#{…}")`, then a `case` over
/// every key it can reach, each arm folded as that literal key. In a
/// template every arm is escaped or html-safe already, and the caller
/// marks the whole raw, since arms differ in which they are.
#[allow(clippy::too_many_arguments)]
fn dynamic_fold(
    call: &Call,
    arms: Vec<(String, Translation)>,
    subtrees: Vec<String>,
    otherwise: Translation,
    otherwise_html: bool,
    span: Span,
    in_view: bool,
    catalog: &Catalog,
    locale: &str,
) -> Option<Expr> {
    let (parts, _) = call.dynamic.as_ref()?;
    let base = Translate::full_key(&call.lookup, &call.lookup.key).ok()?;
    let local = Symbol::from(format!("__i18n_key_{}", span.start));
    let read = || typed(Expr::new(span, ExprNode::Var { id: VarId(0), name: local.clone() }));
    let key_text = typed(Expr::new(span, ExprNode::StringInterp {
        parts: std::iter::once(InterpPart::Text { value: base }).chain(parts.iter().cloned()).collect(),
    }));
    let assign = Expr::new(span, ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: local.clone() },
        value: typed(const_send(span, "I18n", "normalize_key", vec![key_text])),
    });
    let shown = |(folded, html): (Expr, bool)| match (in_view, html) {
        (true, false) => typed(Expr::new(span, ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] })),
            method: Symbol::from("html_escape"),
            args: vec![folded],
            block: None,
            parenthesized: true,
        })),
        _ => folded,
    };
    let mut case_arms = Vec::new();
    for (key, translation) in arms {
        let mut each = call.clone_for(&key);
        each.dynamic = None;
        let full = lit(span, format!("{locale}.{key}"));
        let body = shown(fold(each, translation, span, in_view, catalog, full)?);
        case_arms.push(Arm { pattern: Pattern::Lit { value: Literal::Str { value: key } }, guard: None, body });
    }
    for key in subtrees {
        let body = typed(const_send(span, "I18n", "subtree_translation", vec![read()]));
        case_arms.push(Arm { pattern: Pattern::Lit { value: Literal::Str { value: key } }, guard: None, body });
    }
    let missing_key = typed(Expr::new(span, ExprNode::StringInterp {
        parts: vec![InterpPart::Text { value: format!("{locale}.") }, InterpPart::Expr { expr: read() }],
    }));
    let mut rest = call.clone_for(&call.lookup.key);
    rest.dynamic = None;
    let (folded, html) = fold(rest, otherwise, span, in_view, catalog, missing_key)?;
    let html = html || otherwise_html;
    let otherwise = shown((folded, html));
    // A key that reaches nothing in this locale is the else alone: Ruby has no `case` without a `when`.
    let body = if case_arms.is_empty() {
        otherwise
    } else {
        case_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: otherwise });
        typed(Expr::new(span, ExprNode::Case { scrutinee: read(), arms: case_arms }))
    };
    let whole = typed(Expr::new(span, ExprNode::Seq { exprs: vec![assign, body] }));
    Some(whole)
}

impl Call {
    fn clone_for(&self, key: &str) -> Call {
        Call {
            lookup: Translate { key: key.to_string(), ..self.lookup.clone() },
            count: self.count.clone(),
            values: self.values.clone(),
            order: self.order.clone(),
            dynamic: self.dynamic.clone(),
            locale: self.locale.clone(),
        }
    }
}

/// `translation` as the call's value, with whether it is html-safe (an
/// `_html` key's, or a template's missing-translation span). `key` is
/// the missing key's text, read only for `Translation::Missing`.
fn fold(mut call: Call, translation: Translation, span: Span, in_view: bool, catalog: &Catalog, key: Expr) -> Option<(Expr, bool)> {
    let html = in_view && (crate::i18n::is_html_safe_key(&call.lookup.key) || matches!(translation, Translation::Missing { .. }));
    let bound = bind_options(&mut call, &translation, span);
    let render = |template: &str| interpolate(span, template, &call, html);
    let mut folded = match (translation, &call.count) {
        (Translation::One(t), _) => render(&t),
        (Translation::Plural { zero, one, other }, Some(count)) => {
            let is = |n: i64, then: Expr, els: Expr| {
                let mut cond = Expr::new(span, ExprNode::Send {
                    recv: Some(count.clone()),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: n } })],
                    block: None,
                    parenthesized: false,
                });
                cond.ty = Some(Ty::Bool);
                let mut e = Expr::new(span, ExprNode::If { cond, then_branch: then, else_branch: els });
                e.ty = Some(Ty::Str);
                e
            };
            let plural = is(1, render(&one), render(&other));
            match zero {
                Some(z) => is(0, render(&z), plural),
                None => plural,
            }
        }
        (Translation::Plural { .. }, None) => return None,
        (Translation::Missing { considered, .. }, _) => missing(span, key, &considered, &call, in_view, &catalog.missing),
    };
    folded.ty = Some(Ty::Str);
    if !bound.is_empty() {
        folded = typed(Expr::new(span, ExprNode::Seq { exprs: bound.into_iter().chain([folded]).collect() }));
    }
    Some((folded, html))
}

fn lit(span: Span, value: String) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Str { value } }))
}

fn const_send(span: Span, recv: &str, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from(recv)] })),
        method: Symbol::from(method),
        args,
        block: None,
        parenthesized: true,
    })
}

fn raw(span: Span, folded: Expr) -> Expr {
    typed(Expr::new(span, ExprNode::Send {
        recv: None,
        method: Symbol::from("raw"),
        args: vec![folded],
        block: None,
        parenthesized: true,
    }))
}

/// `I18n.missing_translation(key, raises)` (`missing_translation_among`
/// when I18n lists the `default:` keys it also considered), or in a
/// template the debug span Action View renders, titled with the call's
/// options in order: ActionView tries each default key itself and names
/// only the first key in the span.
fn missing(span: Span, key: Expr, considered: &[String], call: &Call, in_view: bool, policy: &crate::i18n::MissingPolicy) -> Expr {
    let mut raises = const_send(span, "I18n", "missing_raises", vec![
        Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: policy.raises } }),
        lit(span, policy.flipped.join(",")),
    ]);
    raises.ty = Some(Ty::Bool);
    if !in_view && !considered.is_empty() {
        let rest: String = considered.iter().map(|k| format!("\n- {k}")).collect();
        return typed(const_send(span, "I18n", "missing_translation_among", vec![key, lit(span, rest), raises]));
    }
    if !in_view {
        return typed(const_send(span, "I18n", "missing_translation", vec![key, raises]));
    }
    let mut parts = Vec::new();
    for name in &call.order {
        let value = match name.as_str() {
            "count" => call.count.clone(),
            _ => call.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
        };
        if let Some(value) = value {
            parts.push(InterpPart::Text { value: format!(", {name}: ") });
            parts.push(InterpPart::Expr { expr: escaped(span, value) });
        }
    }
    let details = if parts.is_empty() { lit(span, String::new()) } else { typed(Expr::new(span, ExprNode::StringInterp { parts })) };
    typed(const_send(span, "I18n", "missing_translation_tag", vec![key, details, raises]))
}

fn typed(mut e: Expr) -> Expr {
    e.ty = Some(Ty::Str);
    e
}

/// `ERB::Util.html_escape(value.to_s)`, as an HTML-safe key's values read.
fn escaped(span: Span, value: Expr) -> Expr {
    let to_s = typed(Expr::new(span, ExprNode::Send {
        recv: Some(value),
        method: Symbol::from("to_s"),
        args: vec![],
        block: None,
        parenthesized: false,
    }));
    typed(Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Const {
            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
        })),
        method: Symbol::from("html_escape"),
        args: vec![to_s],
        block: None,
        parenthesized: true,
    }))
}

/// Ruby evaluates each keyword value once, in order, whether or not the
/// translation reads it; folding can repeat, drop or reorder them, so
/// once any option is not a plain read, every option that is not a
/// literal is bound to a local first, in source order. The one exception
/// is a sole such option among literals, read exactly once by a single
/// template, which already evaluates once and in order.
fn bind_options(call: &mut Call, translation: &Translation, span: Span) -> Vec<Expr> {
    let option = |call: &Call, name: &str| match name {
        "count" => call.count.clone(),
        _ => call.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
    };
    let effectful: Vec<String> = call.order.iter().filter(|n| option(call, n).is_some_and(|v| !is_plain(&v))).cloned().collect();
    let unbound: Vec<String> = call
        .order
        .iter()
        .filter(|n| option(call, n).is_some_and(|v| !matches!(&*v.node, ExprNode::Lit { .. })))
        .cloned()
        .collect();
    if effectful.is_empty() {
        return Vec::new();
    }
    if let ([only], [_], Translation::One(template)) = (effectful.as_slice(), unbound.as_slice(), translation) {
        let reads = crate::i18n::pieces(template)
            .map(|p| p.iter().filter(|x| matches!(x, Piece::Name(n) if n == only)).count())
            .unwrap_or(0);
        if reads == 1 && call.lookup.interpolates() {
            return Vec::new();
        }
    }
    let mut assigns = Vec::new();
    for name in unbound {
        let local = Symbol::from(format!("__i18n_{name}_{}", span.start));
        let value = option_mut(call, &name).expect("collected above");
        let read = Expr { ty: value.ty.clone(), ..Expr::new(span, ExprNode::Var { id: VarId(0), name: local.clone() }) };
        let value = std::mem::replace(value, read);
        assigns.push(Expr::new(span, ExprNode::Assign { target: LValue::Var { id: VarId(0), name: local }, value }));
    }
    assigns
}

fn option_mut<'a>(call: &'a mut Call, name: &str) -> Option<&'a mut Expr> {
    match name {
        "count" => call.count.as_mut(),
        _ => call.values.iter_mut().find(|(n, _)| n == name).map(|(_, v)| v),
    }
}

/// A read that evaluates to the same value each time it is repeated.
fn is_plain(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. })
}

/// `template` with each `%{name}` replaced by its value's `to_s`, or
/// as written when the call passes no value.
fn interpolate(span: Span, template: &str, call: &Call, html: bool) -> Expr {
    let pieces = match call.lookup.interpolates() {
        true => crate::i18n::pieces(template).unwrap_or_else(|_| vec![Piece::Text(template.to_string())]),
        false => vec![Piece::Text(template.to_string())],
    };
    let mut parts = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(value) => parts.push(InterpPart::Text { value }),
            Piece::Name(name) => {
                let value = if name == "count" { call.count.clone() } else { call.values.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()) };
                let value = value.expect("resolve refuses a name the call does not pass");
                parts.push(InterpPart::Expr { expr: if html { escaped(span, value) } else { value } });
            }
        }
    }
    let mut e = match parts.as_slice() {
        [] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: String::new() } }),
        [InterpPart::Text { value }] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.clone() } }),
        _ => Expr::new(span, ExprNode::StringInterp { parts }),
    };
    e.ty = Some(Ty::Str);
    e
}

#[cfg(test)]
mod tree_answer_tests {
    use super::*;
    use crate::i18n::FallbackPolicy;

    // The original eager path: build all locale/environment variants,
    // then check whether every one answers a plain subtree.
    fn eager(catalog: &Catalog, full: &str) -> bool {
        let mut locales = catalog.locales();
        for other in &catalog.others {
            if !locales.contains(&other.locale) {
                locales.push(other.locale.clone());
            }
        }
        let lands = |c: &Catalog| c.entries.contains_key(full) || c.opaque.contains(full) || c.is_subtree(full);
        catalog.variants(&locales).iter().all(|v| {
            let chain = match &v.fallbacks {
                Some(defaults) => crate::i18n::fallback_chain(&v.locale, defaults),
                None => vec![v.locale.clone()],
            };
            chain.iter().filter_map(|l| catalog.catalog_for(l)).find(|c| lands(c)).is_some_and(|c| c.plain_subtree(full))
        })
    }

    #[test]
    fn lazy_tree_answers_match_eager_locale_variants() {
        let source = "en:\n  shared:\n    title: Shared\n    empty: {}\n  menu:\n    title: Menu\n    empty: {}\n  mixed:\n    title: Text\n    count: 2\n  only_en:\n    title: English\n  changed:\n    title: Text\nfr:\n  shared:\n    title: Partage\n    empty: {}\n  menu:\n    title: Carte\n    empty: {}\n  mixed:\n    title: Texte\n    count: 3\n  changed: false\nfr-CA:\n  shared:\n    title: Partage\n    empty: {}\n  regional:\n    title: Quebec\n";
        let mut files = vec![("locales.yml".into(), source.into())];
        let (mut catalog, problems) = Catalog::load("en", &mut files);
        assert!(problems.is_empty());
        catalog.load_locales(vec!["en".into(), "fr".into(), "fr-CA".into()], FallbackPolicy::default(), &[], &mut files);
        let recv = Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("I18n")] });
        let keys = ["", "shared", "shared.empty", "menu", "menu.title", "menu.empty", "mixed", "mixed.count", "only_en", "changed", "regional", "missing", "men", "menu."];
        let settings = [None, Some(Vec::new()), Some(vec!["en".into()]), Some(vec!["fr-CA".into(), "en".into()])];
        let (mut positive, mut negative) = (0, 0);
        for default in &settings {
            for environment in &settings {
                catalog.fallbacks = FallbackPolicy {
                    app: default.clone(),
                    envs: vec![("production".into(), environment.clone()), ("test".into(), default.clone())],
                    ..Default::default()
                };
                for duplicate in [false, true] {
                    let mut c = catalog.clone();
                    if duplicate {
                        // fr-CA remains in `others` even when it is not available.
                        c.available.retain(|l| l != "fr-CA");
                        c.available.extend(["en".into(), "fr".into()]);
                        c.others.push(c.others[0].clone());
                    }
                    for key in keys {
                        let args = [lit(Span::synthetic(), key.into())];
                        let expected = eager(&c, key).then(|| key.to_string());
                        if expected.is_some() { positive += 1 } else { negative += 1 }
                        for method in ["t", "translate"] {
                            assert_eq!(tree_answer(&c, Some(&recv), method, &args, None), expected,
                                "{key:?}, {default:?}, {environment:?}, duplicate={duplicate}");
                        }
                    }
                }
            }
        }
        assert!(positive > 0 && negative > 0);
    }
}

#[cfg(test)]
mod phase_cache_tests {
    use super::*;

    #[test]
    fn nested_phases_do_not_reuse_another_catalogs_answer() {
        let catalog = |text: &str| {
            let mut c = Catalog::default();
            c.locale = "en".into();
            c.entries.insert("label".into(), text.into());
            c
        };
        let (first, second) = (catalog("first"), catalog("second"));
        let call = Call { lookup: Translate { key: "label".into(), ..Default::default() },
            count: None, values: vec![], order: vec![], dynamic: None, locale: None };
        with_plan_cache(|| {
            let expected = plans(&first, &call, false).unwrap();
            assert_eq!(plans_at(&first, &call, false).unwrap(), expected);
            with_plan_cache(|| assert_eq!(plans_at(&second, &call, false).unwrap(), plans(&second, &call, false).unwrap()));
            assert_eq!(plans_at(&first, &call, false).unwrap(), expected);
        });
    }
}
