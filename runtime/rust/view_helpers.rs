//! Roundhouse Rust view-helpers runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/view_helpers.rs`). Mirrors
//! `runtime/typescript/view_helpers.ts` byte-for-byte where HTML
//! output matters — the compare tool asserts that the Rails
//! reference and every target produce the same DOM, so helper
//! output here must match what TS produces, which in turn matches
//! what Rails renders (modulo masked tokens + fingerprints).

#![allow(dead_code, unused_variables)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use base64::Engine;

use crate::runtime::ValidationError;

/// Layout-slot context threaded through views. Populated by
/// `content_for` and read by layouts.
#[derive(Debug, Default, Clone)]
pub struct RenderCtx {
    pub notice: Option<String>,
    pub alert: Option<String>,
    pub title: Option<String>,
}

// ── Per-request render state ────────────────────────────────────
//
// Rails' `yield` / `content_for` / `yield :slot` idiom expects the
// inner view (which sets slots + returns a body) to share state
// with the outer layout (which reads them). We use a thread-local
// here — axum handlers run concurrently on the tokio multi-thread
// runtime, so a global mutex would serialize requests; thread-
// locals scope per-worker and the server's middleware resets them
// on each request entry.

thread_local! {
    static YIELD_BODY: RefCell<String> = const { RefCell::new(String::new()) };
    static SLOTS: RefCell<BTreeMap<String, String>> = RefCell::new(BTreeMap::new());
}

/// Called by the server before dispatching each request — wipes
/// any stale slot values from a prior handler.
pub fn reset_render_state() {
    YIELD_BODY.with(|y| y.borrow_mut().clear());
    SLOTS.with(|s| s.borrow_mut().clear());
}

/// Stash the controller's rendered view body so the layout's
/// `<%= yield %>` can read it.
pub fn set_yield(body: &str) {
    YIELD_BODY.with(|y| *y.borrow_mut() = body.to_string());
}

pub fn get_yield() -> String {
    YIELD_BODY.with(|y| y.borrow().clone())
}

/// Read a named slot. Returns empty string when unset so string
/// concat in the emit doesn't produce "undefined" or panic.
pub fn get_slot(name: &str) -> String {
    SLOTS.with(|s| s.borrow().get(name).cloned().unwrap_or_default())
}

/// `content_for(:slot, "body")` — setter form. The 2-arg variant
/// appends into the named slot (Rails semantics); emit-time the
/// append vs overwrite behavior rarely matters for the scaffold,
/// but we mirror Rails to avoid a surprise. Returns empty so the
/// surrounding concat doesn't double-count the stashed value.
pub fn content_for_set(slot: &str, body: &str) {
    SLOTS.with(|s| {
        let mut map = s.borrow_mut();
        let prior = map.get(slot).cloned().unwrap_or_default();
        map.insert(slot.to_string(), prior + body);
    });
}

/// Getter form — used in layouts as `<%= content_for(:title) %>`
/// and `<%= content_for(:title) || "Default" %>`.
pub fn content_for_get(slot: &str) -> String {
    get_slot(slot)
}

// ── Rails layout helpers ────────────────────────────────────────

pub fn csrf_meta_tags() -> String {
    "<meta name=\"csrf-param\" content=\"authenticity_token\" />\n<meta name=\"csrf-token\" content=\"\" />".to_string()
}

/// Rails emits nothing for `csp_meta_tag` when no CSP policy is
/// configured; the scaffold has none, so the helper returns empty.
pub fn csp_meta_tag() -> String {
    String::new()
}

pub fn stylesheet_link_tag(name: &str, opts: &HashMap<String, String>) -> String {
    let mut attrs = String::new();
    for (k, v) in opts {
        attrs.push_str(&format!(" {}=\"{}\"", k, escape_html(v)));
    }
    format!(
        "<link rel=\"stylesheet\" href=\"/assets/{}.css\"{} />",
        escape_html(name),
        attrs,
    )
}

/// `<%= javascript_importmap_tags %>` — importmap JSON + module-
/// preload links + bootstrap `<script type="module">`. `pins` is
/// the ingested `config/importmap.rb` pin list, emitted per-app
/// into `src/importmap.rs`. `main_entry` defaults to
/// `"application"` matching Rails.
pub fn javascript_importmap_tags(pins: &[(&str, &str)], main_entry: &str) -> String {
    // Inline the JSON manually so key order matches the pin-list
    // order exactly (matters when the compare tool checks text
    // content of the importmap script). serde_json's BTreeMap
    // would sort alphabetically and diverge.
    let mut imports_json = String::from("{\n");
    for (i, (name, path)) in pins.iter().enumerate() {
        imports_json.push_str(&format!("    {name:?}: {path:?}"));
        if i + 1 < pins.len() {
            imports_json.push(',');
        }
        imports_json.push('\n');
    }
    imports_json.push_str("  }");
    let mut out = format!(
        "<script type=\"importmap\" data-turbo-track=\"reload\">{{\n  \"imports\": {imports_json}\n}}</script>"
    );
    for (_, path) in pins {
        out.push_str(&format!("\n<link rel=\"modulepreload\" href=\"{path}\">"));
    }
    out.push_str(&format!(
        "\n<script type=\"module\">import \"{}\"</script>",
        escape_html(main_entry),
    ));
    out
}

// ── Nav helpers ────────────────────────────────────────────────

pub fn link_to(text: &str, url: &str, opts: &HashMap<String, String>) -> String {
    let mut attrs = String::new();
    for (k, v) in opts {
        attrs.push_str(&format!(" {}=\"{}\"", k, escape_html(v)));
    }
    format!(
        "<a href=\"{}\"{}>{}</a>",
        escape_html(url),
        attrs,
        escape_html(text),
    )
}

pub fn link_to_simple(text: &str, url: &str) -> String {
    link_to(text, url, &HashMap::new())
}

/// Rails' `button_to`. Option keys mirror the TS runtime's:
///  - `method: "delete|patch|put"` → `_method` hidden input
///  - `class:` → on the `<button>`
///  - `form_class:` → on the `<form>` (defaults `"button_to"`)
///  - `data-*` flattened keys → button `data-*` attributes
pub fn button_to(text: &str, target: &str, opts: &HashMap<String, String>) -> String {
    let method = opts.get("method").map(|s| s.as_str()).unwrap_or("post");
    let button_cls = opts.get("class").map(|s| s.as_str()).unwrap_or("");
    let form_cls = opts
        .get("form_class")
        .map(|s| s.as_str())
        .unwrap_or("button_to");
    let method_lower = method.to_ascii_lowercase();
    let method_input = if method_lower != "post" && method_lower != "get" {
        format!(
            "<input type=\"hidden\" name=\"_method\" value=\"{}\" />",
            escape_html(method),
        )
    } else {
        String::new()
    };
    let mut data_attrs = String::new();
    for (k, v) in opts {
        if k.starts_with("data-") {
            data_attrs.push_str(&format!(" {}=\"{}\"", escape_html(k), escape_html(v)));
        }
    }
    let button_cls_attr = if button_cls.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", escape_html(button_cls))
    };
    let csrf_input = "<input type=\"hidden\" name=\"authenticity_token\" value=\"\">";
    format!(
        "<form class=\"{}\" method=\"post\" action=\"{}\">{}<button{}{} type=\"submit\">{}</button>{}</form>",
        escape_html(form_cls),
        escape_html(target),
        method_input,
        button_cls_attr,
        data_attrs,
        escape_html(text),
        csrf_input,
    )
}

/// Form-tag wrapper for `form_with`. Emits `<form
/// class=... action=... accept-charset="UTF-8" method="post">`
/// matching Rails' UTF8_ENFORCER_TAG-equipped output. CSRF token
/// is blank (the compare tool masks it); _method override for
/// PATCH is emitted when the record is persisted.
pub fn form_wrap(
    resource_path: &str,
    is_persisted: bool,
    html_class: &str,
    inner: &str,
) -> String {
    let method_input = if is_persisted {
        "<input type=\"hidden\" name=\"_method\" value=\"patch\">".to_string()
    } else {
        String::new()
    };
    let csrf_input = "<input type=\"hidden\" name=\"authenticity_token\" value=\"\">";
    let class_attr = if html_class.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", escape_html(html_class))
    };
    format!(
        "<form{} action=\"{}\" accept-charset=\"UTF-8\" method=\"post\">{}{}{}</form>",
        class_attr,
        escape_html(resource_path),
        method_input,
        csrf_input,
        inner,
    )
}

/// Humanize a snake_case field for a label.
fn humanize(field: &str) -> String {
    let spaced = field.replace('_', " ");
    let mut c = spaced.chars();
    match c.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
    }
}

/// FormBuilder for the scaffold shape.
pub struct FormBuilder {
    pub prefix: String,
    pub html_class: String,
    pub is_persisted: bool,
}

impl FormBuilder {
    pub fn new(prefix: &str, html_class: &str, is_persisted: bool) -> Self {
        Self {
            prefix: prefix.to_string(),
            html_class: html_class.to_string(),
            is_persisted,
        }
    }

    fn id_for(&self, field: &str) -> String {
        format!("{}_{}", self.prefix, field)
    }

    fn name_for(&self, field: &str) -> String {
        format!("{}[{}]", self.prefix, field)
    }

    pub fn label(&self, field: &str, opts: &HashMap<String, String>) -> String {
        let cls = opts
            .get("class")
            .map(|c| format!(" class=\"{}\"", escape_html(c)))
            .unwrap_or_default();
        format!(
            "<label for=\"{}\"{}>{}</label>",
            escape_html(&self.id_for(field)),
            cls,
            escape_html(&humanize(field)),
        )
    }

    /// Rails omits `value=""` on empty text fields. Match that.
    pub fn text_field(
        &self,
        field: &str,
        value: &str,
        opts: &HashMap<String, String>,
    ) -> String {
        let cls = opts
            .get("class")
            .map(|c| format!(" class=\"{}\"", escape_html(c)))
            .unwrap_or_default();
        let value_attr = if value.is_empty() {
            String::new()
        } else {
            format!(" value=\"{}\"", escape_html(value))
        };
        format!(
            "<input type=\"text\"{} name=\"{}\" id=\"{}\"{}>",
            cls,
            escape_html(&self.name_for(field)),
            escape_html(&self.id_for(field)),
            value_attr,
        )
    }

    /// Rails wraps textarea content in newlines even when empty —
    /// `<textarea>\n<value>\n</textarea>`. Part of the HTML5
    /// spec-compliant shape; required for byte-equal compare.
    pub fn textarea(
        &self,
        field: &str,
        value: &str,
        opts: &HashMap<String, String>,
    ) -> String {
        let cls = opts
            .get("class")
            .map(|c| format!(" class=\"{}\"", escape_html(c)))
            .unwrap_or_default();
        let rows = opts
            .get("rows")
            .map(|r| format!(" rows=\"{}\"", escape_html(r)))
            .unwrap_or_default();
        format!(
            "<textarea{}{} name=\"{}\" id=\"{}\">\n{}</textarea>",
            rows,
            cls,
            escape_html(&self.name_for(field)),
            escape_html(&self.id_for(field)),
            escape_html(value),
        )
    }

    pub fn submit(&self, opts: &HashMap<String, String>) -> String {
        let cls = opts
            .get("class")
            .map(|c| format!(" class=\"{}\"", escape_html(c)))
            .unwrap_or_default();
        // Rails capitalizes the resource name for scaffold-generated
        // submit buttons: "Create Article" / "Update Article".
        let human_prefix = {
            let mut c = self.prefix.chars();
            match c.next() {
                None => String::new(),
                Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
            }
        };
        let label = if let Some(lbl) = opts.get("label") {
            lbl.clone()
        } else if self.is_persisted {
            format!("Update {}", human_prefix)
        } else {
            format!("Create {}", human_prefix)
        };
        let esc = escape_html(&label);
        format!(
            "<input type=\"submit\" name=\"commit\" value=\"{}\"{} data-disable-with=\"{}\">",
            esc, cls, esc,
        )
    }
}

/// `<%= error_messages_for(record.errors, "article") %>`.
pub fn error_messages_for(errors: &[ValidationError], noun: &str) -> String {
    if errors.is_empty() {
        return String::new();
    }
    let count = errors.len();
    let plural = if count == 1 { "" } else { "s" };
    let mut items = String::new();
    for err in errors {
        items.push_str(&format!(
            "<li>{} {}</li>",
            escape_html(&humanize(&err.field)),
            escape_html(&err.message),
        ));
    }
    format!(
        "<div id=\"error_explanation\" class=\"bg-red-50 text-red-500 px-3 py-2 font-medium rounded-md mt-3\"><h2>{} error{} prohibited this {} from being saved:</h2><ul class=\"list-disc ml-6\">{}</ul></div>",
        count,
        plural,
        escape_html(noun),
        items,
    )
}

/// True when any ValidationError in `errors` targets the named
/// field. Feeds the scaffold's conditional form-field classes
/// (error-red vs ok-gray borders).
pub fn field_has_error(errors: &[ValidationError], field: &str) -> bool {
    errors.iter().any(|e| e.field == field)
}

pub fn turbo_stream_from(channel: &str) -> String {
    let json = format!("\"{}\"", channel.replace('\\', "\\\\").replace('"', "\\\""));
    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    format!(
        "<turbo-cable-stream-source channel=\"Turbo::StreamsChannel\" signed-stream-name=\"{}--unsigned\"></turbo-cable-stream-source>",
        encoded,
    )
}

/// `dom_id(record [, prefix])` — Rails convention:
///   one-arg  → `<singular>_<id>`               (article_1)
///   two-arg  → `<prefix>_<singular>_<id>`      (comments_count_article_1)
/// Takes the singular name explicitly; rust can't introspect the
/// record's type the way TS can via `constructor.name`. Emitters
/// produce the singular from the ingested model list.
pub fn dom_id(singular: &str, id: i64, prefix: Option<&str>) -> String {
    let id_str = if id == 0 {
        "new".to_string()
    } else {
        id.to_string()
    };
    let base = format!("{}_{}", singular, id_str);
    match prefix {
        Some(p) if !p.is_empty() => format!("{}_{}", p, base),
        _ => base,
    }
}

/// Naive pluralization — append `s` when count != 1.
pub fn pluralize(count: i64, word: &str) -> String {
    if count == 1 {
        format!("1 {}", word)
    } else {
        format!("{} {}s", count, word)
    }
}

/// `truncate(text, length: N, omission: "...")`. Rails default
/// length is 30, default omission is "...".
pub fn truncate(text: &str, opts: &HashMap<String, String>) -> String {
    let length: usize = opts
        .get("length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let omission = opts
        .get("omission")
        .map(|s| s.as_str())
        .unwrap_or("...");
    if text.chars().count() <= length {
        return text.to_string();
    }
    let cut = length.saturating_sub(omission.chars().count());
    let mut out: String = text.chars().take(cut).collect();
    out.push_str(omission);
    out
}

pub fn content_for(slot: &str, body: &str) -> String {
    content_for_set(slot, body);
    String::new()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_for_setter_roundtrips() {
        reset_render_state();
        content_for_set("title", "Articles");
        assert_eq!(get_slot("title"), "Articles");
        reset_render_state();
        assert_eq!(get_slot("title"), "");
    }

    #[test]
    fn submit_uses_capitalized_prefix() {
        let b = FormBuilder::new("article", "", false);
        let got = b.submit(&HashMap::new());
        assert!(got.contains("value=\"Create Article\""));
        assert!(got.contains("data-disable-with=\"Create Article\""));
        assert!(got.contains("name=\"commit\""));
    }

    #[test]
    fn text_field_omits_empty_value() {
        let b = FormBuilder::new("article", "", false);
        let got = b.text_field("title", "", &HashMap::new());
        assert!(!got.contains("value="));
    }

    #[test]
    fn textarea_wraps_value_in_newlines() {
        let b = FormBuilder::new("article", "", false);
        let got = b.textarea("body", "", &HashMap::new());
        assert!(got.contains(">\n</textarea>"));
    }

    #[test]
    fn form_wrap_emits_accept_charset() {
        let got = form_wrap("/articles", false, "contents", "");
        assert!(got.contains("accept-charset=\"UTF-8\""));
    }

    #[test]
    fn dom_id_prefix_form() {
        assert_eq!(dom_id("article", 3, None), "article_3");
        assert_eq!(
            dom_id("article", 3, Some("comments_count")),
            "comments_count_article_3"
        );
    }
}
