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
            Some(zone) => active_support_call("in_time_zone", vec![epoch, zone.clone()]),
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
    if method.as_str() == "in_time_zone" && args.len() <= 1 {
        let grounded = match args.first() {
            Some(zone) => active_support_call("in_time_zone", vec![r.clone(), zone.clone()]),
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
        "beginning_of_week" | "at_beginning_of_week" => ("beginning_of_week", 0),
        "end_of_week" | "at_end_of_week" => ("end_of_week", 0),
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
    let mut call_args = vec![r.clone()];
    call_args.extend(args.iter().cloned());
    if matches!(
        target,
        "today?" | "yesterday?" | "tomorrow?" | "past?" | "future?"
    ) {
        call_args.push(now());
    }
    *expr.node = active_support_call(target, call_args);
}

fn rewrite_date_value(expr: &mut Expr, r: &Expr, method: &str, args: &[Expr]) {
    if method == "in_time_zone" && args.len() <= 1 {
        let midnight = time_expr(active_support_call("date_at_midnight", vec![r.clone()]));
        let grounded = match args.first() {
            Some(zone) => active_support_call("in_time_zone", vec![midnight, zone.clone()]),
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
        "beginning_of_week" | "at_beginning_of_week" => ("date_beginning_of_week", 0),
        "end_of_week" | "at_end_of_week" => ("date_end_of_week", 0),
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
    let mut call_args = vec![r.clone()];
    call_args.extend(args.iter().cloned());
    *expr.node = active_support_call(target, call_args);
    expr.ty = Some(Ty::Date);
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

fn active_support_call(method: &str, args: Vec<Expr>) -> ExprNode {
    let mut recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: vec![Symbol::from("ActiveSupport")],
        },
    );
    recv.ty = Some(Ty::Class {
        id: ClassId(Symbol::from("ActiveSupport")),
        args: vec![],
    });
    ExprNode::Send {
        recv: Some(recv),
        method: Symbol::from(method),
        parenthesized: !args.is_empty(),
        args,
        block: None,
    }
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
