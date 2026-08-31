//! A forwarded `**` bundle landed in an OPTIONAL KEYWORD's slot.
//!
//! campfire's timestamps:
//!
//! ```ruby
//! def local_datetime_tag(datetime, style: :time, **attributes)
//! def message_timestamp(message, **attributes)
//!   local_datetime_tag message.created_at, **attributes
//! ```
//!
//! Ingest flattens `style:` to a positional-with-default and
//! `**attributes` to a trailing positional, and erases the call's `**`
//! into a positional too — so the bundle slid one slot left, `style`
//! bound the whole hash and `attributes` bound `{}`. The rendered
//! `<time>` carried `data-local-time-target="{class: …}"` and lost its
//! `class`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

/// Emit the helper MODULES — the call site under test lives in one
/// helper's body calling another, not in a view.
///
/// `emit_library`, not `emit_spinel`: the helper modules ride out with
/// the former, and filtering the latter for them silently yields an
/// EMPTY string, which turns every `!contains` assertion into a
/// vacuous pass.
fn emit_helpers(time_helper: &str, rooms_helper: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/rooms_controller.rb"),
        b"class RoomsController < ApplicationController\n  def index\n    @rooms = Room.all\n  end\nend\n"
            .to_vec(),
    );
    tree.insert(PathBuf::from("app/helpers/time_helper.rb"), time_helper.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/helpers/rooms_helper.rb"), rooms_helper.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/views/rooms/index.html.erb"),
        b"<%= labelled(@rooms.first, class: \"x\") %>\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("labelled"),
        "harness emitted no helper module — every assertion below would be vacuous:\n{src}"
    );
    src
}

/// campfire's `local_datetime_tag`, reduced: an optional keyword sits
/// between the forwarded bundle and the `**rest` slot it was aimed at.
const TAGGED_KW: &str = "module TimeHelper\n  \
    def tagged(name, style: :time, **attributes)\n    attributes.merge(name: name, style: style)\n  end\nend\n";

/// The caller, campfire's `message_timestamp`: forwards its own `**`.
const FORWARDS: &str = "module RoomsHelper\n  \
    def labelled(room, **attributes)\n    tagged room.name, **attributes\n  end\nend\n";

#[test]
fn a_forwarded_bundle_moves_to_the_rest_slot_and_the_keyword_takes_its_default() {
    let src = emit_helpers(TAGGED_KW, FORWARDS);
    assert!(
        src.contains("TimeHelper.tagged(room.name, :time, attributes)"),
        "the bundle belongs in the `**rest` slot with `style`'s declared default \
         filled in ahead of it:\n{src}"
    );
}

/// The bug itself, stated as the thing that must not come back.
#[test]
fn the_bundle_never_binds_the_keyword_slot() {
    let src = emit_helpers(TAGGED_KW, FORWARDS);
    assert!(
        !src.contains("TimeHelper.tagged(room.name, attributes)"),
        "binding the whole hash to `style` is the defect:\n{src}"
    );
}

/// A LITERAL keyword list is `helper_kwargs`' business and still splices
/// by name. Run AFTER it, this pass would see a filled keyword slot and
/// pad it a second time — which is why the order is kwrest_forward then
/// helper_kwargs.
#[test]
fn a_literal_keyword_call_is_still_spliced_by_name_not_padded() {
    let caller = "module RoomsHelper\n  \
        def labelled(room, **attributes)\n    tagged room.name, style: :date\n  end\nend\n";
    let src = emit_helpers(TAGGED_KW, caller);
    assert!(
        src.contains("TimeHelper.tagged(room.name, :date)"),
        "the named keyword binds its own slot:\n{src}"
    );
    assert!(
        !src.contains(":time, :date"),
        "a spliced keyword must not then be treated as a forwarded bundle:\n{src}"
    );
}

/// A GENUINE optional positional is one Ruby lets a caller fill, so an
/// argument there says nothing and is left alone. This is the guard that
/// keeps the pass off ordinary `def f(a, b = 1, **o)` helpers.
#[test]
fn a_real_optional_positional_declines() {
    let callee = "module TimeHelper\n  \
        def tagged(name, style = :time, **attributes)\n    attributes.merge(name: name, style: style)\n  end\nend\n";
    let caller = "module RoomsHelper\n  \
        def labelled(room, **attributes)\n    tagged room.name, attributes\n  end\nend\n";
    let src = emit_helpers(callee, caller);
    assert!(
        src.contains("TimeHelper.tagged(room.name, attributes)"),
        "`style = :time` is fillable positionally in the source, so the call stands:\n{src}"
    );
}

/// Nothing between the bundle and the `**rest` slot: the erasure was
/// already correct, and `kwsplat`'s note about `**rest` needing no
/// special case holds.
#[test]
fn a_bundle_already_in_the_rest_slot_is_untouched() {
    let callee = "module TimeHelper\n  \
        def tagged(name, **attributes)\n    attributes.merge(name: name)\n  end\nend\n";
    let src = emit_helpers(callee, FORWARDS);
    assert!(
        src.contains("TimeHelper.tagged(room.name, attributes)"),
        "no keyword slot to skip, so no padding:\n{src}"
    );
}
