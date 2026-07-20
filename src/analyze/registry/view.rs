//! ActionView view-context surface: the `FormBuilder`, the mime-responds
//! `Collector`, the `ActionView::Base` flat-helper accumulator (links, tags,
//! flash accessors, jbuilder `json`, route helpers, kaminari, simple_form,
//! and the app/helpers fold), and the `ActionDispatch::Flash::FlashHash`
//! class. Extracted verbatim from `Analyzer::with_adapter`.

use std::collections::{BTreeSet, HashMap};

use crate::analyze::ClassInfo;
use crate::App;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(
    classes: &mut HashMap<ClassId, ClassInfo>,
    app: &App,
    route_helper_names: &[String],
) {
    // ActionView form builder — `form_with do |form| form.text_field
    // ... end`. `form_with` yields a FormBuilder whose field helpers
    // render to strings (`ActiveSupport::SafeBuffer`, modeled as Str).
    // Registered so both the block param AND the per-field calls type:
    // once `form` is a FormBuilder, an unregistered `form.x` would be a
    // dispatch *error*, so the field surface is covered here.
    let form_builder_id = ClassId(Symbol::from("ActionView::Helpers::FormBuilder"));
    let form_builder_ty = Ty::Class { id: form_builder_id.clone(), args: vec![] };
    let mut form_builder = ClassInfo::default();
    for m in [
        "label", "submit", "button", "text_field", "text_area", "textarea",
        "hidden_field", "password_field", "email_field", "number_field",
        "url_field", "tel_field", "telephone_field", "phone_field",
        "search_field", "color_field", "range_field", "date_field",
        "time_field", "datetime_field", "datetime_local_field", "month_field",
        "week_field", "file_field", "check_box", "radio_button", "select",
        "collection_select", "grouped_collection_select", "time_zone_select",
        "collection_check_boxes", "collection_radio_buttons", "date_select",
        "time_select", "datetime_select", "rich_text_area", "weekday_select",
        "id", "to_s",
    ] {
        form_builder.instance_methods.insert(Symbol::from(m), Ty::Str);
    }
    // `form.object` is the form's model (unknown model → gradual);
    // nested `fields_for`/`fields` yield another builder.
    form_builder.instance_methods.insert(Symbol::from("object"), Ty::Untyped);
    for m in ["fields_for", "fields"] {
        form_builder
            .instance_methods
            .insert(Symbol::from(m), super::block_fn(&form_builder_ty, Ty::Str));
    }
    classes.insert(form_builder_id, form_builder);

    // `respond_to do |format| format.html { } format.json { } end` —
    // the block yields a mime Collector whose format methods return
    // nil. (`respond_to` itself is registered on ApplicationController.)
    let mut collector = ClassInfo::default();
    for m in [
        "html", "json", "xml", "js", "rss", "atom", "text", "csv", "any",
        "all", "none",
    ] {
        collector.instance_methods.insert(Symbol::from(m), Ty::Nil);
    }
    classes.insert(
        ClassId(Symbol::from("ActionController::MimeResponds::Collector")),
        collector,
    );

    // View context — the `self` a view body types against. `form_with`
    // lives here (flat view helpers — `link_to`/`render`/… — will join
    // it); the view loops set this as `self_ty` so implicit-self helper
    // calls dispatch against it.
    // Route URL helper names from the ingested route table — one
    // `<as_name>_path` / `<as_name>_url` per named route (same flattening
    // the route emitters use). Registered on the view context,
    // ApplicationController, and library classes below. See
    // `registry::routes`.

    let mut action_view = ClassInfo::default();
    action_view
        .instance_methods
        .insert(Symbol::from("form_with"), super::block_fn(&form_builder_ty, Ty::Str));
    // Flat view helpers — links, tags, asset/meta tags, text and number
    // formatting, dom ids, render, turbo helpers. All render to strings
    // (`ActiveSupport::SafeBuffer`, modeled as Str), so the implicit-self
    // call types and any `.html_safe`/`.gsub`/etc. chained on the result
    // resolves through `str_method`. (Route helpers `*_path`/`*_url`,
    // flash `notice`/`alert`, and jbuilder `json` are registered
    // elsewhere.)
    for helper in [
        // links / urls
        "link_to", "button_to", "link_to_if", "link_to_unless",
        "link_to_unless_current", "mail_to", "url_for",
        // tags / assets / meta
        "content_tag", "image_tag", "image_url", "image_path",
        "video_tag", "audio_tag", "asset_path", "asset_url",
        "favicon_link_tag", "stylesheet_link_tag", "stylesheet_path",
        "javascript_include_tag", "javascript_path",
        "javascript_importmap_tags", "javascript_tag",
        "stylesheet_pack_tag", "javascript_pack_tag", "csrf_meta_tags",
        "csrf_meta_tag", "csp_meta_tag", "auto_discovery_link_tag",
        "preload_link_tag", "action_cable_meta_tag",
        "content_security_policy_nonce",
        // text / number formatting
        "pluralize", "truncate", "simple_format", "highlight", "excerpt",
        "word_wrap", "sanitize", "sanitize_css", "strip_tags",
        "strip_links", "raw", "h", "html_escape", "concat", "safe_join",
        "cycle", "current_cycle", "number_to_currency", "number_to_human",
        "number_to_human_size", "number_to_percentage", "number_to_phone",
        "number_with_delimiter", "number_with_precision",
        // dates
        "time_ago_in_words", "distance_of_time_in_words",
        "distance_of_time_in_words_to_now",
        // i18n — the view-side translate/localize helpers (delegate
        // to I18n; lazy-lookup `t(".key")` included). Str like the
        // rest of the SafeBuffer-rendering surface.
        "t", "translate", "l", "localize",
        // Our own HAML lowering's dynamic-attribute helper
        // (`%div{opengraph_tags}` → `render_attrs(…)`, see
        // src/haml.rs) — renders an attribute string.
        "render_attrs",
        // dom / rendering / capture
        "dom_id", "dom_class", "render", "render_to_string", "capture",
        "content_for", "provide", "escape_javascript", "j",
        // turbo / hotwire
        "turbo_frame_tag", "turbo_stream_from", "turbo_refreshes_with",
        "turbo_include_tags", "turbo_page_requires_reload",
        // form option builders + FormTagHelper (all render to SafeBuffer
        // strings, like the tag helpers above).
        "options_for_select", "options_from_collection_for_select",
        "option_groups_from_collection_for_select", "grouped_options_for_select",
        "time_zone_options_for_select", "collection_select",
        "form_tag", "label_tag", "text_field_tag", "password_field_tag",
        "hidden_field_tag", "text_area_tag", "check_box_tag",
        "radio_button_tag", "select_tag", "submit_tag", "button_tag",
        "field_set_tag", "file_field_tag", "email_field_tag",
        "number_field_tag", "search_field_tag", "telephone_field_tag",
        "url_field_tag", "date_field_tag", "color_field_tag",
        "fields_for", "token_list", "class_names",
        // controller/request context exposed to views (and controllers,
        // registered there too) — both return the current name as Str.
        "action_name", "controller_name", "controller_path",
    ] {
        action_view
            .instance_methods
            .entry(Symbol::from(helper))
            .or_insert(Ty::Str);
    }
    // `tag` is the dynamic TagBuilder — `tag.div`/`tag.details` build an
    // element from the *method name*, so it can't be a fixed Str return
    // (that turns `tag.foo` into a dispatch error). Untyped (gradual):
    // both `tag("br")` and `tag.section` flow through without erroring.
    action_view.instance_methods.insert(Symbol::from("tag"), Ty::Untyped);
    // Flash convenience accessors — Rails 7 scaffolds emit bare
    // `notice`/`alert` in views; both read `flash[:notice]`/`[:alert]`.
    // Typed Str (not Str|Nil): consistent with the other Str-returning
    // helpers, and a nilable here trips Crystal's strict nil-concat
    // narrowing in `<%= notice %>`. `.present?` still resolves on Str.
    for m in ["notice", "alert"] {
        action_view.instance_methods.insert(Symbol::from(m), Ty::Str);
    }
    // `flash` — the FlashHash. Bare `flash` was unmodeled, so
    // `flash[:error]` / `flash.now[:error]` / `flash.each` / `flash.keep`
    // (pervasive in controllers and views) all bottomed out at Var. Type
    // it as a FlashHash whose surface is registered below; both the view
    // (instance) and controller (class-side) contexts get it.
    let flash_ty = Ty::Class {
        id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
        args: vec![],
    };
    action_view
        .instance_methods
        .insert(Symbol::from("flash"), flash_ty.clone());
    // jbuilder `json` builder (in `*.json.jbuilder` views) is dynamic —
    // `json.<field>`/`json.array!`/`json.partial!` build from the method
    // name, so Untyped (gradual) is the honest type and chains through
    // it without erroring.
    action_view.instance_methods.insert(Symbol::from("json"), Ty::Untyped);
    // Route URL helpers (view side).
    for name in route_helper_names {
        action_view
            .instance_methods
            .entry(Symbol::from(name.as_str()))
            .or_insert(Ty::Str);
    }
    // Kaminari's view-side paginator renders to a SafeBuffer string,
    // like the tag helpers above.
    action_view
        .instance_methods
        .entry(Symbol::from("paginate"))
        .or_insert(Ty::Str);
    // `params` is exposed to templates too (same strong-params
    // surface the controller context declares).
    action_view.instance_methods.insert(
        Symbol::from("params"),
        Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) },
    );
    // SimpleForm's form builder — same shape as `form_with` but the
    // yielded builder (`f.input`, `f.association`, …) is a SimpleForm
    // class we don't model structurally, so the block param is the
    // gradual escape: `f.input :name` flows through instead of
    // bottoming out unresolved.
    for m in ["simple_form_for", "simple_fields_for"] {
        action_view
            .instance_methods
            .entry(Symbol::from(m))
            .or_insert_with(|| super::block_fn(&Ty::Untyped, Ty::Str));
    }
    // Helper-fold: Rails mixes EVERY module under app/helpers into
    // every view (`helpers :all` default). Declaring them as
    // `include`s of the view context lets `fold_concern_surfaces`
    // copy each helper's typed surface onto `ActionView::Base` at
    // every harvest round — so a bare `material_symbol(…)` in a
    // template resolves exactly like a concern method on a model,
    // refining as the fixpoint types helper bodies. Hardcoded
    // framework entries above win over a same-named app helper
    // (own-entry-wins in the fold); acceptable, both are Str-shaped
    // in practice.
    let helper_modules: BTreeSet<ClassId> =
        app.helper_method_index.values().cloned().collect();
    action_view.includes.extend(helper_modules);
    classes.insert(ClassId(Symbol::from("ActionView::Base")), action_view);

    // The FlashHash returned by `flash`. Values are messages (Str); `now`
    // is the same hash scoped to this request (so `flash.now[:x]` types);
    // `notice`/`alert`/`error`/`success` are the convenience readers Rails
    // generates; `keep`/`discard`/`each` return the hash for chaining;
    // predicates and `[]` round out the surface. Lookups not listed fall
    // through to "no known method" — extend as the corpus demands.
    {
        let mut flash = ClassInfo::default();
        let flash_self = Ty::Class {
            id: ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
            args: vec![],
        };
        for (m, ty) in [
            ("[]", Ty::Str),
            ("[]=", Ty::Nil),
            ("store", Ty::Nil),
            ("now", flash_self.clone()),
            ("notice", Ty::Str),
            ("alert", Ty::Str),
            ("error", Ty::Str),
            ("success", Ty::Str),
            ("notice=", Ty::Str),
            ("alert=", Ty::Str),
            ("delete", Ty::Str),
            ("keep", flash_self.clone()),
            ("discard", flash_self.clone()),
            ("each", flash_self.clone()),
            ("each_pair", flash_self.clone()),
            ("clear", flash_self.clone()),
            ("update", flash_self.clone()),
            ("merge!", flash_self.clone()),
            ("key?", Ty::Bool),
            ("has_key?", Ty::Bool),
            ("include?", Ty::Bool),
            ("any?", Ty::Bool),
            ("empty?", Ty::Bool),
            ("present?", Ty::Bool),
            ("blank?", Ty::Bool),
            ("keys", Ty::Array { elem: Box::new(Ty::Sym) }),
            ("values", Ty::Array { elem: Box::new(Ty::Str) }),
            ("to_h", Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }),
            ("to_hash", Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) }),
        ] {
            flash.instance_methods.insert(Symbol::from(m), ty);
        }
        classes.insert(
            ClassId(Symbol::from("ActionDispatch::Flash::FlashHash")),
            flash,
        );
    }
}
