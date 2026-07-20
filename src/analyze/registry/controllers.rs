//! ActionController surfaces: the `ActionController::Base.helpers` proxy and
//! the hardcoded `ApplicationController` surface (params/session/render,
//! flash, respond_to, route helpers, and Devise scope helpers). Extracted
//! verbatim from `Analyzer::with_adapter`. The per-app controller class
//! registration (region 22) stays in the orchestrator — it depends on the
//! mod-private `controller_includes` helper and on `ApplicationController`
//! having been inserted here first. Runs after `view::register` because the
//! Devise fold also augments `ActionView::Base`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::App;
use crate::dialect::ModelBodyItem;
use crate::expr::ExprNode;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(
    classes: &mut HashMap<ClassId, ClassInfo>,
    app: &App,
    route_helper_names: &[String],
) {
    // `ActionController::Base.helpers` — the view-helper proxy a model
    // or library reaches for to build paths/URLs outside a request
    // (`ActionController::Base.helpers.image_url(...)` in user.rb). The
    // literal class is unmodeled (controllers carry a hardcoded
    // surface, but `ActionController::Base` itself was never a
    // registered class), so the call errored. `helpers` returns the
    // proxy (gradual — its method surface is the full view-helper set);
    // the other entries are the framework class-side config readers
    // that occasionally appear on the bare base class.
    {
        let mut acb = ClassInfo::default();
        for m in ["helpers", "helper", "default_url_options"] {
            acb.class_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        classes
            .entry(ClassId(Symbol::from("ActionController::Base")))
            .or_insert(acb);
    }

    // Hardcoded ApplicationController-ish surface. Real inheritance chains
    // and per-controller overrides land when a fixture forces them.
    let mut app_ctrl = ClassInfo::default();
    let params_ty = Ty::Hash {
        key: Box::new(Ty::Sym),
        value: Box::new(Ty::Str),
    };
    app_ctrl.class_methods.insert(Symbol::from("params"), params_ty);
    app_ctrl.class_methods.insert(Symbol::from("session"),
        Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) });
    app_ctrl.class_methods.insert(Symbol::from("render"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("redirect_to"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("head"), Ty::Nil);
    // HTTP cache-control declarations (`expires_in 3.minutes,
    // public: true`) — side-effecting header writes.
    app_ctrl.class_methods.insert(Symbol::from("expires_in"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("expires_now"), Ty::Nil);
    // `flash` (FlashHash) and the current action/controller names are
    // available on the controller via implicit self, same as in views.
    app_ctrl.class_methods.insert(
        Symbol::from("flash"),
        Ty::Class {
            id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
            args: vec![],
        },
    );
    for m in ["action_name", "controller_name", "controller_path"] {
        app_ctrl.class_methods.insert(Symbol::from(m), Ty::Str);
    }
    // Route URL helpers (controller side — `redirect_to articles_url`).
    for name in route_helper_names {
        app_ctrl
            .class_methods
            .entry(Symbol::from(name.as_str()))
            .or_insert(Ty::Str);
    }
    // `respond_to do |format| ... end` — yields the mime Collector
    // registered above, so the `format` block param (and `format.html`/
    // `format.json` calls) type. Block-yielding Fn; result is nil.
    app_ctrl.class_methods.insert(
        Symbol::from("respond_to"),
        super::block_fn(
            &Ty::Class {
                id: ClassId(Symbol::from("ActionController::MimeResponds::Collector")),
                args: vec![],
            },
            Ty::Nil,
        ),
    );
    // `request` / `response` / `logger` return framework objects
    // (ActionDispatch::Request, etc.) we don't model structurally.
    // Gradual `Untyped` so chains like `request.referer` /
    // `request.remote_ip` / `request.env[...]` flow through
    // dispatch instead of bottoming out at Var.
    app_ctrl.class_methods.insert(Symbol::from("request"), Ty::Untyped);
    app_ctrl.class_methods.insert(Symbol::from("response"), Ty::Untyped);
    app_ctrl.class_methods.insert(Symbol::from("logger"), Ty::Untyped);
    // Devise scope helpers. A model declaring the `devise` DSL
    // (`class User; devise :registerable, …`) makes Devise generate
    // `current_user` / `user_signed_in?` / `authenticate_user!` on
    // every controller — the app's own declaration is the fact
    // source, no convention guessing. `current_<scope>` is nilable
    // (no signed-in user); the session object is opaque. Without
    // this, `current_user` bottoms out unresolved and cascades into
    // every `@account = current_account`-style controller ivar
    // (343 sites in Mastodon).
    for model in &app.models {
        let declares_devise = model.body.iter().any(|item| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return false };
            matches!(
                &*expr.node,
                ExprNode::Send { recv: None, method, .. } if method.as_str() == "devise"
            )
        });
        if !declares_devise {
            continue;
        }
        let scope = crate::naming::snake_case(
            model.name.0.as_str().rsplit("::").next().unwrap_or(""),
        );
        let model_ty = Ty::Class { id: model.name.clone(), args: vec![] };
        app_ctrl.class_methods.insert(
            Symbol::from(format!("current_{scope}").as_str()),
            Ty::Union { variants: vec![model_ty.clone(), Ty::Nil] },
        );
        app_ctrl.class_methods.insert(
            Symbol::from(format!("{scope}_signed_in?").as_str()),
            Ty::Bool,
        );
        app_ctrl.class_methods.insert(
            Symbol::from(format!("authenticate_{scope}!").as_str()),
            Ty::Nil,
        );
        app_ctrl.class_methods.insert(
            Symbol::from(format!("{scope}_session").as_str()),
            Ty::Untyped,
        );
        for m in ["sign_in", "sign_out", "bypass_sign_in"] {
            app_ctrl
                .class_methods
                .entry(Symbol::from(m))
                .or_insert(Ty::Untyped);
        }
        // Devise marks `current_<scope>` / `<scope>_signed_in?` as
        // `helper_method`, so templates see them too — register on
        // the view context (inserted into `classes` above).
        if let Some(view_cls) =
            classes.get_mut(&ClassId(Symbol::from("ActionView::Base")))
        {
            view_cls.instance_methods.insert(
                Symbol::from(format!("current_{scope}").as_str()),
                Ty::Union { variants: vec![model_ty, Ty::Nil] },
            );
            view_cls.instance_methods.insert(
                Symbol::from(format!("{scope}_signed_in?").as_str()),
                Ty::Bool,
            );
        }
    }
    classes.insert(ClassId(Symbol::from("ApplicationController")), app_ctrl);
}
