//! Rails' `tag.<name>(…)` builder → the HTML string it builds
//! (`lower::tag_builder`), in HELPER METHOD BODIES.
//!
//! `tag` is a `method_missing` proxy. Nothing supplies it in an emitted
//! tree — a helper lowers to a module function with no view context — so
//! the call survived into the emit as a literal `tag.details(…)` with
//! `tag` unbound and took the page down at render. campfire writes ~55
//! of these across 21 files.
//!
//! ERB is NOT this pass's territory: the view walker already owns
//! `<%= tag.x do %> … <% end %>`, whose block body is template buffer
//! ops rather than plain Ruby. These tests pin the helper-body half.

use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::tag_builder::apply_tag_builder_lowering;
use roundhouse::App;

fn lower(source: &str) -> (String, usize) {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let diags = apply_tag_builder_lowering(&mut app);
    let out = roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags.len())
}

fn helper(body: &str) -> (String, usize) {
    lower(&format!("module H\n  def h\n    {body}\n  end\nend\n"))
}

#[test]
fn void_element_gets_no_closing_tag() {
    // campfire's layout writes five of these; `</meta>` is not HTML.
    let (out, diags) = helper(r#"tag.meta name: "turbo-prefetch", content: "true""#);
    // Literal attribute values render as interpolated parts rather
    // than inlined text — the same shape the form builder emits.
    assert!(out.contains("<meta name="), "expected a meta tag:\n{out}");
    assert!(out.contains("content="), "expected the content attr:\n{out}");
    assert!(out.contains(r#">".html_safe"#), "tag should close and be safe:\n{out}");
    assert!(!out.contains("</meta>"), "void elements take no closing tag:\n{out}");
    assert_eq!(diags, 0, "a literal-hash call should not ledger");
}

#[test]
fn non_void_element_with_no_content_still_closes() {
    let (out, _) = helper(r#"tag.div class: "x""#);
    assert!(out.contains("<div class="), "expected a div:\n{out}");
    assert!(out.contains("></div>"), "expected an empty but closed div:\n{out}");
}

#[test]
fn leading_positional_is_escaped_content() {
    let (out, _) = helper(r#"tag.span("Translate", class: "for-screen-reader")"#);
    assert!(out.contains("<span"), "expected a span:\n{out}");
    assert!(out.contains("</span>"), "expected a closing span:\n{out}");
    assert!(
        out.contains("Translate"),
        "expected the content rendered:\n{out}"
    );
}

#[test]
fn data_hash_flattens_and_kebab_cases() {
    // Rails' `tag_options` expands `data:` in place, mapping `_` → `-`.
    // campfire's popup controller depends on the exact attribute names.
    let (out, _) = helper(
        r#"tag.div class: "m", data: { popup_target: "menu", controller: "popup" }"#,
    );
    assert!(
        out.contains("data-popup-target") && out.contains("data-controller"),
        "expected flattened kebab-cased data attributes:\n{out}"
    );
    assert!(
        !out.contains("data=\\\"") && !out.contains("popup_target=") ,
        "the data hash must not render as one attribute:\n{out}"
    );
}

#[test]
fn falsy_boolean_attribute_is_omitted_not_rendered_false() {
    // `disabled="false"` reads to a browser as DISABLED — the opposite.
    let (out, _) = helper("tag.button disabled: false");
    assert!(!out.contains("disabled"), "a false boolean must vanish:\n{out}");
}

#[test]
fn result_is_marked_html_safe_so_callers_do_not_escape_it() {
    // Rails' builder returns a SafeBuffer. Lowered to a bare string the
    // view walker would escape it, and `&lt;meta&gt;` is not a meta tag.
    let (out, _) = helper(r#"tag.br"#);
    assert!(
        out.contains("html_safe"),
        "the built string must carry the safe marker:\n{out}"
    );
}

#[test]
fn a_local_named_tag_is_not_the_builder() {
    // campfire's Opengraph::Document iterates parsed nodes as |tag| and
    // calls `tag.key?(…)`. Rewriting that would be silently destructive,
    // and `link` IS an HTML element name.
    let (out, diags) = lower(
        "module H\n  def h(nodes)\n    nodes.map { |tag| tag.link }\n  end\nend\n",
    );
    assert!(
        out.contains("tag.link"),
        "a bound local named `tag` must be left alone:\n{out}"
    );
    assert_eq!(diags, 0, "leaving a non-builder alone is not a ledger line");
}

#[test]
fn unknown_element_name_declines_and_ledgers() {
    let (out, diags) = helper("tag.not_an_element class: \"x\"");
    assert!(out.contains("not_an_element"), "call should survive:\n{out}");
    assert_eq!(diags, 1, "an unknown element should be ledgered");
}

#[test]
fn computed_attributes_fall_back_to_the_runtime_and_ledger() {
    // campfire's `tag.time **attributes, datetime: …` desugars to a
    // merge chain, which cannot be walked at compile time. The fallback
    // is what keeps the pass TOTAL — no site is left with unbound `tag`.
    let (out, diags) = lower(
        "module H\n  def h(attrs)\n    tag.time \"x\", attrs.merge({ a: 1 })\n  end\nend\n",
    );
    assert!(
        out.contains("content_tag"),
        "expected the runtime fallback:\n{out}"
    );
    assert_eq!(diags, 1, "the fallback should be ledgered");
}

// ---- the LEGACY function form ---------------------------------------

/// `tag(:meta, name: …)` is ActionView's ORIGINAL helper, not the
/// `method_missing` builder, and it renders differently. MEASURED
/// against Rails 8:
///
///   tag(:meta, name: "n", content: "c")  =>  <meta name="n" content="c" />
///   tag.meta(name: "n")                  =>  <meta name="n">
///
/// campfire's `current_user_meta_tags` writes two of them, so every
/// page's `<head>` depended on it.
#[test]
fn the_legacy_function_form_self_closes() {
    let (out, diags) = helper(r#"tag(:meta, name: "current-user-id", content: "7")"#);
    assert!(out.contains("<meta name="), "expected a meta tag:\n{out}");
    assert!(
        out.contains(r#" />".html_safe"#),
        "the legacy form keeps the XHTML close the builder drops:\n{out}"
    );
    assert!(!out.contains("</meta>"), "and takes no closing tag:\n{out}");
    assert_eq!(diags, 0, "a literal-hash call should not ledger");
}

/// The attribute renderer is shared with the builder, so a `ViewHelpers`
/// receiver has to be qualified here too — bare, it resolves against the
/// enclosing helper module and raises NameError.
#[test]
fn the_legacy_form_qualifies_its_escape_receiver() {
    let (out, _) = helper(r#"tag(:meta, name: "x", content: value)"#);
    assert!(
        out.contains("ActionView::ViewHelpers.html_escape"),
        "the escape receiver is qualified:\n{out}"
    );
}

/// No attributes at all — `tag(:br)`.
#[test]
fn the_legacy_form_works_without_options() {
    let (out, diags) = helper("tag(:br)");
    assert!(out.contains("<br />"), "expected a self-closed br:\n{out}");
    assert_eq!(diags, 0);
}

/// A computed element name has nothing to expand at compile time, and
/// an unknown one is not an element we build. Both leave the call
/// alone rather than guessing.
#[test]
fn the_legacy_form_declines_what_it_cannot_resolve() {
    let (computed, _) = helper("tag(name_var, class: \"x\")");
    assert!(
        computed.contains("tag(name_var"),
        "a computed element name is left alone:\n{computed}"
    );
    let (unknown, diags) = helper(r#"tag(:not_an_element, class: "x")"#);
    assert!(
        unknown.contains("tag(:not_an_element"),
        "an unknown element is left alone:\n{unknown}"
    );
    assert_eq!(diags, 1, "and is ledgered:\n{unknown}");
}

// ── button_to, the BLOCK spelling ────────────────────────────────
//
// Rails' `button_to(url, html_options) { … }` — first argument the
// URL, block the button's content. The view walker owns the ERB
// spelling; campfire writes three of these in HELPER modules
// (rooms, users, rooms/involvements), where the emitted call landed
// on a module that defines no `button_to`.

#[test]
fn button_to_with_a_block_expands_to_the_form_wrapper() {
    let (out, _) = helper(
        r#"button_to "/rooms/1", method: :put, class: "btn" do
      "content"
    end"#,
    );
    assert!(out.contains("<form action="), "expected the form wrapper:\n{out}");
    // `method:` decides the FORM's override input, not a button attr.
    assert!(out.contains("method_override_input"), "expected the _method input:\n{out}");
    assert!(out.contains(r#"<button type=\"submit\""#), "expected the submit button:\n{out}");
    // Everything else in the options hash is a `<button>` attribute.
    assert!(out.contains("class="), "expected the class attr:\n{out}");
    assert!(out.contains("csrf_token_hidden_input"), "expected the CSRF input:\n{out}");
    assert!(out.contains("</button>"), "expected the button to close:\n{out}");
    assert!(out.contains("</form>"), "expected the form to close:\n{out}");
    // The block is CAPTURED, not escaped — Rails treats what it
    // yields as the button's HTML content.
    assert!(!out.contains("button_to \""), "the bare call should be gone:\n{out}");
}

#[test]
fn button_to_without_a_block_is_left_to_the_runtime() {
    // The POSITIONAL form `(text, href, opts)` is what the runtime
    // `ActionView::ViewHelpers.button_to` implements; this pass claims
    // only the block spelling.
    let (out, _) = helper(r#"button_to "Delete", "/rooms/1", method: :delete"#);
    assert!(out.contains("button_to \""), "positional form should survive:\n{out}");
    assert!(!out.contains("<form action="), "positional form is not expanded here:\n{out}");
}

#[test]
fn button_to_with_computed_options_declines() {
    // A `merge` chain has nothing to split `method:`/`form_class:` out
    // of at compile time, and those two decide the FORM rather than the
    // button. Leave the site loud instead of rendering a POST form for
    // what the caller meant as a PUT.
    let (out, _) = helper(r#"button_to "/rooms/1", opts.merge(class: "btn") do
      "x"
    end"#);
    assert!(out.contains("button_to \""), "computed options should decline:\n{out}");
    assert!(!out.contains("<form action="), "declined site must not expand:\n{out}");
}
