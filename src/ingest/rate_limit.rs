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
//! WHAT IS LEFT. `store:`, a `by:` or `with:` that is not a lambda, or a
//! `name:` that is not a literal — the macro stays
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
    if_cond: Option<Symbol>,
    unless_cond: Option<Symbol>,
    if_cond_expr: Option<Expr>,
    unless_cond_expr: Option<Expr>,
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
        // Isolated in its own `prism::scope` — never the outer one
        // that spans the whole app's ingest — so a bug in the
        // generated method source can't render its parse errors
        // against an unrelated real file (see `ingest::sources`'s
        // module doc).
        let (result, diags) = crate::ingest::prism::scope(|| {
            super::controller::ingest_controller(src.as_bytes(), "<rate_limit>")
        });
        let parsed = match (result, diags.is_empty()) {
            (Ok(Some(c)), true) => c,
            (Ok(None), true) => continue,
            (Ok(_), false) => {
                super::survey::record_synthesis_failure(
                    "<rate_limit>",
                    &format!("rate_limit forwarder for `{}`", controller.name.0.as_str()),
                    &diags,
                );
                continue;
            }
            (Err(err), _) => {
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
            if_cond: limit.if_cond.clone(),
            unless_cond: limit.unless_cond.clone(),
            if_cond_expr: limit.if_cond_expr.clone(),
            unless_cond_expr: limit.unless_cond_expr.clone(),
            block: None,
            prepend: false,
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
/// scope: …, only: […], except: […]` → its parts, or None for any call this is
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
    let mut scope_src: Option<String> = None;
    let mut only: Vec<Symbol> = Vec::new();
    let mut except: Vec<Symbol> = Vec::new();
    let mut if_cond = None;
    let mut unless_cond = None;
    let mut if_cond_expr = None;
    let mut unless_cond_expr = None;
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
            // Rails defaults the cache-key scope to this controller's
            // path. `scope:` replaces that, so two controllers can share
            // one budget: a symbol, a string, or a call such as
            // `OtherController.controller_path`.
            "scope" => scope_src = Some(scope_source(v)?),
            "only" => only = symbol_list(v)?,
            "except" => except = symbol_list(v)?,
            "if" => match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => if_cond = Some(value.clone()),
                // The guard evaluates the predicate. Storing the lambda
                // would test the lambda object, which is always truthy.
                // A parameter has no binding once the body is inlined.
                ExprNode::Lambda { params, rest_param, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && block_param.is_none() =>
                {
                    if_cond_expr = Some((*body).clone())
                }
                _ => return None,
            },
            "unless" => match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => unless_cond = Some(value.clone()),
                ExprNode::Lambda { params, rest_param, block_param, body, .. }
                    if params.is_empty() && rest_param.is_none() && block_param.is_none() =>
                {
                    unless_cond_expr = Some((*body).clone())
                }
                _ => return None,
            },
            // A custom store changes which counter increments. The shared
            // limiter has no store argument, so expanding without it would
            // silently use the default cache.
            "store" => return None,
            _ => return None,
        }
    }
    let method = match (&name, only.as_slice()) {
        (Some(n), _) => format!("rate_limit_{}", crate::naming::snake_case(n)),
        (None, [one]) => format!("rate_limit_{}", one.as_str()),
        (None, _) => "rate_limit".to_string(),
    };
    // Rails: ["rate-limit", scope || controller_path, name, by].compact.join(":")
    let key_src = match scope_src {
        Some(scope) => {
            let mut parts = vec![format!(
                "{} + ({}).to_s",
                ruby_string_literal("rate-limit:"),
                scope
            )];
            if let Some(n) = &name {
                parts.push(ruby_string_literal(&format!(":{n}:")));
            } else {
                parts.push(ruby_string_literal(":"));
            }
            parts.push(format!(
                "({}).to_s",
                by_src.unwrap_or_else(|| "request.remote_ip".to_string())
            ));
            parts.join(" + ")
        }
        None => {
            let mut prefix = format!("rate-limit:{controller_path}");
            if let Some(n) = &name {
                prefix.push(':');
                prefix.push_str(n);
            }
            prefix.push(':');
            format!(
                "{} + ({}).to_s",
                ruby_string_literal(&prefix),
                by_src.unwrap_or_else(|| "request.remote_ip".to_string())
            )
        }
    };
    Some(Limit {
        method,
        to_src: to_src?,
        within_src: within_src?,
        key_src,
        with_src: with_src.unwrap_or_else(|| "head(:too_many_requests)".to_string()),
        only,
        except,
        if_cond,
        unless_cond,
        if_cond_expr,
        unless_cond_expr,
    })
}

/// `scope:` as the cache-key segment Rails joins. A symbol or string is
/// that segment; a call (`SessionsController.controller_path`) is
/// evaluated, which is how one controller names another's path.
fn scope_source(v: &Expr) -> Option<String> {
    // A symbol or string is captured when the macro expands, which is
    // when Rails evaluates `scope:`. A call is not: Rails would run it
    // once at declaration, and a nil or false result would fall back to
    // this controller's path. Emitting the call inside the request would
    // re-run it, and a nil result would collapse unrelated budgets.
    match &*v.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(ruby_string_literal(value.as_str())),
        ExprNode::Lit { value: Literal::Str { value } } => Some(ruby_string_literal(value)),
        _ => None,
    }
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
    // A double-quoted literal interpolates `#{…}`, `#@ivar`, and
    // `#$global`. The scope was a literal, so each marker stays text.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::with_capacity(escaped.len());
    let chars: Vec<char> = escaped.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && matches!(chars.get(i + 1), Some('{' | '@' | '$')) {
            out.push('\\');
        }
        out.push(chars[i]);
        i += 1;
    }
    format!("\"{out}\"")
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
    fn a_shared_scope_replaces_the_controller_path_in_the_cache_key() {
        // Two controllers count against one budget when the second names
        // the first's path, a symbol, or a string. Rails joins
        // `scope || controller_path` between `rate-limit` and `name`.
        // A call is evaluated by Rails when the filter is declared.
        // Expanding it into the request method would re-run it, and a
        // nil result would collapse this controller's budget into
        // another. The macro stays in place.
        let mut shared = controller(
            "class Users::Sessions::OtpsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, only: :create, scope: Users::SessionsController.controller_path\nend\n",
        );
        assert!(take_from_controller_body(&mut shared).is_empty());

        let mut symbol = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: :api_global\nend\n",
        );
        let symbol = &take_from_controller_body(&mut symbol)[0];
        assert_eq!(
            symbol.key_src,
            "\"rate-limit:\" + (\"api_global\").to_s + \":\" + (request.remote_ip).to_s"
        );

        let mut named = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, name: \"burst\", scope: \"api\"\nend\n",
        );
        let named = &take_from_controller_body(&mut named)[0];
        assert!(named.key_src.contains("\"api\""), "{}", named.key_src);
        assert!(named.key_src.contains(":burst:"), "{}", named.key_src);

        let mut marked = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: \"budget-\\#{id}\"\nend\n",
        );
        let marked = &take_from_controller_body(&mut marked)[0];
        assert!(
            marked.key_src.contains("\\#{"),
            "a literal interpolation marker stays literal; {}",
            marked.key_src
        );

        // Single quotes keep `#@` and `#$` as text. Emitting them inside
        // a double-quoted key would interpolate the ivar or the global.
        let mut shorthand = controller(
            "class Api::TokensController < ApplicationController\n  \
             rate_limit to: 100, within: 5.minutes, scope: '#@token #$budget'\nend\n",
        );
        let shorthand = &take_from_controller_body(&mut shorthand)[0];
        assert!(
            shorthand.key_src.contains("\\#@token") && shorthand.key_src.contains("\\#$budget"),
            "shorthand interpolation markers stay literal; {}",
            shorthand.key_src
        );
    }

    #[test]
    fn an_unknown_option_leaves_the_macro_in_place() {
        let mut c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, bogus: true\nend\n",
        );
        assert!(take_from_controller_body(&mut c).is_empty());
        assert!(c.body.iter().any(|i| matches!(i, ControllerBodyItem::Unknown { .. })));
    }

    #[test]
    fn if_unless_and_by_lambda_are_kept() {
        let mut c = controller(
            "class TokensController < ApplicationController\n  \
             rate_limit to: 5, within: 1.minute, only: :create, if: -> { @credential }, unless: :quiet?, by: -> { @token }\nend\n",
        );
        let limits = take_from_controller_body(&mut c);
        assert_eq!(limits.len(), 1);
        let limit = &limits[0];
        assert_eq!(limit.only, vec![Symbol::from("create")]);
        assert!(limit.if_cond_expr.is_some());
        assert_eq!(limit.unless_cond.as_ref().map(|s| s.as_str()), Some("quiet?"));
        assert!(limit.key_src.contains("@token"), "{}", limit.key_src);
        let filter = c.body.iter().find_map(|item| match item {
            ControllerBodyItem::Filter { filter, .. } => Some(filter),
            _ => None,
        }).expect("filter");
        assert!(filter.if_cond_expr.is_some());
        assert_eq!(filter.unless_cond.as_ref().map(|s| s.as_str()), Some("quiet?"));
    }

    #[test]
    fn a_custom_store_is_not_dropped() {
        let mut c = controller(
            "class SessionsController < ApplicationController\n  \
             rate_limit to: 10, within: 3.minutes, store: custom_store\nend\n",
        );
        assert!(take_from_controller_body(&mut c).is_empty());
        assert!(c.body.iter().any(|item| matches!(item, ControllerBodyItem::Unknown { .. })));
    }
}
