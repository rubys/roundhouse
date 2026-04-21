//! Roundhouse Rust view-helpers runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/view_helpers.rs`). Provides the Rails-
//! compatible view helpers emitted view fns call into: `link_to`,
//! `button_to`, `form_wrap`, the `FormBuilder` methods, `turbo_
//! stream_from`, `dom_id`, `pluralize`, plus a `RenderCtx` carrying
//! layout slots (notice / alert / title).
//!
//! Mirrors `runtime/typescript/view_helpers.ts` in intent + method
//! signatures. Implementations are deliberately minimal — enough
//! HTML for the scaffold blog's acceptance to pass, without
//! pretending to be a full Rails port.
//!
//! FormBuilder's per-field methods take the current value as an
//! explicit `&str` arg rather than reading off a trait-object
//! record. Emit-side knows the record + field and can produce the
//! direct field access — keeps the runtime free of dynamic-field
//! dispatch, which rust would otherwise need a trait + derive to
//! provide.

#![allow(dead_code, unused_variables)]

use std::collections::HashMap;

use base64::Engine;

use crate::runtime::ValidationError;

/// Layout-slot context threaded through views. Populated by
/// `content_for` and read by layouts. The emitted layout doesn't
/// consult these yet, so fields are write-only placeholders.
#[derive(Debug, Default, Clone)]
pub struct RenderCtx {
    pub notice: Option<String>,
    pub alert: Option<String>,
    pub title: Option<String>,
}

/// Flash accessor. Emitted views read `notice.present?`; this free
/// function stays so emit-time lowering has a target name — returns
/// None at runtime since the production server doesn't thread
/// session flash through yet.
pub fn notice() -> Option<String> {
    None
}

/// `<a href="url" class="...">text</a>`. Options flatten as
/// attributes. Rails also supports `method: :delete` on link_to,
/// which morphs into a button-wrapped form — use `button_to` for
/// that case.
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

/// Two-arg variant — for `link_to(text, url)` without options.
pub fn link_to_simple(text: &str, url: &str) -> String {
    link_to(text, url, &HashMap::new())
}

/// `<form method="post" action="..."><button>text</button></form>`
/// with hidden `_method` for non-POST verbs. The server's method-
/// override middleware reads the `_method` form field and rewrites
/// the request method before routing.
pub fn button_to(text: &str, target: &str, opts: &HashMap<String, String>) -> String {
    let method = opts
        .get("method")
        .map(|s| s.as_str())
        .unwrap_or("post");
    let cls = opts.get("class").map(|s| s.as_str()).unwrap_or("");
    let method_lower = method.to_ascii_lowercase();
    let method_input = if method_lower != "post" && method_lower != "get" {
        format!(
            "<input type=\"hidden\" name=\"_method\" value=\"{}\"/>",
            escape_html(method),
        )
    } else {
        String::new()
    };
    format!(
        "<form method=\"post\" action=\"{}\" class=\"{}\">{}<button>{}</button></form>",
        escape_html(target),
        escape_html(cls),
        method_input,
        escape_html(text),
    )
}

/// Form-tag wrapper. `resource_path` is the URL the form submits
/// to (new records POST to the collection URL; persisted records
/// PATCH to the member URL via `_method` override). `is_persisted`
/// drives the method-override decision. CSRF is stubbed with an
/// empty token — the server doesn't verify today, but we emit the
/// field so Turbo/Rails conventions find it.
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
        "<form method=\"post\" action=\"{}\"{}>{}{}{}</form>",
        escape_html(resource_path),
        class_attr,
        method_input,
        csrf_input,
        inner,
    )
}

/// Humanize a snake_case field for a label: `"first_name"` →
/// `"First name"`. Rails' default `label` helper does this.
fn humanize(field: &str) -> String {
    let spaced = field.replace('_', " ");
    let mut c = spaced.chars();
    match c.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
    }
}

/// FormBuilder for the scaffold shape. `prefix` is the Rails
/// `name` prefix (`"article"` → inputs get `name="article[title]"`).
/// `is_persisted` drives `submit`'s label (Create vs Update).
/// Field values are passed at each call site — emit knows which
/// record + field; no dynamic dispatch needed.
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
        format!(
            "<input type=\"text\" name=\"{}\" id=\"{}\" value=\"{}\"{}>",
            escape_html(&self.name_for(field)),
            escape_html(&self.id_for(field)),
            escape_html(value),
            cls,
        )
    }

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
            "<textarea name=\"{}\" id=\"{}\"{}{}>{}</textarea>",
            escape_html(&self.name_for(field)),
            escape_html(&self.id_for(field)),
            cls,
            rows,
            escape_html(value),
        )
    }

    pub fn submit(&self, opts: &HashMap<String, String>) -> String {
        let cls = opts
            .get("class")
            .map(|c| format!(" class=\"{}\"", escape_html(c)))
            .unwrap_or_default();
        let label = if let Some(lbl) = opts.get("label") {
            lbl.clone()
        } else if self.is_persisted {
            format!("Update {}", self.prefix)
        } else {
            format!("Create {}", self.prefix)
        };
        format!(
            "<input type=\"submit\" value=\"{}\"{}>",
            escape_html(&label),
            cls,
        )
    }
}

/// `<%= error_messages_for(record, "article") %>` — Rails
/// scaffold error block if there are validation errors, empty
/// string otherwise. Emitter feeds the record's `.validate()`
/// output (a `Vec<ValidationError>`) — cheap revalidation keeps
/// the model struct free of a persistent error-state field.
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

/// `<%= turbo_stream_from "channel" %>` — emits the Turbo cable
/// tag that triggers the client to subscribe. The signed-stream-
/// name is base64-of-JSON-string; Roundhouse doesn't verify the
/// HMAC signature today (appending `--unsigned` matches what the
/// TS runtime does). The server's cable handler decodes the base64
/// to recover the channel name for broadcast routing.
pub fn turbo_stream_from(channel: &str) -> String {
    let json = format!("\"{}\"", channel.replace('\\', "\\\\").replace('"', "\\\""));
    let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    format!(
        "<turbo-cable-stream-source channel=\"Turbo::StreamsChannel\" signed-stream-name=\"{}--unsigned\"></turbo-cable-stream-source>",
        encoded,
    )
}

/// `dom_id(record)` → `"<singular>_<id>"`. Rust's generated view
/// code calls this with `(name, id)` — the naming + id come from
/// the emitter's knowledge of the record's type and field.
pub fn dom_id(name: &str, id: i64) -> String {
    if id == 0 {
        String::new()
    } else {
        format!("{}_{}", name, id)
    }
}

/// Naive pluralization — appends `s` when count != 1.
pub fn pluralize(count: i64, word: &str) -> String {
    if count == 1 {
        format!("1 {}", word)
    } else {
        format!("{} {}s", count, word)
    }
}

/// `content_for(:slot, body)` stashes into RenderCtx. Not wired to
/// layouts yet — returns empty so `_buf` doesn't accumulate the
/// stored value twice.
pub fn content_for(slot: &str, body: &str) -> String {
    let _ = slot;
    let _ = body;
    String::new()
}

/// Conservative HTML escaping — enough for scaffold blog output.
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
    fn label_humanizes_and_escapes() {
        let b = FormBuilder::new("article", "", false);
        let got = b.label("first_name", &HashMap::new());
        assert!(got.contains("for=\"article_first_name\""));
        assert!(got.contains(">First name</label>"));
    }

    #[test]
    fn submit_uses_update_when_persisted() {
        let b = FormBuilder::new("article", "", true);
        let got = b.submit(&HashMap::new());
        assert!(got.contains("value=\"Update article\""));
    }

    #[test]
    fn submit_uses_create_when_new() {
        let b = FormBuilder::new("article", "", false);
        let got = b.submit(&HashMap::new());
        assert!(got.contains("value=\"Create article\""));
    }

    #[test]
    fn error_messages_renders_block_when_non_empty() {
        let errs = vec![
            ValidationError::new("title", "can't be blank"),
            ValidationError::new("body", "is too short"),
        ];
        let got = error_messages_for(&errs, "article");
        assert!(got.contains("2 errors prohibited this article"));
        assert!(got.contains("Title can&#39;t be blank"));
        assert!(got.contains("Body is too short"));
    }

    #[test]
    fn error_messages_empty_when_no_errors() {
        assert_eq!(error_messages_for(&[], "article"), "");
    }

    #[test]
    fn turbo_stream_from_base64s_channel() {
        let got = turbo_stream_from("articles");
        // base64 of `"articles"` (with quotes) is `ImFydGljbGVzIg==`
        assert!(got.contains("signed-stream-name=\"ImFydGljbGVzIg==--unsigned\""));
    }

    #[test]
    fn form_wrap_emits_method_override_when_persisted() {
        let got = form_wrap("/articles/1", true, "contents", "");
        assert!(got.contains("value=\"patch\""));
        assert!(got.contains("action=\"/articles/1\""));
    }

    #[test]
    fn form_wrap_no_method_override_for_new_record() {
        let got = form_wrap("/articles", false, "contents", "");
        assert!(!got.contains("_method"));
    }
}
