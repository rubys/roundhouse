//! Native keyword packets stay packets.
//!
//! Ordinary application ingestion now keeps keyword parameters, and
//! `analyze::forwarding` classifies a verified source keyword destination
//! as `Native`. `lower::forwarding` therefore leaves that call's
//! `KeywordSplat` intact instead of projecting it to a positional Hash
//! and expanding each key at the caller.
//!
//! The retained contract is Ruby's: the packet is evaluated once, the
//! callee owns defaults and unknown-key `ArgumentError`, and a later
//! packet wins over an earlier literal. Ingest still writes that mix as
//! a merge; the lowerer puts `**` back around the merge instead of
//! indexing each key at the caller. A positional Hash beside `*rest` is
//! not given a splat. An untyped receiver of a known keyword selector is
//! refused, and the packet is not expanded.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::{Diagnostic, Severity};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

static RUN: AtomicU64 = AtomicU64::new(0);

/// Ingest → analyze → the shared post-analyze lowerings → ruby render.
/// This is the pipeline that classifies keyword destinations; calling
/// `apply_kwsplat_expansion` alone re-runs a projection the classifier
/// has already declined to make.
fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let emitted = emit_app(source);
    (emitted.source, emitted.diags)
}

fn errors(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags.iter().filter(|d| d.severity == Severity::Error).collect()
}

fn keyword_refusal(diags: &[Diagnostic]) -> bool {
    errors(diags).iter().any(|d| {
        d.message.contains("keyword destination's native argument ABI cannot be verified")
            || d.message
                .contains("keyword producer's full forwarding destination cannot be verified")
    })
}

struct Emitted {
    source: String,
    diags: Vec<Diagnostic>,
    files: Vec<roundhouse::emit::EmittedFile>,
}

fn emit_app(source: &str) -> Emitted {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest");
    let mut app = App::new();
        app.library_classes.extend(classes);
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags =
        roundhouse::lower::apply_post_analyze_lowerings(&mut app, analyzer.class_registry());
    let files = emit_library(&app);
    let source = files
        .iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    Emitted { source, diags, files }
}

/// Run the emitted library against an assertion script. The script is
/// the contract; a quiet diagnostic list is not.
fn run_emitted(emitted: &Emitted, script: &str) {
    assert!(
        errors(&emitted.diags).is_empty(),
        "a verified native packet must not be refused:\n{}\n{:?}",
        emitted.source,
        emitted.diags
    );
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-kwsplat-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut requires = Vec::new();
    for file in &emitted.files {
        if file.path.extension().is_some_and(|e| e == "rb") {
            let path = dir.join(&file.path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, &file.content).expect("write");
            requires.push(format!(
                "require_relative {:?}",
                file.path.with_extension("").to_string_lossy()
            ));
        }
    }
    let output = Command::new("ruby")
        .arg("-e")
        .arg(format!("{}\n{script}", requires.join("\n")))
        .current_dir(&dir)
        .output()
        .expect("ruby");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "emitted native packet failed:\nstdout:\n{}\nstderr:\n{}\nemitted:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        emitted.source
    );
}

const BADGE: &str = r##"
class Badge
  def svg(**opts)
    render_code(size: 2, **opts)
  end

  def computed(**opts)
    render_code(size: compute, **opts)
  end

  def from_call
    render_code(size: 2, **defaults)
  end

  def compute
    @ticks = (@ticks || 0) + 1
  end

  def defaults
    @ticks = (@ticks || 0) + 1
    { color: "red" }
  end

  def render_code(size:, color: "black")
    "#{size}:#{color}"
  end

  def ticks
    @ticks || 0
  end
end
"##;

#[test]
fn a_verified_keyword_destination_keeps_the_packet() {
    let (out, diags) = lower_and_emit(
        r#"
class Image
  def initialize(name:, width:, height:)
    @name = name
  end

  def label
    @name
  end
end

class Sound
  def build(image)
    Image.new(**image)
  end
end
"#,
    );
    assert!(
        out.contains("Image.new(**image)"),
        "a verified keyword constructor keeps the packet, it does not expand it:\n{out}"
    );
    assert!(
        !out.contains("image[:name]") && !out.contains("image.fetch"),
        "expansion would copy callee binding into the caller:\n{out}"
    );
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let emitted = emit_app(
        r##"
class Image
  def initialize(name:, width:, height:)
    @name = name
    @width = width
    @height = height
  end

  def label
    "#{@name}:#{@width}:#{@height}"
  end
end

class Sound
  def build(image)
    Image.new(**image)
  end
end
"##,
    );
    assert!(
        emitted.source.contains("Image.new(**image)"),
        "the runnable shape must keep the packet too:\n{}",
        emitted.source
    );
    run_emitted(
        &emitted,
        r##"
image = Sound.new.build({ name: "a", width: 2, height: 3 })
raise "packet did not bind keywords: #{image.label}" unless image.label == "a:2:3"
begin
  Sound.new.build({ name: "a", width: 2 })
  raise "missing keyword was accepted"
rescue ArgumentError
end
begin
  Sound.new.build({ name: "a", width: 2, height: 3, extra: 1 })
  raise "unknown keyword was accepted"
rescue ArgumentError
end
"##,
    );
}

/// `push` is an untyped parameter, so `notification`'s keyword-rest ABI
/// cannot be verified. The packet stays a splat — expanding it would
/// invent keys — and the site is refused rather than projected away.

#[test]
fn an_untyped_receiver_into_keyword_rest_is_refused_without_expansion() {
    let (out, diags) = lower_and_emit(
        r#"
class Push
  def notification(**params)
    params
  end
end

class Pool
  def deliver(push, payload)
    push.notification(**payload)
  end
end
"#,
    );
    assert!(
        out.contains("notification(**payload)"),
        "refusal must keep the source packet, not expand or drop it:\n{out}"
    );
    assert!(
        !out.contains("payload[:") && !out.contains("payload.fetch"),
        "an unverified keyword-rest is not a list of indexes:\n{out}"
    );
    assert!(
        keyword_refusal(&diags),
        "an untyped receiver of a known keyword selector must be refused:\n{out}\n{diags:?}"
    );
}

/// `def f(*items, **opts); f(payload)` is a valid positional call.
/// Restoring `**payload` would move the Hash from `items` onto `opts`.
#[test]
fn a_positional_rest_beside_keyword_rest_is_not_an_erased_splat() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define(version: 1) do\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br#"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "forwards" do
    consume(payload)
  end

  private
    def consume(*items, **opts)
      items
    end
end
"#
        .to_vec(),
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("consume(payload)") || src.contains("consume(payload,"),
        "a *items,**opts callee must keep the positional Hash:\n{src}"
    );
    assert!(
        !src.contains("consume(**payload)"),
        "must not restore a splat that *items already accepted:\n{src}"
    );
}

#[test]
fn optional_keywords_keep_defaults_in_the_callee() {
    let (out, diags) = lower_and_emit(
        r#"
class Tag
  def initialize(name:, size: 48)
    @name = name
  end
end

class Builder
  def build(opts)
    Tag.new(**opts)
  end
end
"#,
    );
    assert!(
        out.contains("Tag.new(**opts)"),
        "the packet stays intact; the callee default is not copied to the caller:\n{out}"
    );
    assert!(
        !out.contains("fetch(:size") && !out.contains("opts[:size]"),
        "a caller-side fetch would restate the default outside the callee:\n{out}"
    );
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn a_computed_default_stays_in_the_callee_and_the_packet_is_kept() {
    let (out, diags) = lower_and_emit(
        r#"
class Tag
  def initialize(name:, owner: Current.user)
    @name = name
  end
end

class Builder
  def build(opts)
    Tag.new(**opts)
  end
end
"#,
    );
    assert!(
        out.contains("def initialize(name:, owner: Current.user)"),
        "the computed default stays where it was written:\n{out}");
    assert!(
        out.contains("Tag.new(**opts)"),
        "a computed default is no longer a reason to project the packet:\n{out}"
    );
    assert!(
        !out.contains("Tag.new(opts)") && !out.contains("opts.fetch"),
        "neither the positional projection nor a caller-side fetch is the contract:\n{out}"
    );
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn an_impure_packet_is_retained_and_evaluated_once() {
    let (out, diags) = lower_and_emit(
        r#"
class Image
  def initialize(name:, width:)
    @name = name
  end
end

class Sound
  def build(source)
    Image.new(**source.dimensions)
  end
end
"#,
    );
    assert!(
        out.contains("Image.new(**source.dimensions)"),
        "the packet expression stays one splat, not one index per keyword:\n{out}"
    );
    assert!(errors(&diags).is_empty(), "{diags:?}");
    let emitted = emit_app(
        r#"
class Image
  def initialize(name:, width:)
    @seen = [name, width]
  end

  def seen
    @seen
  end
end

class Source
  def initialize
    @reads = 0
  end

  def reads
    @reads
  end

  def dimensions
    @reads += 1
    { name: "once", width: 4 }
  end
end

class Sound
  def build(source)
    Image.new(**source.dimensions)
  end
end
"#,
    );
    assert!(
        emitted.source.contains("Image.new(**source.dimensions)"),
        "the runnable packet must still be one splat:\n{}",
        emitted.source
    );
    run_emitted(
        &emitted,
        r##"
source = Source.new
image = Sound.new.build(source)
raise "packet rebound: #{image.seen.inspect}" unless image.seen == ["once", 4]
raise "packet expression ran #{source.reads} times" unless source.reads == 1
"##,);
}

#[test]
fn literal_keyword_call_is_untouched() {
    let (out, diags) = lower_and_emit(
        r#"
class Image
  def initialize(name:, width:)
    @name = name
  end
end

class Sound
  def build
    Image.new(name: "a", width: 1)
  end
end
"#,
    );
    assert!(
        out.contains(r#"Image.new(name: "a", width: 1)"#),
        "literal kwargs must survive verbatim:\n{out}"
    );
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn a_positional_argument_beside_a_packet_is_not_part_of_the_packet() {
    let (out, diags) = lower_and_emit(
        r#"
class Image
  def initialize(id, name:, width:)
    @id = id
  end
end

class Sound
  def build(id, opts)
    Image.new(id, **opts)
  end
end
"#,
    );
    assert!(
        out.contains("Image.new(id, **opts)"),
        "the positional stays positional and the packet stays a packet:\n{out}"
    );
    assert!(!out.contains("opts[:name]"), "{out}");
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn a_positional_rest_destination_does_not_steal_a_keyword_packet() {
    // `*args` can absorb a positional Hash, so `Logger.new(**opts)` is
    // not evidence that the Hash was positional. The keyword packet has
    // to remain a keyword packet; otherwise `level:` never binds.
    let (out, diags) = lower_and_emit(
        r#"
class Logger
  def initialize(*args, level:)
    @args = args
  end
end

class Sound
  def build(opts)
    Logger.new(**opts)
  end
end
"#,
    );
    assert!(
        out.contains("Logger.new(**opts)"),
        "a *rest callee still receives keywords only through the splat:\n{out}"
    );
    assert!(!out.contains("Logger.new(opts)"), "{out}");
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn explicit_keywords_beside_a_packet_stay_beside_it() {
    let (out, diags) = lower_and_emit(
        r#"
class Notification
  def initialize(title:, body:, badge:, endpoint:)
    @title = title
  end
end

class Subscription
  def notification(**params)
    Notification.new(**params, badge: unread, endpoint: endpoint)
  end
end
"#,
    );
    assert!(
        out.contains("Notification.new(**params, badge: unread, endpoint: endpoint)")
            || out.contains(
                "Notification.new(**params.merge({ badge: unread, endpoint: endpoint }))"
            ),
        "explicit keywords stay at the call; they are not indexed out of the packet:\n{out}"
    );
    assert!(!out.contains("params[:title]"), "{out}");
    assert!(errors(&diags).is_empty(), "{diags:?}"
    );
}

#[test]
fn a_later_packet_overrides_an_earlier_literal_inside_the_callee() {
    let emitted = emit_app(BADGE);
    let (out, diags) = (emitted.source.as_str(), &emitted.diags
    );
    assert!(
        out.contains("render_code(**{ size: 2 }.merge(opts))"),
        "the literal and the later packet survive as one splatted merge:\n{out}"
    );
    assert!(
        !out.contains("opts.fetch"),
        "the callee, not a caller-side fetch, decides which key wins:\n{out}"
    );
    assert!(errors(diags).is_empty(), "{diags:?}");
    run_emitted(
        &emitted,
        r##"
badge = Badge.new
raise "later packet must override the literal: #{badge.svg(size: 9, color: "red")}" unless badge.svg(size: 9, color: "red") == "9:red"
raise "omitted packet key must use the literal, not the callee default: #{badge.svg}" unless badge.svg == "2:black"
raise "false and nil must survive the packet: #{badge.svg(size: false, color: nil).inspect}" unless badge.svg(size: false, color: nil) == "false:"
begin
  badge.svg(unknown: 1)
  raise "unknown key was accepted"
rescue ArgumentError
end
"##,
    );
}

#[test]
fn an_impure_bundle_beside_a_literal_is_evaluated_once() {
    let emitted = emit_app(BADGE);
    let (out, diags) = (emitted.source.as_str(), &emitted.diags
    );
    assert!(
        out.contains("render_code(**{ size: 2 }.merge(defaults))"),
        "the bundle stays one splatted merge, not one read per keyword:\n{out}"
    );
    assert!(
        !out.contains(".fetch(:size") && !out.contains(".fetch(:color"),
        "fetch would evaluate the bundle once per keyword:\n{out}"
    );
    assert!(errors(diags).is_empty(), "{diags:?}");
    run_emitted(
        &emitted,
        r##"
badge = Badge.new
raise "bundle lost its override: #{badge.from_call}" unless badge.from_call == "2:red"
raise "bundle ran #{badge.ticks} times" unless badge.ticks == 1
"##,);
}

#[test]
fn a_computed_value_beside_a_later_packet_is_evaluated_even_when_overridden() {
    let emitted = emit_app(BADGE);
    let (out, diags) = (emitted.source.as_str(), &emitted.diags
    );
    assert!(
        out.contains("render_code(**{ size: compute }.merge(opts))"),
        "the computed value stays inside one splatted merge, ahead of the packet:\n{out}"
    );
    assert!(errors(diags).is_empty(), "{diags:?}");
    run_emitted(
        &emitted,
        r##"
badge = Badge.new
raise "packet did not override the computed value: #{badge.computed(size: 9)}" unless badge.computed(size: 9) == "9:black"
raise "overridden computed value ran #{badge.ticks} times" unless badge.ticks == 1
raise "omitted key did not use the computed value: #{badge.computed}" unless badge.computed == "2:black"
raise "computed value ran #{badge.ticks} times" unless badge.ticks == 2
"##,
    );
}

/// A test helper forwarding `**attributes` into another helper that
/// declares keywords. The packet is retained; optional defaults stay in
/// the callee, including when the caller omits them.
#[test]
fn a_test_helper_forwarding_a_packet_keeps_it() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define(version: 1) do\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br#"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "forwards" do
    assert_equal "a", embed_from(href: "a", url: "b")
  end

  private
    def attachment_for(href:, url:, filename: "Title", caption: "Description")
      href
    end

    def embed_from(**attributes)
      attachment_for(**attributes)
    end
end
"#
        .to_vec(),
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("attachment_for(**attributes)"),
        "the forwarded packet stays a packet against the class's own helper:\n{src}"
    );
    assert!(
        src.contains("filename: \"Title\"") && src.contains("caption: \"Description\""),
        "optional defaults stay on the callee, not copied onto the caller:\n{src}"
    );
    assert!(
        !src.contains("attributes.fetch") && !src.contains("attributes[:href]"),
        "must not expand the packet at the caller:\n{src}"
    );
}

/// `f(**h)` into `def f(**rest)` keeps both the splat and the keyword-rest
/// parameter. Projecting either side to a positional Hash is the Ruby 3
/// arity error this pass used to paper over by flattening.
#[test]
fn a_splat_into_keyword_rest_keeps_both_sides() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define(version: 1) do\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br#"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "forwards" do
    assert_equal({ href: "a", url: "b" }, embeds_from(href: "a", url: "b"))
  end

  private
    def attachments_for(**details)
      details
    end

    def embeds_from(**details)
      attachments_for(**details)
    end
end
"#
        .to_vec(),
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("def attachments_for(**details)"),
        "a consuming **rest stays a keyword-rest, not a positional Hash:\n{src}"
    );
    assert!(
        src.contains("attachments_for(**details)"),
        "the call into that def keeps the splat:\n{src}"
    );
    assert!(
        src.contains("def embeds_from(**details)"),
        "the forwarding def keeps its keyword-rest:\n{src}"
    );
    assert!(
        ! src.contains("details = {}"),
        "flattening the rest would make the restored splat unexpected keywords:\n{src}"
    );
}

/// A chain of forwarding `**rest` defs keeps the keyword-rest on every
/// def and restores the splat on every call. A positional `wrap(payload)`
/// is not that call and must not gain a splat.
#[test]
fn a_forwarding_keyword_rest_chain_keeps_every_packet() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define(version: 1) do\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("test/models/room_test.rb"),
        br#"require "test_helper"

class RoomTest < ActiveSupport::TestCase
  test "forwards" do
    wrap(payload)
  end

  private
    def other(**details)
      details
    end

    def consume(**details)
      other(**details)
    end

    def wrap(**details)
      consume(**details)
    end
end
"#
        .to_vec(),
    );
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = roundhouse::emit::ruby::emit_spinel(&app)
        .into_iter()
        .filter(|f| f.path.to_string_lossy().contains("room_test"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    for name in ["other", "consume", "wrap"] {
    assert!(
        src.contains(&format!("def {name}(**details)")),
            "{name} must keep its keyword-rest:\n{src}"
        );
    }
    assert!( src.contains("other(**details)"), "{src}"
    );
    assert!(
        src.contains("consume(**details)"), "{src}");
    // A bare Hash remains positional even when the callee rejects its arity.
    // Argument-count heuristics must not convert that error into success.
    assert!(
        src.contains("wrap(payload)"), "a positional Hash must not become keywords:\n{src}");
}

/// A constructed receiver is verified, so the keyword-rest call the
/// untyped-receiver case refuses is retained and run. The packet binds
/// as keywords, including an empty packet and a false value. A bare
/// positional Hash must preserve Ruby's ArgumentError instead.
#[test]
fn a_constructed_keyword_rest_receiver_keeps_and_runs_the_packet() {
    let source = r#"
class Push
  def notification(**params)
    params
  end
end

class Pool
  def deliver(payload)
    push = Push.new
    push.notification(**payload)
  end

  def positional(payload)
    push = Push.new
    push.notification(payload)
  end
end
"#;
    let emitted = emit_app(source);
    assert!(
        emitted.source.contains("notification(**payload)"),
        "a verified keyword-rest keeps the splat:\n{}",
        emitted.source
    );
    assert!(
        !emitted.source.contains("payload[:") && !emitted.source.contains("payload.fetch"),
        "keyword-rest is not expanded into indexes:\n{}",
        emitted.source
    );
    assert!(errors(&emitted.diags).is_empty(), "{:?}", emitted.diags);
    run_emitted(
        &emitted,
        r##"
pool = Pool.new
raise "packet did not bind as keywords: #{pool.deliver({ href: "a", ok: false }).inspect}" unless pool.deliver({ href: "a", ok: false }) == { href: "a", ok: false }
raise "empty packet invented keys: #{pool.deliver({}).inspect}" unless pool.deliver({}) == {}
begin
  pool.positional({ href: "a" })
  raise "positional Hash was converted to keywords"
rescue ArgumentError
end
"##,
    );
}
