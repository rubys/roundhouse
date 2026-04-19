//! Roundhouse Rust view-helpers runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/view_helpers.rs`). Provides the Rails-
//! compatible view helpers emitted view fns call into: `link_to`,
//! `button_to`, `form_wrap`, the `FormBuilder` methods, `turbo_
//! stream_from`, `dom_id`, `pluralize`, plus a `RenderCtx` carrying
//! layout slots (notice / alert / title).
//!
//! Implementations are deliberately minimal — each returns HTML
//! that's close enough to Rails' output for the scaffold blog's
//! tests to pass, without pretending to be a full Rails port. A
//! later phase can swap in faithful output (escaping, full option
//! support, ARIA, etc.); the function signatures are the stable
//! contract.

#![allow(dead_code, unused_variables)]

use std::collections::HashMap;

/// Layout-slot context threaded through views. Populated by
/// `content_for` and read by layouts. Phase 4d's emitted views
/// don't actually consult layouts, so these fields are write-only
/// placeholders.
#[derive(Debug, Default, Clone)]
pub struct RenderCtx {
    pub notice: Option<String>,
    pub alert: Option<String>,
    pub title: Option<String>,
}

/// Flash accessor. Rails' view code reads `notice.present?` — this
/// wrapper lets that compile as `notice().present()` at emit time
/// (if we go that route) or simply return None here. Phase 4d views
/// call `notice.present?` as a field access on the RenderCtx, so
/// this free function is provided for parallel cases.
pub fn notice() -> Option<String> {
    None
}

/// `<a href="url" class="...">text</a>`. Options hash is flattened
/// as attributes. Rails also supports methods like `method: :delete`
/// which turn links into button-wrapped forms — we ignore those in
/// this stub; `button_to` exists for that case.
pub fn link_to(text: String, url: String, opts: HashMap<String, String>) -> String {
    let mut attrs = String::new();
    for (k, v) in &opts {
        attrs.push_str(&format!(" {}=\"{}\"", k, escape_html(v)));
    }
    format!("<a href=\"{}\"{}>{}</a>", escape_html(&url), attrs, escape_html(&text))
}

/// Two-arg variant used when Ruby code passes `link_to(text, url)`
/// without options.
pub fn link_to_simple(text: String, url: String) -> String {
    link_to(text, url, HashMap::new())
}

/// `<form method="post" action="...">...<button>text</button></form>`.
/// Rails' button_to wraps a one-button form with a hidden `_method`
/// input for non-POST verbs. `target` can be a URL string or a model
/// (dispatched to a path helper at emit time — we accept a URL
/// string here to keep the runtime simple). Options: `method:
/// :delete`, `class:`, `data:` (hash).
pub fn button_to(text: String, target: String, opts: HashMap<String, String>) -> String {
    let method = opts.get("method").cloned().unwrap_or_else(|| "post".to_string());
    let class = opts.get("class").cloned().unwrap_or_default();
    let method_input = if method.to_lowercase() != "post" && method.to_lowercase() != "get" {
        format!("<input type=\"hidden\" name=\"_method\" value=\"{}\"/>", method)
    } else {
        String::new()
    };
    format!(
        "<form method=\"post\" action=\"{}\" class=\"{}\">{}<button>{}</button></form>",
        escape_html(&target),
        escape_html(&class),
        method_input,
        escape_html(&text),
    )
}

/// Form-tag wrapper. Called by the emitter after rendering a
/// `form_with` block's inner buffer. `action` is the URL the form
/// submits to (None for unbound forms); `class` is the html class
/// attribute; `inner` is the pre-rendered block contents.
pub fn form_wrap(action: Option<&dyn HasFormAction>, class: &str, inner: &str) -> String {
    let action_attr = action
        .map(|a| format!(" action=\"{}\"", escape_html(&a.form_action())))
        .unwrap_or_default();
    format!(
        "<form method=\"post\"{} class=\"{}\">{}</form>",
        action_attr,
        escape_html(class),
        inner,
    )
}

/// Models implement this so `form_with(model: @article)` can resolve
/// the submit URL. Phase 4d default: empty string (the emitter
/// uses `None` for the action arg when the model's path isn't
/// derivable; a smarter emit can add `impl HasFormAction` per
/// model).
pub trait HasFormAction {
    fn form_action(&self) -> String {
        String::new()
    }
}

/// Blanket impl so `&T` / `()` / refs to model structs all work as
/// `Option<&dyn HasFormAction>` call arguments.
impl HasFormAction for () {}

/// Stub Rails FormBuilder. One instance per form_with block. Phase
/// 4d emits one input per `form.label` / `form.text_field` call;
/// options (`class:`, `rows:`, etc.) are ignored.
pub struct FormBuilder<'a> {
    record: &'a dyn HasFormAction,
    html_class: String,
}

impl<'a> FormBuilder<'a> {
    pub fn new(record: &'a dyn HasFormAction, html_class: &str) -> Self {
        Self { record, html_class: html_class.to_string() }
    }

    pub fn label(&self, field: &str) -> String {
        format!("<label for=\"{}\">{}</label>", escape_html(field), escape_html(field))
    }

    pub fn text_field(&self, field: &str) -> String {
        format!("<input type=\"text\" name=\"{}\"/>", escape_html(field))
    }

    pub fn textarea(&self, field: &str) -> String {
        format!("<textarea name=\"{}\"></textarea>", escape_html(field))
    }

    pub fn submit(&self) -> String {
        "<input type=\"submit\" value=\"Submit\"/>".to_string()
    }
}

/// `<turbo-cable-stream-source>` tag. Phase 4d stub returns a
/// pseudo-tag string so it's visible in the rendered output without
/// needing a live websocket runtime.
pub fn turbo_stream_from(channel: String) -> String {
    format!("<turbo-cable-stream-source channel=\"{}\"/>", escape_html(&channel))
}

/// `dom_id(record)` → `"<singular>_<id>"`. Rails uses the model's
/// model_name + id. We approximate with the record's `.id` field —
/// the `impl HasDomId` trait lets each model override.
pub fn dom_id(record: &dyn HasDomId) -> String {
    record.dom_id()
}

pub trait HasDomId {
    fn dom_id(&self) -> String;
}

impl HasDomId for () {
    fn dom_id(&self) -> String {
        String::new()
    }
}

/// `pluralize(count, word)` → `"1 article"` / `"2 articles"`. Naive
/// pluralization — Rails has an inflector; we just append `s` when
/// count != 1.
pub fn pluralize(count: i64, word: String) -> String {
    if count == 1 {
        format!("1 {}", word)
    } else {
        format!("{} {}s", count, word)
    }
}

/// `content_for(:slot, body)` signature. Phase 4d stores into a
/// thread-local RenderCtx; layouts would consume. Not wired to
/// layouts yet — the return is an empty string so `_buf` doesn't
/// accumulate anything.
pub fn content_for(slot: String, body: String) -> String {
    let _ = slot;
    let _ = body;
    String::new()
}

/// Conservative HTML escaping — enough for the scaffold blog. A
/// full escape would also handle whitespace in attribute values and
/// null bytes, but those don't appear in the scaffold's output.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
