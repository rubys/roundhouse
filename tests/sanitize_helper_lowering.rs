//! `include ActionView::Helpers::SanitizeHelper` in a MODEL, and the
//! two helpers it brings.
//!
//! campfire's `Opengraph::Metadata` includes it to call
//! `sanitize(strip_tags(title))` on its own attributes. Both halves
//! were missing: no target ships an `ActionView::Helpers` namespace, so
//! the surviving `include` was an `uninitialized constant` at
//! class-definition time (a boot failure, not a route failure), and the
//! two calls emitted bare because the model-side helper table did not
//! name them.
//!
//! The runtime bodies themselves are checked against the real
//! `Rails::HTML5::FullSanitizer` in
//! `runtime/ruby/action_view/view_helpers_ext.rb`'s own notes; what
//! this file gates is the COMPILER half — that the include goes and the
//! calls come out qualified.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(model_src: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/card.rb"), model_src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // `emit_spinel`, not `emit_lowered_models`: the bare-call
    // qualification is a whole-app pass (it needs the helper index), so
    // the narrower entry point emits the model with its calls still
    // bare and would gate nothing.
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.ends_with("card.rb"))
        .expect("no card.rb emitted")
        .content
}

const CARD: &str = r#"class Card
  include ActiveModel::Model
  include ActionView::Helpers::SanitizeHelper

  attr_accessor :title

  def clean
    self.title = sanitize(strip_tags(title))
  end
end
"#;

#[test]
fn the_helper_include_does_not_survive() {
    let src = emitted(CARD);
    // The include runs at class-definition time, so leaving it in is a
    // tree that does not boot — not a route that 500s.
    assert!(!src.contains("ActionView::Helpers"), "{src}");
}

#[test]
fn its_members_come_out_qualified() {
    let src = emitted(CARD);
    assert!(
        src.contains(
            "ActionView::ViewHelpers.sanitize(ActionView::ViewHelpers.strip_tags(title))"
        ),
        "{src}"
    );
}
