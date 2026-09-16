//! `rate_limit to: N, within: D, …` → a `before_action` and the private
//! method it runs.
//!
//! actionpack's `ActionController::RateLimiting` (7.2+; the Rails 8
//! authentication generator writes one on `SessionsController#create`
//! and one on `PasswordsController#create`, so the Rails Guides store
//! carries two) is one filter:
//!
//! ```ruby
//! rate_limit to: 10, within: 3.minutes, only: :create,
//!            with: -> { redirect_to new_session_path, alert: "Try again later." }
//! ```
//!
//! ```ruby
//! before_action -> { rate_limiting(to:, within:, by:, with:, store:, name:) }, **options
//!
//! def rate_limiting(to:, within:, by:, with:, store:, name:)
//!   cache_key = ["rate-limit", controller_path, name, instance_exec(&by)].compact.join(":")
//!   count = store.increment(cache_key, 1, expires_in: within)
//!   instance_exec(&with) if count && count > to
//! end
//! ```
//!
//! Generated here as a real method and a real filter, the way
//! `allow_browser` is, because every consumer downstream reads methods
//! and filters and none reads a class-body macro:
//!
//! ```ruby
//! def rate_limit_create
//!   if ActionController::RateLimiter.exceeded?("rate-limit:sessions:" + (request.remote_ip).to_s, 180, 10)
//!     redirect_to new_session_path, alert: "Try again later."
//!   end
//! end
//! ```
//!
//! plus `before_action :rate_limit_create` with the macro's `only:` /
//! `except:`. The counter and its window live in the shared runtime
//! (`ActionController::RateLimiter` over `Rails::Cache#increment_str`),
//! on the same store the fragment cache uses, as in Rails.
//!
//! WHAT IS FOLDED AT INGEST. `within:` written as `<int>.<unit>` becomes
//! the seconds literal, so the generated method carries no Duration;
//! any other expression is forwarded through `.to_i`. `by:` and `with:`
//! must be `->` lambdas — their bodies are re-rendered to source and
//! ingested inside the method — and default to `request.remote_ip` and
//! `head :too_many_requests`, actionpack's own defaults. `controller_path`
//! is the class name underscored, which is what Rails derives too.
//!
//! WHAT IS LEFT. `if:`/`unless:`, `store:`, a `by:` or `with:` that is
//! not a lambda, or a `name:` that is not a literal — the macro stays
//! where it was and the survey names it, which is the contract for a
//! class-body macro roundhouse cannot expand (`report_unrecognized_controller_macros`).
//! The method reaches the ruby-family lanes fully; a strict target
//! carries the `RateLimiter` call as a runtime seam, the posture
//! `allow_browser`'s concern form already has.

use crate::dialect::{Controller, ControllerBodyItem, Filter, FilterKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

struct Limit {
    method: String,
    to_src: String,
    within_src: String,
    key_src: String,
    with_src: String,
    only: Vec<Symbol>,
    except: Vec<Symbol>,
}

pub fn lower_rate_limit(app: &mut crate::App) {
    for controller in &mut app.controllers {
        let limits = take_from_controller_body(controller);
        if limits.is_empty() {
            continue;
        }
        let mut methods = String::new();
        for l in &limits {
            methods.push_str(&method_source(l));
        }
        let src = format!(
            "class {} < ApplicationController\n  private\n{}end\n",
            controller.name.0.as_str(),
            methods
        );
        let parsed = match super::controller::ingest_controller(src.as_bytes(), "<rate_limit>") {
            Ok(Some(c)) => c,
            Ok(None) => continue,
            Err(err) => {
                super::survey::record(&err);
                continue;
            }
        };
        let has_private_marker = controller
            .body
            .iter()
            .any(|item| matches!(item, ControllerBodyItem::PrivateMarker { .. }));
        if !has_private_marker {
            controller.body.push(ControllerBodyItem::PrivateMarker {
                leading_comments: Vec::new(),
                leading_blank_line: true,
            });
        }
        for item in parsed.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                controller.body.push(ControllerBodyItem::Action {
                    action,
                    leading_comments: Vec::new(),
                    leading_blank_line: true,
                });
            }
        }
    }
}

/// ```ruby
///   def rate_limit_create
///     if ActionController::RateLimiter.exceeded?(<key>, <within>, <to>)
///       <with>
///     end
///   end
/// ```
fn method_source(l: &Limit) -> String {
    format!(
        "  def {}\n    if ActionController::RateLimiter.exceeded?({}, {}, {})\n      {}\n    end\n  end\n",
        l.method, l.key_src, l.within_src, l.to_src, l.with_src
    )
}

/// Replace every expandable `rate_limit` in the class body with its
/// filter; return what each expands to. Ones this cannot read stay.
fn take_from_controller_body(controller: &mut Controller) -> Vec<Limit> {
    let path = crate::naming::underscore(
        controller.name.0.as_str().strip_suffix("Controller").unwrap_or(controller.name.0.as_str()),
    );
    let mut found: Vec<Limit> = Vec::new();
    for item in controller.body.iter_mut() {
        let ControllerBodyItem::Unknown { expr, leading_comments, leading_blank_line } = item else {
            continue;
        };
        let Some(mut limit) = limit_from_call(expr, &path) else { continue };
        // Two macros on one controller (or one without a `name:`) must
        // not collide on the method name; the ordinal keeps them apart.
        if found.iter().any(|f| f.method == limit.method) {
            limit.method = format!("{}_{}", limit.method, found.len() + 1);
        }
        let f = Filter {
            target_span: crate::span::Span::synthetic(),
            kind: FilterKind::Before,
            target: Symbol::from(limit.method.as_str()),
            from_concern: None,
            only: limit.only.clone(),
            except: limit.except.clone(),
            only_style: Default::default(),
            except_style: Default::default(),
            if_cond: None,
            unless_cond: None,
            if_cond_expr: None,
            unless_cond_expr: None,
            block: None,
        };
        *item = ControllerBodyItem::Filter {
            filter: f,
            leading_comments: std::mem::take(leading_comments),
            leading_blank_line: *leading_blank_line,
        };
        found.push(limit);
    }
    found
}

/// `rate_limit to: N, within: D, by: -> {…}, with: -> {…}, name: "…",
/// only: […], except: […]` → its parts, or None for any call this is
/// not, or an option it cannot expand.
fn limit_from_call(call: &Expr, controller_path: &str) -> Option<Limit> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    if method.as_str() != "rate_limit" {
        return None;
    }
    let [opts] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return None };
    let mut to_src: Option<String> = None;
    let mut within_src: Option<String> = None;
    let mut by_src: Option<String> = None;
    let mut with_src: Option<String> = None;
    let mut name: Option<String> = None;
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
        match key.as_str() {
            "to" => to_src = Some(crate::emit::ruby::expr::emit_expr(v)),
            "within" => within_src = Some(seconds_source(v)),
            "by" => by_src = Some(lambda_body_source(v)?),
            "with" => with_src = Some(lambda_body_source(v)?),
            "name" => {
                let ExprNode::Lit { value: Literal::Str { value } } = &*v.node else { return None };
                name = Some(value.clone());
            }
            "only" => only = symbol_list(v)?,
            "except" => except = symbol_list(v)?,
            _ => return None,
        }
    }
    let method = match (&name, only.as_slice()) {
        (Some(n), _) => format!("rate_limit_{}", crate::naming::snake_case(n)),
        (None, [one]) => format!("rate_limit_{}", one.as_str()),
        (None, _) => "rate_limit".to_string(),
    };
    // Rails: ["rate-limit", controller_path, name, by].compact.join(":")
    let mut prefix = format!("rate-limit:{controller_path}");
    if let Some(n) = &name {
        prefix.push(':');
        prefix.push_str(n);
    }
    prefix.push(':');
    let key_src = format!(
        "{} + ({}).to_s",
        ruby_string_literal(&prefix),
        by_src.unwrap_or_else(|| "request.remote_ip".to_string())
    );
    Some(Limit {
        method,
        to_src: to_src?,
        within_src: within_src?,
        key_src,
        with_src: with_src.unwrap_or_else(|| "head(:too_many_requests)".to_string()),
        only,
        except,
    })
}

/// The body of a `-> { … }` as source; None for anything else.
fn lambda_body_source(v: &Expr) -> Option<String> {
    let ExprNode::Lambda { body, .. } = &*v.node else { return None };
    Some(crate::emit::ruby::expr::emit_expr(body))
}

/// `within:` as seconds: `3.minutes` folds to `180`; anything else is
/// forwarded through `.to_i`, which reads seconds off an Integer and
/// off an `ActiveSupport::Duration` alike.
fn seconds_source(v: &Expr) -> String {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*v.node {
        if args.is_empty() {
            if let ExprNode::Lit { value: Literal::Int { value: n } } = &*r.node {
                let per = match method.as_str() {
                    "second" | "seconds" => Some(1),
                    "minute" | "minutes" => Some(60),
                    "hour" | "hours" => Some(3600),
                    "day" | "days" => Some(86400),
                    "week" | "weeks" => Some(7 * 86400),
                    _ => None,
                };
                if let Some(per) = per {
                    return (n * per).to_string();
                }
            }
        }
    }
    format!("({}).to_i", crate::emit::ruby::expr::emit_expr(v))
}

fn ruby_string_literal(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `:create` / `[:create, :update]` → the names; anything else → None.
fn symbol_list(v: &Expr) -> Option<Vec<Symbol>> {
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(vec![value.clone()]),
        ExprNode::Array { elements, .. } => elements
            .iter()
            .map(|e| match &*e.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(src: &str) -> Controller {
        super::super::controller::ingest_controller(src.as_bytes(), "app/controllers/sessions_controller.rb")
            .unwrap()
            .unwrap()
    }

    #[test]
    fn the_store_form_expands_to_a_filter_and_a_method() {
        let mut c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, only: :create, with: -> { redirect_to new_session_path, alert: \"Try again later.\" }\n  \
             def create\n  end\nend\n",
        );
        let limits = take_from_controller_body(&mut c);
        assert_eq!(limits.len(), 1);
        let l = &limits[0];
        assert_eq!(l.method, "rate_limit_create");
        assert_eq!(l.to_src, "10");
        assert_eq!(l.within_src, "180");
        assert_eq!(l.key_src, "\"rate-limit:sessions:\" + (request.remote_ip).to_s");
        assert!(l.with_src.starts_with("redirect_to"), "{}", l.with_src);
        assert_eq!(l.only, vec![Symbol::from("create")]);
        let filter = c.body.iter().find_map(|i| match i {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        });
        assert_eq!(filter.unwrap().target.as_str(), "rate_limit_create");
        let src = method_source(l);
        assert!(src.contains("ActionController::RateLimiter.exceeded?(\"rate-limit:sessions:\" + (request.remote_ip).to_s, 180, 10)"), "{src}");
    }

    #[test]
    fn defaults_and_the_key_name_follow_rails() {
        let mut c = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 5, within: 1.hour, by: -> { params[:email] }, name: \"signup\"\nend\n",
        );
        let limits = take_from_controller_body(&mut c);
        let l = &limits[0];
        assert_eq!(l.method, "rate_limit_signup");
        assert_eq!(l.within_src, "3600");
        assert_eq!(l.key_src, "\"rate-limit:api/tokens:signup:\" + (params[:email]).to_s");
        assert_eq!(l.with_src, "head(:too_many_requests)");
        assert!(l.only.is_empty());
    }

    #[test]
    fn an_option_it_cannot_expand_leaves_the_macro_in_place() {
        let mut c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, if: :guest?\nend\n",
        );
        assert!(take_from_controller_body(&mut c).is_empty());
        assert!(c.body.iter().any(|i| matches!(i, ControllerBodyItem::Unknown { .. })));
    }
}
