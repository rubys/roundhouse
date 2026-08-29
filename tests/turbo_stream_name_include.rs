//! A channel that guards its own stream: `include Turbo::Streams::
//! StreamName::ClassMethods`.
//!
//! campfire's `RoomMessagesChannel` verifies the stream name at
//! subscribe time so that revoking a membership actually stops
//! delivery. The include runs at class-definition time, so the missing
//! module was not a channel that failed to subscribe — it was a tree
//! that failed to boot.
//!
//! The bug underneath was general: an include's `ClassId` is ONE Symbol
//! holding the whole path, and the const-anchor resolver keys on the
//! ROOT segment. Unsplit, `Turbo::Streams::StreamName::ClassMethods`
//! was its own root and matched nothing.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_channel() -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/channels/application_cable/channel.rb"),
        b"module ApplicationCable\n  class Channel < ActionCable::Channel::Base\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/channels/room_messages_channel.rb"),
        br#"class RoomMessagesChannel < ApplicationCable::Channel
  include Turbo::Streams::StreamName::ClassMethods

  def subscribed
    if stream_name = verified_stream_name_from_params
      stream_from stream_name
    else
      reject
    end
  end
end
"#
        .to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // `emit_library`: channels are ingested as ordinary app classes and
    // land in `app.library_classes`, not in the lowered-model set.
    ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.ends_with("room_messages_channel.rb"))
        .expect("no channel emitted")
        .content
}

#[test]
fn the_include_carries_a_require_for_the_module_it_names() {
    let src = emitted_channel();
    assert!(src.contains("include Turbo::Streams::StreamName::ClassMethods"), "{src}");
    assert!(src.contains("runtime/turbo_streams"), "{src}");
}

/// TWO ends now, where there were three: `turbo_stream_from` writes the
/// `--unsigned` suffix and `Turbo::Streams::StreamName.verified` reads
/// it. The overlay's `cable.rb` used to carry a THIRD — its own
/// `decode_stream_name`, because a subscribe decoded the name itself
/// instead of routing to the channel that owns it. Channel dispatch
/// removed that reason, and the decoder with it, so this asserts the
/// spelling is gone from cable.rb rather than matching there too.
#[test]
fn the_runtime_module_shares_the_unsigned_encoding() {
    let module = std::fs::read_to_string("runtime/spinel/turbo_streams.rb").expect("read");
    assert!(module.contains(r#"split("--", 2)"#), "{module}");
    let cable = std::fs::read_to_string("runtime/spinel/scaffold/ruby_overlay/cable.rb")
        .expect("read cable");
    assert!(
        !cable.contains(r#"split("--", 2)"#),
        "the overlay decodes a stream name again instead of letting the channel it \
         dispatched to do it — that is the second spelling coming back:\n{cable}"
    );
    let helpers =
        std::fs::read_to_string("runtime/ruby/action_view/view_helpers.rb").expect("read helpers");
    assert!(helpers.contains("--unsigned"), "{helpers}");
}
