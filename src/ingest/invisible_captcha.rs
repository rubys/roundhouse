//! `invisible_captcha only: :create` → a `before_action` and the private
//! method that asks `ActionController::InvisibleCaptcha`.
//!
//! The gem (`invisible_captcha` 2.x) expands to a spam-detection
//! `before_action`. Generated here as a real filter + method, the way
//! `rate_limit` is. Recognized options: `only:`, `except:`, `prepend:`.
//! Unsupported kwargs (`honeypot:`, `on_spam:`, …) leave the call as
//! `Unknown` so the survey still names them.
//!
//! Filters are installed only after private-method synth succeeds, so a
//! failed re-ingest never leaves orphan `before_action`s.

use crate::dialect::{Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::controller_macro_synth::{
    append_private_actions, expr_symbol_list, install_before_filter,
};

struct Captcha {
    method: String,
    only: Vec<Symbol>,
    except: Vec<Symbol>,
    prepend: bool,
}

pub fn lower_invisible_captcha(app: &mut crate::App) {
    for controller in &mut app.controllers {
        let captchas = collect_captchas(controller);
        if captchas.is_empty() {
            continue;
        }
        let mut methods = String::new();
        for (_, c) in &captchas {
            methods.push_str(&method_source(c));
        }
        if !append_private_actions(controller, "<invisible_captcha>", &methods) {
            continue;
        }
        // Indices are still valid: append only pushed at the end.
        for (idx, c) in &captchas {
            if let Some(item) = controller.body.get_mut(*idx) {
                install_before_filter(
                    item,
                    &c.method,
                    c.only.clone(),
                    c.except.clone(),
                    c.prepend,
                    None,
                    None,
                    None,
                    None,
                );
            }
        }
    }
}

fn method_source(c: &Captcha) -> String {
    format!(
        "  def {}\n    if ActionController::InvisibleCaptcha.spam?(params)\n      head :ok\n    end\n  end\n",
        c.method
    )
}

/// Parse expandable `invisible_captcha` calls without mutating the body.
fn collect_captchas(controller: &Controller) -> Vec<(usize, Captcha)> {
    let mut found: Vec<(usize, Captcha)> = Vec::new();
    for (i, item) in controller.body.iter().enumerate() {
        let ControllerBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let Some(mut captcha) = captcha_from_call(expr) else {
            continue;
        };
        if found.iter().any(|(_, f)| f.method == captcha.method) {
            captcha.method = format!("{}_{}", captcha.method, found.len() + 1);
        }
        found.push((i, captcha));
    }
    found
}

fn captcha_from_call(call: &Expr) -> Option<Captcha> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        ..
    } = &*call.node
    else {
        return None;
    };
    if method.as_str() != "invisible_captcha" {
        return None;
    }
    let mut only = Vec::new();
    let mut except = Vec::new();
    let mut prepend = false;
    if args.is_empty() {
        // Bare `invisible_captcha` — all actions.
    } else {
        let [opts] = args.as_slice() else {
            return None;
        };
        let ExprNode::Hash {
            entries,
            kwargs: true,
        } = &*opts.node
        else {
            return None;
        };
        for (k, v) in entries {
            let ExprNode::Lit {
                value: Literal::Sym { value: key },
            } = &*k.node
            else {
                return None;
            };
            match key.as_str() {
                "only" => only = expr_symbol_list(v)?,
                "except" => except = expr_symbol_list(v)?,
                "prepend" => {
                    let ExprNode::Lit {
                        value: Literal::Bool { value: flag },
                    } = &*v.node
                    else {
                        return None;
                    };
                    prepend = *flag;
                }
                _ => return None,
            }
        }
    }
    Some(Captcha {
        method: "detect_invisible_captcha_spam".into(),
        only,
        except,
        prepend,
    })
}
