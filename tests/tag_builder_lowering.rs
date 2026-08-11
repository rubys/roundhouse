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
