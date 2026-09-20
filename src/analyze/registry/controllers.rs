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
    // geared_pagination's controller concern — the gem's engine mixes it
    // into `ActionController::Base`, so the method is on every controller
    // of an app that carries the gem and is named nowhere in its source.
    // Gradual rather than Relation-typed: it answers the SAME relation it
    // was handed (the windowed receiver), and this registry maps a name to
    // one type with no way to say "argument 0's". Untyped is honest here —
    // no call site in the corpus consumes the return value, they all read
    // `@page` from the view. Implementation:
    // runtime/ruby/action_controller/pagination.rb.
    app_ctrl
        .class_methods
        .insert(Symbol::from("set_page_and_extract_portion_from"), Ty::Untyped);

    // The `Page` that method ASSIGNS. `runtime/ruby/action_controller/
    // pagination.rb` sets `@page = ActionController::Page.new(...)` and
    // the VIEW is the only reader (`@page.records`, `@page.last?`,
    // `@page.next_param`) — the gem has no `page` reader, which is why
    // the surface has to be registered as a class rather than reached
    // through one. Without it every such view read is `@page has no
    // known type`; campfire has six across four templates.
    //
    // The method list is `pagination.rbs` verbatim, so the two say the
    // same thing to the analyzer and to a strict target.
    {
        let relation_ty = Ty::Untyped;
        let mut page = ClassInfo::default();
        for (m, ty) in [
            ("number", Ty::Int),
            ("per_page", Ty::Int),
            ("records", relation_ty.clone()),
            ("records_count", Ty::Int),
            ("page_count", Ty::Int),
            ("first?", Ty::Bool),
            ("last?", Ty::Bool),
            ("only?", Ty::Bool),
            ("before_last?", Ty::Bool),
            ("next_param", Ty::Int),
        ] {
            page.instance_methods.insert(Symbol::from(m), ty);
        }
        classes.insert(ClassId(Symbol::from("ActionController::Page")), page);
    }
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
    // `request` is an ActionDispatch::Request with the surface an app
    // reads (the authentication generator's `request.user_agent` /
    // `request.remote_ip`, redirects' `request.url`, the verb
    // predicates the trace's guard verdict already reads by name).
    // Anything off this table falls to "no known method" — extend as
    // the corpus demands. `response` / `logger` stay gradual.
    {
        let request_id = ClassId(Symbol::from("ActionDispatch::Request"));
        let mut request = ClassInfo::default();
        let str_or_nil = || Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
        for m in [
            "user_agent", "remote_ip", "ip", "url", "original_url", "fullpath", "path",
            "host", "host_with_port", "domain", "protocol", "scheme", "port_string",
            "request_method", "method", "raw_post", "uuid", "request_id", "base_url",
            "script_name", "path_info", "query_string", "media_type",
        ] {
            request.instance_methods.insert(Symbol::from(m), Ty::Str);
        }
        for m in [
            "referer", "referrer", "content_type", "origin", "subdomain", "remote_addr",
            // The CSP nonce a layout interpolates into a script tag —
            // `get_header(NONCE)`, so nil until a policy sets one.
            // Typing `request` on the view context is what surfaced
            // its absence: a template that reads it turned from an
            // untyped call into a dispatch failure, which is the
            // trade typing anything makes.
            "content_security_policy_nonce",
        ] {
            request.instance_methods.insert(Symbol::from(m), str_or_nil());
        }
        for m in [
            "get?", "post?", "put?", "patch?", "delete?", "head?", "options?", "xhr?",
            "xml_http_request?", "ssl?", "local?", "form_data?",
        ] {
            request.instance_methods.insert(Symbol::from(m), Ty::Bool);
        }
        request.instance_methods.insert(Symbol::from("port"), Ty::Int);
        request.instance_methods.insert(Symbol::from("content_length"), Ty::Int);
        for m in ["headers", "env", "cookie_jar", "session", "params", "query_parameters",
                  "request_parameters", "path_parameters", "format", "body", "variant",
                  "flash", "subdomains", "accepts", "mime_type", "authorization"] {
            request.instance_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        classes.insert(request_id.clone(), request);
        let request_ty = Ty::Class { id: request_id, args: vec![] };
        // A template reads `request` too — Rails' view context
        // delegates it, and a layout reaching for
        // `request.content_security_policy_nonce` is the shape that
        // found this. Registered on both contexts for the same reason
        // `flash` is (see `registry::view`): the controller has it
        // class-side, the view instance-side, and it is one object.
        if let Some(view_cls) = classes.get_mut(&ClassId(Symbol::from("ActionView::Base")))
        {
            view_cls
                .instance_methods
                .insert(Symbol::from("request"), request_ty.clone());
        }
        app_ctrl.class_methods.insert(Symbol::from("request"), request_ty);
    }
    app_ctrl.class_methods.insert(Symbol::from("response"), Ty::Untyped);
    app_ctrl.class_methods.insert(Symbol::from("logger"), Ty::Untyped);
    // `cookies` is the cookie jar: string values in and out,
    // `signed`/`permanent`/`encrypted` are the same jar with a codec
    // (so `cookies.signed.permanent[:session_id] = …` chains), and
    // `delete` answers the removed value.
    {
        let jar_id = ClassId(Symbol::from("ActionDispatch::Cookies::CookieJar"));
        let jar_ty = Ty::Class { id: jar_id.clone(), args: vec![] };
        let mut jar = ClassInfo::default();
        let str_or_nil = || Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
        for m in ["[]", "delete", "fetch"] {
            jar.instance_methods.insert(Symbol::from(m), str_or_nil());
        }
        // `[]=` takes a String or an options Hash; answers what it was
        // given, which callers never read.
        jar.instance_methods.insert(Symbol::from("[]="), Ty::Nil);
        for m in ["signed", "permanent", "encrypted", "signed_or_encrypted"] {
            jar.instance_methods.insert(Symbol::from(m), jar_ty.clone());
        }
        for m in ["key?", "has_key?", "include?"] {
            jar.instance_methods.insert(Symbol::from(m), Ty::Bool);
        }
        jar.instance_methods.insert(Symbol::from("to_h"), Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) });
        jar.instance_methods.insert(Symbol::from("clear"), Ty::Nil);
        classes.insert(jar_id, jar);
        app_ctrl.class_methods.insert(Symbol::from("cookies"), jar_ty);
    }
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
