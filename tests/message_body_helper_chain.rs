//! `auto_link h(...)` — the expression campfire renders every message
//! body through, and the four missing pieces behind it.
//!
//! `MessagesHelper#message_presentation` is one line:
//!
//! ```text
//! auto_link h(ContentFilters::TextMessagePresentationFilters
//!   .apply(message.body.body)), html: { target: "_blank" }
//! ```
//!
//! and it is wrapped in the method's own `rescue Exception => e` that
//! returns `""`. So a single unqualified helper does not raise, does not
//! warn, and does not appear in any tally — it renders an EMPTY MESSAGE
//! BODY. Both halves were unqualified for as long as the benchmark
//! fixture seeded messages with no bodies to render, which is why the
//! DOM comparison read as exact parity the whole time.
//!
//! What this file gates is the COMPILER half: that a bare `h` and a bare
//! `auto_link` in a helper body come out qualified. The runtime halves
//! (the real `rails-html-sanitizer` behind `sanitize`/`strip_tags`, the
//! ported `auto_link`, and the html_safe mark on `ActionText::Content
//! #to_s`) live in the CRuby overlay and are exercised by the campfire
//! oracle comparison, where `/rooms/1/messages` is tag-for-tag identical
//! to Rails on real bodies.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "kind", null: false
  end
end
"#;

const HELPER: &str = r#"module MessagesHelper
  def message_presentation(message)
    auto_link h(message.kind), html: { target: "_blank" }
  end
end
"#;

fn emitted() -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/message.rb"),
        b"class Message < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/helpers/messages_helper.rb"), HELPER.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // Helpers ride `emit_library`, not the lowered-model set.
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("messages_helper.rb"))
        .expect("no messages_helper.rb emitted")
        .content
}

#[test]
fn both_halves_of_the_body_expression_are_qualified() {
    let src = emitted();
    assert!(
        src.contains("ActionView::ViewHelpers.auto_link("),
        "auto_link stayed bare — a NoMethodError inside campfire's rescue:\n{src}"
    );
    assert!(
        src.contains("ActionView::ViewHelpers.h("),
        "h stayed bare — a NoMethodError inside campfire's rescue:\n{src}"
    );
}

/// `h` is Rails' ALIAS for `html_escape`, not a second escape. The
/// runtime must define it as a delegate, because the CRuby overlay
/// replaces `html_escape` with an html_safe-aware version — a
/// separately-implemented `h` would quietly keep escaping the markup
/// Rails passes through, and every formatted message would render its
/// own tags as visible text.
#[test]
fn the_runtime_defines_h_as_a_delegate_to_html_escape() {
    let rt = std::fs::read_to_string("runtime/ruby/action_view/view_helpers.rb").expect("read");
    let at = rt.find("def self.h(value)").unwrap_or_else(|| panic!("no `h` in the runtime"));
    let body = &rt[at..at + 60];
    assert!(body.contains("html_escape(value.to_s)"), "{body}");
}

/// The safe-list sanitizer is the REAL gem on the CRuby lane, not a
/// hand-rolled scanner: the safe-list pass is HTML5 tree construction
/// (`"<b>a<p>b</b>c"` becomes `"<b>a</b><p><b>b</b>c</p>"`), and a
/// scanner is correct only on the well-formed inputs nobody attacks
/// with. Guarded, so an app that never sanitizes boots without it.
#[test]
fn the_overlay_sanitizer_rides_the_real_gem_and_is_guarded() {
    let overlay = std::fs::read_to_string(
        "runtime/spinel/scaffold/ruby_overlay/runtime/action_view_sanitize.rb",
    )
    .expect("read overlay");
    assert!(overlay.contains(r#"require "rails-html-sanitizer""#), "{overlay}");
    assert!(overlay.contains("rescue LoadError"), "{overlay}");
    assert!(overlay.contains("Rails::HTML5::SafeListSanitizer"), "{overlay}");
}
