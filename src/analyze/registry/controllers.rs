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
    // `params` is an `ActionController::Parameters` — NOT a Hash. It
    // answers the Hash-like reads (`[]`, `fetch`, `key?`, `to_h`), the
    // strong-parameters chain (`require`/`permit`/`expect`, each
    // answering a Parameters again or the value under the key), and the
    // typed_parameters gem's `fetch_type`/`require_type`/`require_hash`/
    // `permit_types`. What a key holds is whatever the request carried
    // (String, Array, nested Parameters, nil), so element reads are
    // gradual; the chain methods keep the receiver's class.
    {
        let params_id = ClassId(Symbol::from("ActionController::Parameters"));
        let params_ty = Ty::Class { id: params_id.clone(), args: vec![] };
        let mut p = ClassInfo::default();
        let hash_str_untyped = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) };
        for m in [
            // strong parameters
            "permit", "permit!", "except", "slice", "merge", "merge!", "reverse_merge",
            "reverse_merge!", "with_defaults", "with_defaults!", "deep_dup", "dup", "clone",
            "compact", "compact_blank", "select", "reject", "filter", "keep_if", "delete_if",
            "transform_values", "transform_keys", "deep_transform_keys", "extract!", "update",
            "slice!", "except!",
            // typed_parameters: answers the Parameters restricted to the schema
            "permit_types", "require_hash",
        ] {
            p.instance_methods.insert(Symbol::from(m), params_ty.clone());
        }
        for m in [
            // typed_parameters answers the value under a key, typed by the schema
            // argument: gradual until that schema is modeled.
            "delete", "required", "fetch_type", "require_type", "required_type",
            "instance_variable_get",
        ] {
            p.instance_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        for m in [
            "key?", "has_key?", "include?", "member?", "empty?", "blank?", "present?",
            "permitted?", "nil?", "any?", "none?", "all?", "eql?", "==", "!", "!=",
            "value?", "has_value?", "frozen?", "respond_to?", "is_a?", "kind_of?",
            "instance_of?", "equal?",
        ] {
            p.instance_methods.insert(Symbol::from(m), Ty::Bool);
        }
        for m in ["to_h", "to_unsafe_h", "to_hash", "to_unsafe_hash", "to_hwia", "as_json"] {
            p.instance_methods.insert(Symbol::from(m), hash_str_untyped.clone());
        }
        p.instance_methods.insert(
            Symbol::from("keys"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        for m in ["to_query", "to_param", "to_s", "inspect", "to_json"] {
            p.instance_methods.insert(Symbol::from(m), Ty::Str);
        }
        for m in ["hash", "object_id"] {
            p.instance_methods.insert(Symbol::from(m), Ty::Int);
        }
        // What a key holds is whatever the request carried: a scalar
        // String, an Array (`ids[]=1&ids[]=2`, a JSON array), a nested
        // Parameters (`order[a]=1`), an uploaded file, or nothing. The element reads answer
        // that whole union (`param_value_ty`); union dispatch resolves a
        // send on whichever arms answer it, so `params[:id].to_i` still
        // reads as the String arm's Integer while `params[:ids].map` and
        // `params[:order].permit(...)` read as the Array and Parameters
        // arms. `expect(:id)` is the scalar form.
        p.instance_methods.insert(Symbol::from("[]"), param_value_ty(true));
        p.instance_methods.insert(Symbol::from("dig"), param_value_ty(true));
        // `require(:key)` answers the value under the key (raising when it
        // is blank), so it is the same union minus nil.
        p.instance_methods.insert(Symbol::from("require"), param_value_ty(false));
        for m in ["[]=", "expect"] {
            p.instance_methods.insert(Symbol::from(m), Ty::Str);
        }
        // The class-level configuration (`always_permitted_parameters`,
        // `permit_all_parameters`, `action_on_unpermitted_parameters`).
        p.class_methods.insert(Symbol::from("new"), params_ty.clone());
        p.class_methods.insert(
            Symbol::from("always_permitted_parameters"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        p.class_methods.insert(
            Symbol::from("always_permitted_parameters="),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        p.class_methods.insert(Symbol::from("permit_all_parameters"), Ty::Bool);
        p.class_methods.insert(Symbol::from("permit_all_parameters="), Ty::Bool);
        classes.insert(params_id, p);
        app_ctrl.class_methods.insert(Symbol::from("params"), params_ty);
    }
    // `session` is an `ActionDispatch::Request::Session`, hash-like but not a
    // Hash: it answers `id`, `options`, `loaded?`, `destroy`, and whatever an app
    // mixes onto it (core adds `essential`). Typing it `Hash[String, String]`
    // made every one of those a send_dispatch_failed.
    {
        let session_id = ClassId(Symbol::from("ActionDispatch::Request::Session"));
        let mut session = ClassInfo::default();
        // An unregistered parent: the rack session hash underneath is not
        // modeled, so a method this table does not list stays gradual instead
        // of failing dispatch.
        session.parent = Some(ClassId(Symbol::from("Rack::Session::Abstract::SessionHash")));
        // Reads keep the String-or-nil answer the Hash typing gave: the
        // blank-predicate grounding of `(rd = session[:k]).present?` needs a
        // nilable String receiver to lower.
        // Deleting a key returns the same value as reading it, or nil
        // when absent (e.g. `session.delete(:return_to) || root_url`).
        for m in ["[]", "delete"] {
            session.instance_methods.insert(
                Symbol::from(m),
                Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            );
        }
        for m in ["fetch", "dig"] {
            session.instance_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        for m in ["[]=", "store", "clear", "destroy", "update", "merge!", "each", "reload!"] {
            session.instance_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        for m in ["key?", "has_key?", "include?", "empty?", "loaded?", "exists?"] {
            session.instance_methods.insert(Symbol::from(m), Ty::Bool);
        }
        session.instance_methods.insert(Symbol::from("id"), Ty::Union { variants: vec![Ty::Str, Ty::Nil] });
        session.instance_methods.insert(Symbol::from("keys"), Ty::Array { elem: Box::new(Ty::Str) });
        session.instance_methods.insert(
            Symbol::from("options"),
            Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) },
        );
        for m in ["to_hash", "to_h"] {
            session.instance_methods.insert(
                Symbol::from(m),
                Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) },
            );
        }
        classes.insert(session_id.clone(), session);
        app_ctrl.class_methods.insert(Symbol::from("session"), Ty::Class { id: session_id, args: vec![] });
    }
    app_ctrl.class_methods.insert(Symbol::from("render"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("redirect_to"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("redirect_back_or_to"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("head"), Ty::Nil);
    // Rails' implicit forgery check — also a known filter target so
    // `filter_targets::framework_methods` keeps it (and analyzer
    // dispatch on a bare call resolves).
    app_ctrl.class_methods.insert(Symbol::from("verify_authenticity_token"), Ty::Nil);
    app_ctrl.class_methods.insert(Symbol::from("verified_request?"), Ty::Bool);
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
            // The cookie-jar salts (`Request::Cookies`), read where an app
            // builds its own MessageVerifier off the request.
            "signed_cookie_salt", "encrypted_cookie_salt",
            "encrypted_signed_cookie_salt", "authenticated_encrypted_cookie_salt",
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
        // Rack env accessors and the parameter/header views Rails exposes on every
        // request. `POST`/`GET` are the raw form/query hashes; the header
        // readers are Rack env lookups.
        for m in ["POST", "GET", "session_options"] {
            request.instance_methods.insert(
                Symbol::from(m),
                Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Untyped) },
            );
        }
        for m in ["get_header", "set_header", "delete_header", "cookies"] {
            request.instance_methods.insert(Symbol::from(m), Ty::Untyped);
        }
        request.instance_methods.insert(Symbol::from("original_fullpath"), Ty::Str);
        request.instance_methods.insert(Symbol::from("request_method_symbol"), Ty::Sym);
        request.instance_methods.insert(Symbol::from("route_uri_pattern"), str_or_nil());
        request.instance_methods.insert(Symbol::from("content_length"), Ty::Int);
        for m in ["headers", "env", "cookie_jar", "session", "params", "query_parameters",
                  "request_parameters", "path_parameters", "format", "body", "variant",
                  "flash", "subdomains", "accepts", "mime_type", "authorization",
                  // The app's key generator, and the controller CLASS the
                  // request routed to (`Request#controller_class`).
                  "key_generator", "controller_class"] {
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
    // HTTP Basic and Token auth (runtime/spinel/http_authentication.rb):
    // each yields the credentials the client sent and answers what the
    // block decided; the `or_request` forms render the 401 challenge
    // themselves when there were none or the block refused. Registered
    // with BOTH block parameters, because a block yielding two values
    // that names one leaves the second bound to nothing and everything
    // compared against it untyped.
    let yields_two = |first: (&str, Ty), second: (&str, Ty), verdict: Ty| Ty::Fn {
        params: Vec::new(),
        block: Some(Box::new(Ty::Fn {
            params: [first, second]
                .into_iter()
                .map(|(name, ty)| crate::ty::Param {
                    name: Symbol::from(name),
                    ty,
                    kind: crate::ty::ParamKind::Required,
                })
                .collect(),
            block: None,
            ret: Box::new(verdict),
            effects: crate::effect::EffectSet::default(),
        })),
        // The block's verdict, or nil once the challenge is rendered.
        ret: Box::new(Ty::Untyped),
        effects: crate::effect::EffectSet::default(),
    };
    let str_hash = || Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) };
    for m in ["authenticate_or_request_with_http_basic", "authenticate_with_http_basic"] {
        app_ctrl.class_methods.insert(
            Symbol::from(m),
            yields_two(("username", Ty::Str), ("password", Ty::Str), Ty::Bool),
        );
    }
    for m in ["authenticate_or_request_with_http_token", "authenticate_with_http_token"] {
        app_ctrl.class_methods.insert(
            Symbol::from(m),
            // A Token block usually answers the authenticated record.
            yields_two(("token", Ty::Str), ("options", str_hash()), Ty::Untyped),
        );
    }
    for m in ["request_http_basic_authentication", "request_http_token_authentication"] {
        app_ctrl.class_methods.insert(Symbol::from(m), Ty::Nil);
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
    // The surface belongs to `ActionController::Base`, which is where
    // Rails defines it and where the emitted runtime puts it — not to
    // `ApplicationController`, which is merely the class an app's own
    // controllers happen to inherit from. Registered there, with
    // `ApplicationController` inheriting it, so a controller that does
    // NOT descend from the app's own base still resolves: roundhouse's
    // synthesized redirect controller is exactly that, and its
    // `redirect_to` read as a call nothing knew.
    let acb_id = ClassId(Symbol::from("ActionController::Base"));
    let acb = classes.entry(acb_id.clone()).or_default();
    acb.instance_methods.extend(app_ctrl.instance_methods.drain());
    for (name, ty) in app_ctrl.class_methods.drain() {
        acb.class_methods.insert(name, ty);
    }
    acb.includes.extend(app_ctrl.includes.drain(..));
    // The entry stays — code and tests key on it, and an app's own
    // `ApplicationController` merges into it — but what it carries now
    // comes from its parent.
    let mut app_ctrl_entry = app_ctrl;
    app_ctrl_entry.parent = Some(acb_id);
    classes.insert(ClassId(Symbol::from("ApplicationController")), app_ctrl_entry);

    // `ActionDispatch::Router.escape_path`, the one router method generated
    // code calls: a routing redirect's `%{name}` re-escapes the decoded
    // capture with it (`synthesize_redirect_controller`). Only that one,
    // read from the runtime's signatures; the router's matching internals
    // are not Rails API an app could call. Lives next to the other
    // ActionDispatch types, not the ActionView registrar.
    {
        const RBS: &str = include_str!("../../../runtime/ruby/action_dispatch/router.rbs");
        if let Ok(parsed) = crate::rbs::parse_app_signatures(RBS) {
            let id = ClassId(Symbol::from("ActionDispatch::Router"));
            let name = Symbol::from("escape_path");
            if let Some(ty) = parsed.get(&id).and_then(|methods| methods.get(&name)) {
                classes.entry(id).or_default().class_methods.entry(name).or_insert_with(|| ty.clone());
            }
        }
    }
}

/// The type of a value read out of an `ActionController::Parameters`:
/// `String | Array[untyped] | ActionController::Parameters |
/// ActionDispatch::Http::UploadedFile` (a multipart file part), plus nil
/// when the key may be absent. Request data is untyped at the element
/// level (`Array[untyped]`): nothing in the request constrains it.
pub(crate) fn param_value_ty(nilable: bool) -> Ty {
    let mut variants = vec![
        Ty::Str,
        Ty::Array { elem: Box::new(Ty::Untyped) },
        Ty::Class { id: ClassId(Symbol::from("ActionController::Parameters")), args: vec![] },
        Ty::Class { id: ClassId(Symbol::from("ActionDispatch::Http::UploadedFile")), args: vec![] },
    ];
    if nilable {
        variants.push(Ty::Nil);
    }
    Ty::Union { variants }
}
