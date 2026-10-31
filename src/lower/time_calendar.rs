// Not reopened on `Time` / `Date` (no built-in reopening, and spinel cannot
// dispatch one): each extension grounds to a function in
// runtime/ruby/active_support_ext.rb (Time) or the date-gated
// runtime/spinel/active_support_date_parsing.rb (`date_*` family).
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

pub fn apply_time_calendar_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    // Crystal prints these names literally. The rewrite used to live in
    // `emit/crystal/expr.rs`. `time_current` calls the same function on
    // the node it just built (`Time.current`, `to_fs(:iso8601)`).
    ground_crystal_time_bridge(expr);
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node {
        let time_class = matches!(&*r.node, ExprNode::Const { path }
            if path.last().is_some_and(|s| s.as_str() == "Time")
                && path.iter().rev().skip(1).all(|s| s.as_str().is_empty()));
        if time_class && method.as_str() == "find_zone" && args.len() == 1 {
            *r = Expr::new(r.span, ExprNode::Const {
                path: vec![Symbol::from("ActiveSupport"), Symbol::from("TimeZone")],
            });
            *method = Symbol::from("lookup");
            return;
        }
    }
    // Not `TimeZone[name]`: a class-side index is an operator on the strict targets; the runtime port answers it as `lookup`.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node {
        if method.as_str() == "[]" && args.len() == 1 && is_time_zone_class(r) {
            *method = Symbol::from("lookup");
            return;
        }
        if method.as_str() == "formatted_offset" && args.is_empty() && matches!(r.ty, Some(Ty::Time)) {
            let time = r.clone();
            *expr.node = active_support_call("formatted_offset", vec![time]);
            expr.ty = Some(Ty::Str);
            return;
        }
    }
    if let ExprNode::Send { recv: Some(r), method, .. } = &mut *expr.node {
        if is_time_const(r) && method.as_str() == "use_zone" {
            *r = Expr::new(r.span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] });
            return;
        }
    }
    let ExprNode::Send {
        recv: Some(r),
        method,
        args,
        block: None,
        ..
    } = &*expr.node
    else {
        return;
    };
    if is_time_zone_send(r) && args.is_empty() {
        let today = || time_expr(active_support_call("beginning_of_day", vec![now()]));
        let grounded = match method.as_str() {
            "now" => active_support_call("now", vec![]),
            "today" => active_support_call("beginning_of_day", vec![now()]),
            "yesterday" => active_support_call("yesterday", vec![today()]),
            "tomorrow" => active_support_call("tomorrow", vec![today()]),
            _ => return,
        };
        *expr.node = grounded;
        expr.ty = Some(Ty::Time);
        return;
    }
    // Rails `Date.current` / `Date.yesterday` / `Date.tomorrow` — class-side.
    if is_date_const(r) && args.is_empty() {
        let grounded = match method.as_str() {
            "current" => active_support_call("date_current", vec![now()]),
            "yesterday" => {
                active_support_call("date_yesterday", vec![date_expr(active_support_call("date_current", vec![now()]))])
            }
            "tomorrow" => {
                active_support_call("date_tomorrow", vec![date_expr(active_support_call("date_current", vec![now()]))])
            }
            _ => return,
        };
        *expr.node = grounded;
        expr.ty = Some(Ty::Date);
        return;
    }
    // `Integer#in_time_zone` — epoch seconds (ActiveSupport Numeric).
    if is_int_value(r) && method.as_str() == "in_time_zone" && args.len() <= 1 {
        let epoch = time_expr(active_support_call("time_at_epoch", vec![r.clone()]));
        let grounded = match args.first() {
            Some(zone) => active_support_call("in_time_zone", vec![epoch, zone_name_arg(zone)]),
            None => active_support_call("present", vec![epoch]),
        };
        *expr.node = grounded;
        expr.ty = Some(Ty::Time);
        return;
    }
    if is_date_value(r) {
        let recv = r.clone();
        let method_name = method.as_str().to_string();
        let call_args = args.to_vec();
        rewrite_date_value(expr, &recv, &method_name, &call_args);
        return;
    }
    if !is_time_value(r) {
        return;
    }
    if method.as_str() == "change"
        && matches!(r.ty, Some(Ty::Time))
        && let Some(usec) = change_usec_arg(args)
    {
        *expr.node = active_support_call("change_time_usec", vec![r.clone(), usec]);
        expr.ty = Some(Ty::Time);
        return;
    }
    if method.as_str() == "to_date" && args.is_empty() {
        *expr.node = active_support_call("date_current", vec![r.clone()]);
        expr.ty = Some(Ty::Date);
        return;
    }
    if method.as_str() == "in_time_zone" && args.len() <= 1 {
        let grounded = match args.first() {
            Some(zone) => active_support_call("in_time_zone", vec![r.clone(), zone_name_arg(zone)]),
            None => active_support_call("present", vec![r.clone()]),
        };
        *expr.node = grounded;
        return;
    }
    if let (Some(unit), true) = (all_range_unit(method.as_str()), args.is_empty()) {
        let edge = |side: &str| time_expr(active_support_call(&format!("{side}_of_{unit}"), vec![r.clone()]));
        *expr.node = ExprNode::Range { begin: Some(edge("beginning")), end: Some(edge("end")), exclusive: false };
        return;
    }
    let weekday = match method.as_str() {
        "sunday?" => Some(0),
        "monday?" => Some(1),
        "tuesday?" => Some(2),
        "wednesday?" => Some(3),
        "thursday?" => Some(4),
        "friday?" => Some(5),
        "saturday?" => Some(6),
        _ => None,
    };
    if let (Some(n), true) = (weekday, args.is_empty()) {
        let lit = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Int { value: n },
            },
        );
        *expr.node = active_support_call("on_wday?", vec![r.clone(), lit]);
        return;
    }
    let (target, max_args) = match method.as_str() {
        "beginning_of_minute" | "at_beginning_of_minute" => ("beginning_of_minute", 0),
        "end_of_minute" | "at_end_of_minute" => ("end_of_minute", 0),
        "beginning_of_hour" | "at_beginning_of_hour" => ("beginning_of_hour", 0),
        "end_of_hour" | "at_end_of_hour" => ("end_of_hour", 0),
        "beginning_of_day" | "at_beginning_of_day" | "midnight" | "at_midnight" => {
            ("beginning_of_day", 0)
        }
        "end_of_day" | "at_end_of_day" => ("end_of_day", 0),
        "noon" | "at_noon" | "middle_of_day" | "at_middle_of_day" => ("noon", 0),
        "beginning_of_week" | "at_beginning_of_week" => ("beginning_of_week", 1),
        "end_of_week" | "at_end_of_week" => ("end_of_week", 1),
        "beginning_of_month" | "at_beginning_of_month" => ("beginning_of_month", 0),
        "end_of_month" | "at_end_of_month" => ("end_of_month", 0),
        "beginning_of_year" | "at_beginning_of_year" => ("beginning_of_year", 0),
        "end_of_year" | "at_end_of_year" => ("end_of_year", 0),
        "next_week" => ("next_week", 0),
        "prev_week" | "last_week" => ("prev_week", 0),
        "yesterday" => ("yesterday", 0),
        "tomorrow" => ("tomorrow", 0),
        "days_since" | "next_day" => ("days_since", 1),
        "days_ago" | "prev_day" => ("days_ago", 1),
        "weeks_since" => ("weeks_since", 1),
        "weeks_ago" => ("weeks_ago", 1),
        "months_since" | "next_month" => ("months_since", 1),
        "months_ago" | "prev_month" => ("months_ago", 1),
        "last_month" => ("months_ago", 0),
        "years_since" | "next_year" => ("years_since", 1),
        "years_ago" | "prev_year" => ("years_ago", 1),
        "last_year" => ("years_ago", 0),
        "today?" => ("today?", 0),
        "yesterday?" => ("yesterday?", 0),
        "tomorrow?" => ("tomorrow?", 0),
        "past?" => ("past?", 0),
        "future?" => ("future?", 0),
        "on_weekend?" => ("on_weekend?", 0),
        "on_weekday?" => ("on_weekday?", 0),
        _ => return,
    };
    if args.len() > max_args {
        return;
    }
    let Some(args) = calendar_args(target, args) else { return };
    let mut call_args = vec![r.clone()];
    call_args.extend(args);
    if matches!(
        target,
        "today?" | "yesterday?" | "tomorrow?" | "past?" | "future?"
    ) {
        call_args.push(now());
    }
    *expr.node = active_support_call(target, call_args);
}

pub(crate) fn date_calendar_duration(expr: &Expr) -> Option<(&'static str, &Expr)> {
    let ExprNode::Send { recv: Some(count), method, args, block: None, .. } = &*expr.node else { return None };
    if !args.is_empty() || !matches!(count.ty, Some(Ty::Int)) {
        return None;
    }
    let unit = match method.as_str() {
        "day" | "days" => "days",
        "week" | "weeks" => "weeks",
        "month" | "months" => "months",
        "year" | "years" => "years",
        _ => return None,
    };
    Some((unit, count))
}

fn rewrite_date_value(expr: &mut Expr, r: &Expr, method: &str, args: &[Expr]) {
    if matches!(method, "+" | "-") && args.len() == 1
        && let Some((unit, count)) = date_calendar_duration(&args[0])
    {
        let direction = if method == "+" { "since" } else { "ago" };
        *expr.node = active_support_call(&format!("date_{unit}_{direction}"), vec![r.clone(), count.clone()]);
        expr.ty = Some(Ty::Date);
        return;
    }
    if method == "in_time_zone" && args.len() <= 1 {
        let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
        let grounded = match args.first() {
            Some(zone) => active_support_call("in_time_zone", vec![midnight, zone_name_arg(zone)]),
            None => active_support_call("present", vec![midnight]),
        };
        *expr.node = grounded;
        expr.ty = Some(Ty::Time);
        return;
    }
    // Date → Time day edges: convert at midnight, then reuse Time helpers.
    if matches!(
        method,
        "beginning_of_day"
            | "at_beginning_of_day"
            | "midnight"
            | "at_midnight"
            | "end_of_day"
            | "at_end_of_day"
            | "noon"
            | "at_noon"
            | "middle_of_day"
            | "at_middle_of_day"
    ) && args.is_empty()
    {
        let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
        let target = match method {
            "end_of_day" | "at_end_of_day" => "end_of_day",
            "noon" | "at_noon" | "middle_of_day" | "at_middle_of_day" => "noon",
            _ => "beginning_of_day",
        };
        *expr.node = active_support_call(target, vec![midnight]);
        expr.ty = Some(Ty::Time);
        return;
    }
    if let (Some(unit), true) = (all_range_unit(method), args.is_empty()) {
        if unit == "day" {
            let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
            let edge = |side: &str| {
                time_expr(active_support_call(&format!("{side}_of_day"), vec![midnight.clone()]))
            };
            *expr.node = ExprNode::Range {
                begin: Some(edge("beginning")),
                end: Some(edge("end")),
                exclusive: false,
            };
            return;
        }
        let edge = |side: &str| {
            date_expr(active_support_call(&format!("date_{side}_of_{unit}"), vec![r.clone()]))
        };
        *expr.node = ExprNode::Range {
            begin: Some(edge("beginning")),
            end: Some(edge("end")),
            exclusive: false,
        };
        return;
    }
    let weekday = match method {
        "sunday?" => Some(0),
        "monday?" => Some(1),
        "tuesday?" => Some(2),
        "wednesday?" => Some(3),
        "thursday?" => Some(4),
        "friday?" => Some(5),
        "saturday?" => Some(6),
        _ => None,
    };
    if let (Some(n), true) = (weekday, args.is_empty()) {
        let lit = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Int { value: n },
            },
        );
        let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
        *expr.node = active_support_call("on_wday?", vec![midnight, lit]);
        return;
    }
    // Predicates compare calendar days against Date.current (Rails),
    // not midnight-Time vs wall clock (that makes "today" past? after noon).
    if matches!(method, "today?" | "yesterday?" | "tomorrow?" | "past?" | "future?")
        && args.is_empty()
    {
        let current = date_expr(active_support_call("date_current", vec![now()]));
        let target = match method {
            "today?" => "date_today?",
            "yesterday?" => "date_yesterday?",
            "tomorrow?" => "date_tomorrow?",
            "past?" => "date_past?",
            "future?" => "date_future?",
            _ => unreachable!(),
        };
        *expr.node = active_support_call(target, vec![r.clone(), current]);
        return;
    }
    if matches!(method, "on_weekend?" | "on_weekday?") && args.is_empty() {
        let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
        *expr.node = active_support_call(method, vec![midnight]);
        return;
    }
    // `Date + n` / `Date - n` — Spinel Date has no arithmetic; ground to day shifts.
    // Leave `Date - Date` alone (Rational day count; not modeled).
    if matches!(method, "+" | "-")
        && args.len() == 1
        && args[0]
            .ty
            .as_ref()
            .is_none_or(|t| matches!(t, Ty::Int | Ty::Var { .. }))
    {
        let target = if method == "+" {
            "date_days_since"
        } else {
            "date_days_ago"
        };
        *expr.node = active_support_call(target, vec![r.clone(), args[0].clone()]);
        expr.ty = Some(Ty::Date);
        return;
    }
    let (target, max_args) = match method {
        "beginning_of_week" | "at_beginning_of_week" => ("date_beginning_of_week", 1),
        "end_of_week" | "at_end_of_week" => ("date_end_of_week", 1),
        "beginning_of_month" | "at_beginning_of_month" => ("date_beginning_of_month", 0),
        "end_of_month" | "at_end_of_month" => ("date_end_of_month", 0),
        "beginning_of_year" | "at_beginning_of_year" => ("date_beginning_of_year", 0),
        "end_of_year" | "at_end_of_year" => ("date_end_of_year", 0),
        "next_week" => ("date_next_week", 0),
        "prev_week" | "last_week" => ("date_prev_week", 0),
        "yesterday" => ("date_yesterday", 0),
        "tomorrow" => ("date_tomorrow", 0),
        "days_since" | "next_day" => ("date_days_since", 1),
        "days_ago" | "prev_day" => ("date_days_ago", 1),
        "weeks_since" => ("date_weeks_since", 1),
        "weeks_ago" => ("date_weeks_ago", 1),
        "months_since" | "next_month" => ("date_months_since", 1),
        "months_ago" | "prev_month" => ("date_months_ago", 1),
        "last_month" => ("date_months_ago", 0),
        "years_since" | "next_year" => ("date_years_since", 1),
        "years_ago" | "prev_year" => ("date_years_ago", 1),
        "last_year" => ("date_years_ago", 0),
        _ => return,
    };
    if args.len() > max_args {
        return;
    }
    let Some(args) = calendar_args(target, args) else { return };
    let mut call_args = vec![r.clone()];
    call_args.extend(args);
    *expr.node = active_support_call(target, call_args);
    expr.ty = Some(Ty::Date);
}

pub(crate) fn week_start_wday(arg: &Expr) -> Option<i64> {
    // Not `Date.beginning_of_week` (the configured default): only a literal names a day the lowering can fix.
    let ExprNode::Lit { value: Literal::Sym { value } } = &*arg.node else { return None };
    ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"]
        .iter()
        .position(|day| *day == value.as_str())
        .map(|wday| wday as i64)
}

fn calendar_args(target: &str, args: &[Expr]) -> Option<Vec<Expr>> {
    if !target.ends_with("_of_week") {
        return Some(args.to_vec());
    }
    args.iter()
        .map(|arg| {
            week_start_wday(arg).map(|wday| {
                let mut lit = Expr::new(arg.span, ExprNode::Lit { value: Literal::Int { value: wday } });
                lit.ty = Some(Ty::Int);
                lit
            })
        })
        .collect()
}

// Not a runtime Range: spinel's Range holds Integer endpoints only, so `where` has to meet the literal and render it as SQL.
fn all_range_unit(method: &str) -> Option<&'static str> {
    match method {
        "all_day" => Some("day"),
        "all_week" => Some("week"),
        "all_month" => Some("month"),
        "all_year" => Some("year"),
        _ => None,
    }
}

// Not a zone object passed through: `ActiveSupport.in_time_zone` keeps one parameter type, `String?`, so Spinel needs no union for it.
fn zone_name_arg(zone: &Expr) -> Expr {
    let is_zone = |t: &Ty| matches!(t, Ty::Class { id, .. } if id.0.as_str() == "ActiveSupport::TimeZone");
    let zone_object = match &zone.ty {
        Some(Ty::Union { variants }) => {
            variants.iter().any(is_zone) && variants.iter().all(|v| is_zone(v) || matches!(v, Ty::Nil))
        }
        Some(t) => is_zone(t),
        None => false,
    };
    if !zone_object {
        return zone.clone();
    }
    let mut name = Expr::new(Span::synthetic(), active_support_call("zone_name", vec![zone.clone()]));
    name.ty = Some(Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() });
    name
}

fn time_expr(node: ExprNode) -> Expr {
    let mut e = Expr::new(Span::synthetic(), node);
    e.ty = Some(Ty::Time);
    e
}

fn date_expr(node: ExprNode) -> Expr {
    let mut e = Expr::new(Span::synthetic(), node);
    e.ty = Some(Ty::Date);
    e
}

fn now() -> Expr {
    time_expr(active_support_call("now", vec![]))
}

fn is_time_zone_class(expr: &Expr) -> bool {
    matches!(&*expr.node, ExprNode::Const { path }
        if matches!(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice(),
            ["ActiveSupport", "TimeZone"] | ["", "ActiveSupport", "TimeZone"]))
}

fn change_usec_arg(args: &[Expr]) -> Option<Expr> {
    let [options] = args else { return None };
    let ExprNode::Hash { entries, .. } = &*options.node else { return None };
    let [(key, value)] = entries.as_slice() else { return None };
    if matches!(&*key.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "usec")
        && matches!(value.ty, Some(Ty::Int))
    {
        Some(value.clone())
    } else {
        None
    }
}

fn active_support_call(method: &str, args: Vec<Expr>) -> ExprNode {
    let mut recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: vec![Symbol::from("ActiveSupport")],
        },
    );
    recv.ty = Some(Ty::Class {
        id: ClassId(Symbol::from("ActiveSupport")),
        args: vec![].into(),
    });
    ExprNode::Send {
        recv: Some(recv),
        method: Symbol::from(method),
        parenthesized: !args.is_empty(),
        args,
        block: None,
    }
}

/// `Time.now.utc` (zero args, bare or root `Time`) → `Time.utc`.
/// Zero-arg `iso8601` on a time value, or on that clock, → `to_rfc3339`.
///
/// `Foo::Time` is a different constant: `Foo::Time.now.utc` stays a chain.
/// An unknown receiver, a `Date`, and an application class keep `iso8601`
/// — those targets print the method name, and only `Time` grew
/// `to_rfc3339`. `Time.utc(year, …)` and `iso8601(n)` stay: Crystal's
/// clock is the no-arg class method, and an explicit fraction is a
/// different call.
pub(crate) fn ground_crystal_time_bridge(expr: &mut Expr) {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() == "iso8601" && args.is_empty() && is_time_rfc3339_receiver(r) {
        *method = Symbol::from("to_rfc3339");
        expr.decisions |= crate::expr::GROUNDED_TIME_FORMAT;
        return;
    }
    if method.as_str() != "utc" || !args.is_empty() {
        return;
    }
    // The send is already `.utc`. Point it at `Time` instead of rebuilding
    // the node, so `parenthesized` and the surrounding expr stay.
    let Some(time) = time_now_const(r) else { return };
    *r = time;
}

/// A tree spliced in after the postorder walk (`to_fs` format lambdas).
pub(crate) fn ground_crystal_time_bridge_tree(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut ground_crystal_time_bridge_tree);
    ground_crystal_time_bridge(expr);
}

/// The `Time` constant under `Time.now`, cloned so the clock send can
/// reuse it as `Time.utc`.
fn time_now_const(now_send: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(inner), method, args, block: None, .. } = &*now_send.node else {
        return None;
    };
    if method.as_str() != "now" || !args.is_empty() {
        return None;
    }
    if is_root_time_const(inner) { Some(inner.clone()) } else { None }
}

/// Bare `Time` and rooted `::Time`. A namespaced `Foo::Time` is not the
/// stdlib clock — same rule as the TypeScript emitter.
fn is_root_time_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path }
        if path.last().is_some_and(|s| s.as_str() == "Time")
            && path.iter().rev().skip(1).all(|s| s.as_str().is_empty()))
}

/// `Time.now` / `Time.utc` / `Time.now.utc`, including the rooted form.
/// The type stamp is missing on some synthesized chains; the shape is
/// still the stdlib clock.
fn is_stdlib_time_clock(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    match method.as_str() {
        "now" | "utc" if is_root_time_const(r) => true,
        "utc" => is_stdlib_time_clock(r),
        _ => false,
    }
}

/// A zero-arg `to_rfc3339` on this receiver is the grounded time format.
/// Emitters use the same test so an application method of that name
/// keeps normal dispatch.
pub(crate) fn is_time_rfc3339_receiver(recv: &Expr) -> bool {
    if is_root_time_const(recv) {
        return false;
    }
    is_time_value(recv) || is_stdlib_time_clock(recv)
}

fn is_time_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
}

fn is_date_const(e: &Expr) -> bool {
    // Bare `Date` and rooted `::Date` (empty leading path segments) are the
    // top-level constant — see #517.
    matches!(&*e.node, ExprNode::Const { path } if path.last().is_some_and(|s| s.as_str() == "Date")
        && path.iter().rev().skip(1).all(|s| s.as_str().is_empty()))
}

fn is_time_zone_send(e: &Expr) -> bool {
    matches!(&*e.node,
        ExprNode::Send { recv: Some(z), method, args, block: None, .. }
            if method.as_str() == "zone" && args.is_empty() && is_time_const(z))
}

// Not every `Time`-typed receiver: the `Time` constant and `Time.zone` flatten onto the same type as a Time value.
fn is_time_value(e: &Expr) -> bool {
    if is_time_const(e) || is_time_zone_send(e) {
        return false;
    }
    let is_time = |t: &Ty| {
        matches!(t, Ty::Time) || matches!(t, Ty::Class { id, .. } if id.0.as_str() == "Time")
    };
    match &e.ty {
        Some(Ty::Union { variants }) => {
            variants.iter().any(is_time)
                && variants.iter().all(|v| is_time(v) || matches!(v, Ty::Nil))
        }
        Some(t) => is_time(t),
        None => false,
    }
}

fn is_date_value(e: &Expr) -> bool {
    if is_date_const(e) {
        return false;
    }
    let is_date = |t: &Ty| {
        matches!(t, Ty::Date) || matches!(t, Ty::Class { id, .. } if id.0.as_str() == "Date")
    };
    match &e.ty {
        Some(Ty::Union { variants }) => {
            variants.iter().any(is_date)
                && variants.iter().all(|v| is_date(v) || matches!(v, Ty::Nil))
        }
        Some(t) => is_date(t),
        None => false,
    }
}

fn is_int_value(e: &Expr) -> bool {
    let is_int = |t: &Ty| matches!(t, Ty::Int);
    match &e.ty {
        Some(Ty::Union { variants }) => {
            variants.iter().any(is_int)
                && variants.iter().all(|v| is_int(v) || matches!(v, Ty::Nil))
        }
        Some(t) => is_int(t),
        None => false,
    }
}
