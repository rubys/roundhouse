//! A core-class reopen keeps its `self.` receiver.
//!
//! campfire's `lib/rails_ext/string.rb` is, verbatim:
//!
//! ```text
//! class String
//!   def all_emoji?
//!     self.match? /\A(\p{Emoji_Presentation}|\p{Extended_Pictographic}|️)+\z/u
//!   end
//! end
//! ```
//!
//! The author wrote `self.`, and the Ruby emitter used to drop it: the
//! SelfRef-implicit shortcut elides an explicit receiver from any send
//! that carries arguments, because a bare name with args always parses
//! as a call. On CRuby that is true and the elision is invisible. On a
//! strict target it is not — the emitted bareword no longer resolves to
//! `String#match?`, and spinel answers
//!
//! ```text
//! spinel: string.rb:3: unsupported call: CallNode `match?` recv=-
//! ```
//!
//! which was one of campfire's remaining spinel-lane walls. Verified
//! both ways against spinel master: with `self.` the file compiles and
//! `"🔔".all_emoji?` answers true; without it, the line above.
//!
//! THIS FILE ASSERTS THE EMITTED TEXT, and that is the point. An earlier
//! attempt fixed the same wall with a post-analyze pass that gave those
//! sends an explicit `self` — the pass fired, its own unit assertions
//! passed at IR level, and the emit did not change by one byte, because
//! the elision downstream erased what it had just written. A lowering
//! can be correct in the IR and undone by the emitter; only the emitted
//! string can tell you which.

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

fn emit_reopen(src: &str, stem: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/message.rb"),
        b"class Message < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("lib/rails_ext/core.rb"), src.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(&format!("{stem}.rb")))
        .unwrap_or_else(|| panic!("no {stem}.rb emitted"))
        .content
}

#[test]
fn a_core_class_reopen_keeps_self_on_a_send_with_arguments() {
    let src = emit_reopen(
        "class String\n  def all_emoji?\n    self.match?(/\\A\\p{Emoji_Presentation}+\\z/)\n  end\nend\n",
        "string",
    );
    assert!(
        src.contains("self.match?"),
        "the `self.` receiver was elided — the emitted bareword does not \
         resolve to String#match? on a strict target:\n{src}"
    );
}

/// The rule keys on the ENCLOSING CLASS, not on the method name. An
/// ordinary library class is unchanged: its self-sends with arguments
/// still elide, which is how every other emitted body reads.
#[test]
fn an_ordinary_library_class_still_elides() {
    let src = emit_reopen(
        "class Formatter\n  def render(x)\n    self.wrap(x, 2)\n  end\n\n  def wrap(x, n)\n    x\n  end\nend\n",
        "formatter",
    );
    assert!(
        src.contains("wrap(x, 2)"),
        "expected the elided form in a non-core class:\n{src}"
    );
    assert!(
        !src.contains("self.wrap"),
        "a non-core class should not have gained an explicit receiver:\n{src}"
    );
}

/// And a ZERO-ARG read keeps `self.` in both, which it always did — the
/// two conditions are independent, so a change to one must not quietly
/// swallow the other.
#[test]
fn a_zero_arg_read_keeps_self_in_an_ordinary_class_too() {
    let src = emit_reopen(
        "class Formatter\n  def render\n    self.width\n  end\n\n  def width\n    2\n  end\nend\n",
        "formatter",
    );
    assert!(
        src.contains("self.width"),
        "a zero-arg self read must keep its receiver — a bare name there \
         reads as a local:\n{src}"
    );
}
