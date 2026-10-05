//! `f(**h)` → `f(k: h[:k], …)` (the shared `lower::apply_kwsplat_expansion`
//! pass, run on the post-analyze hook).
//!
//! Ingest desugars every double splat into the `merge` chain it is
//! defined to be, which erases the `**`. That is correct into a `**rest`
//! callee — ingest models such a parameter as a trailing positional, so
//! both sides agree a keyword bundle is one Hash — and wrong into a
//! callee declaring explicit keywords, which needs the splat to
//! distribute the hash. campfire's `Sound#initialize` writes the second
//! shape and died at `require` with an arity error no static check saw.
//!
//! The evidence that a call WAS a splat is the argument count: since
//! Ruby 3.0 a bare Hash is never auto-converted to keywords, so one more
//! positional argument than the callee has positional parameters, into a
//! callee with keywords, can only have been written `**`.

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Diagnostic;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::kwsplat::apply_kwsplat_expansion;

/// Ingest → analyze → the splat expansion → ruby render. Returns the
/// emitted source plus the pass's residue ledger.
fn expand_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = apply_kwsplat_expansion(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn splat_into_required_keywords_expands() {
    // campfire's shape: `Image.new(**image)` into `def initialize(name:,
    // width:, height:)`.
    let (out, diags) = expand_and_emit(
        r#"
class Image
  def initialize(name:, width:, height:)
    @name = name
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
        out.contains("Image.new(name: image[:name], width: image[:width], height: image[:height])"),
        "expected the splat expanded to keywords:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "clean expansion should not ledger: {diags:?}"
    );
}

#[test]
fn splat_into_keyword_rest_is_left_alone() {
    // `def notification(**params)` ingests as ONE trailing positional, so
    // the desugar's positional hash already binds correctly. Rewriting
    // here would invent keyword names the callee never declared.
    let (out, diags) = expand_and_emit(
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
        out.contains("notification(payload)"),
        "**rest callee must keep the positional hash:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "a correct call must not ledger: {diags:?}"
    );
}

/// `def f(*items, **opts); f(payload)` is a valid positional call.
/// Restoring `**payload` would move the Hash from `items` onto `opts`.
#[test]
fn a_positional_rest_beside_keyword_rest_is_not_an_erased_splat() {
    use std::collections::HashMap;
    use std::path::PathBuf;
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
fn optional_keyword_with_a_literal_default_reads_with_the_default_in_hand() {
    // `k: h[:k]` would pass nil for an absent key where Ruby uses the
    // declared default; `h.fetch(:k, 48)` passes what Ruby would. The
    // default is restated at the call site, which a literal survives.
    let (out, diags) = expand_and_emit(
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
        out.contains("Tag.new(name: opts[:name], size: opts.fetch(:size, 48))"),
        "expected the optional keyword read with its default:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "clean expansion should not ledger: {diags:?}"
    );
}

#[test]
fn optional_keyword_with_a_computed_default_is_ledgered_not_expanded() {
    // `Current.user` means something else where the caller stands, so
    // the pass declines and says so.
    let (out, diags) = expand_and_emit(
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
        out.contains("Tag.new(opts)"),
        "a computed default must leave the call alone:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one ledger line: {diags:?}");
    assert!(
        diags[0].message.contains("not a literal"),
        "ledger should name the reason: {}",
        diags[0].message
    );
}

#[test]
fn impure_splat_expression_is_ledgered_not_expanded() {
    // The expression is evaluated once per keyword, so a call would run
    // N times.
    let (out, diags) = expand_and_emit(
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
        out.contains("Image.new(source.dimensions)"),
        "impure receiver must be left alone:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one ledger line: {diags:?}");
}

#[test]
fn literal_keyword_call_is_untouched() {
    // A written-out keyword list already renders correctly; it is
    // `normalize_trailing_kwargs`' business, not this pass's.
    let (out, diags) = expand_and_emit(
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
    assert!(
        diags.is_empty(),
        "a correct call must not ledger: {diags:?}"
    );
}

#[test]
fn positional_params_are_counted_before_the_excess_test() {
    // `def initialize(id, name:, width:)` called `new(id, **opts)` — the
    // splat is the argument BEYOND the declared positional.
    let (out, _) = expand_and_emit(
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
        out.contains("Image.new(id, name: opts[:name], width: opts[:width])"),
        "expected the trailing splat expanded past the positional:\n{out}"
    );
}

#[test]
fn rest_param_declines_because_the_count_proves_nothing() {
    // `*args` absorbs any number of positional arguments, so "one more
    // than declared" is not evidence of a splat.
    let (out, diags) = expand_and_emit(
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
        out.contains("Logger.new(opts)"),
        "a *rest callee must be left alone:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "no evidence means no ledger line either: {diags:?}"
    );
}

#[test]
fn splat_beside_literal_keywords_takes_the_literal_where_it_names_one() {
    // campfire's `Push::Subscription#notification`: `**params` forwarded
    // beside written-out keywords. Ingest made it `params.merge({ badge:
    // … })`; the expansion takes each keyword from the literal when it
    // names one (evaluated once, as Ruby would) and from the bundle
    // otherwise.
    let (out, diags) = expand_and_emit(
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
        out.contains("Notification.new(title: params[:title], body: params[:body], badge: unread, endpoint: endpoint)"),
        "expected the literal's keywords kept and the rest indexed off the bundle:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "clean expansion should not ledger: {diags:?}"
    );
}

#[test]
fn a_receiverless_call_in_a_library_class_expands_with_the_bundle_winning() {
    // `f(k: v, **h)` is `{ k: v }.merge(h)`: the later `**` wins, so the
    // literal is the default the bundle is read against. The call has no
    // receiver, so the callee is the class's own instance method.
    let (out, diags) = expand_and_emit(
        r##"
class Badge
  def svg(**opts)
    render_code(size: 2, **opts)
  end

  def render_code(size:, color: "black")
    "#{size}:#{color}"
  end
end
"##,
    );
    assert!(
        out.contains(
            r#"render_code(size: opts.fetch(:size, 2), color: opts.fetch(:color, "black"))"#
        ),
        "expected the literal read as the bundle's default:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "clean expansion should not ledger: {diags:?}"
    );
}

#[test]
fn a_literal_merged_with_an_impure_bundle_is_ledgered_not_expanded() {
    // `**defaults()` after a literal keyword: the bundle is a call, and
    // expanding would evaluate it once per keyword. The positional Hash
    // stays and the site is ledgered.
    let (out, diags) = expand_and_emit(
        r##"
class Badge
  def svg
    render_code(size: 2, **defaults())
  end

  def defaults
    { color: "red" }
  end

  def render_code(size:, color: "black")
    "#{size}:#{color}"
  end
end
"##,
    );
    assert!(
        out.contains("render_code({ size: 2 }.merge(defaults"),
        "expected the positional bundle left intact:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
}

#[test]
fn a_computed_value_beside_a_later_splat_is_ledgered_not_expanded() {
    // `f(size: compute(), **opts)` evaluates `compute()` even when
    // `opts` has `:size`. Expanding to `opts.fetch(:size, compute())`
    // would skip it. The positional merge stays.
    let (out, diags) = expand_and_emit(
        r##"
class Badge
  def svg(**opts)
    render_code(size: compute(), **opts)
  end

  def compute
    7
  end

  def render_code(size:, color: "black")
    "#{size}:#{color}"
  end
end
"##,
    );
    assert!(
        out.contains("render_code({ size: compute }.merge(opts)"),
        "expected the positional bundle left intact:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one residue entry: {diags:?}");
}

/// A TEST CLASS forwarding `**attributes` from one of its own helpers
/// into another that declares keywords — campfire's
/// `embed_from(**attributes) = attachment_for(**attributes).attachable`
/// against `attachment_for(href:, url:, filename: "Title", caption:
/// "Description")`. The call is receiverless, so the callee is resolved
/// against the class's own helpers; the optional keywords are read with
/// their literal defaults in hand.
#[test]
fn a_test_classs_own_helper_forwarding_a_splat_expands_against_the_class() {
    use std::collections::HashMap;
    use std::path::PathBuf;
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
        src.contains(
            r#"attachment_for(href: attributes[:href], url: attributes[:url], filename: attributes.fetch(:filename, "Title"), caption: attributes.fetch(:caption, "Description"))"#
        ),
        "expected the forwarded splat expanded against the class's own helper:\n{src}"
    );
}

/// `f(**h)` into `def f(**rest)` must keep the splat. Ingest erases
/// `**h` to a positional Hash; Ruby 3 will not auto-convert it, so the
/// call is `wrong number of arguments (given 1, expected 0)` —
/// campfire's `embeds_from(**details)` → `attachments_for(details)`.
#[test]
fn a_splat_into_keyword_rest_is_restored() {
    use std::collections::HashMap;
    use std::path::PathBuf;
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
    // `attachments_for` only *consumes* `**details`, so ingest flattens
    // the def to `details = {}`. Restoring `**details` against that is
    // unexpected keywords. The caller `embeds_from` forwards, so its
    // def stays `**details`.
    assert!(
        src.contains("def attachments_for(details = {})")
            || src.contains("def attachments_for(details={})"),
        "a consuming **rest must emit as a positional Hash:\n{src}"
    );
    assert!(
        src.contains("attachments_for(details)") || src.contains("attachments_for(details,"),
        "the call into the flattened def stays positional:\n{src}"
    );
    assert!(
        !src.contains("attachments_for(**details)"),
        "must not restore ** against a flattened details = {{}}:\n{src}"
    );
    assert!(
        src.contains("def embeds_from(**details)") || src.contains("def embeds_from(**details,"),
        "a forwarding **rest keeps the keyword-rest def:\n{src}"
    );
}

/// `def f(**rest)` whose body itself forwards `**rest` keeps the
/// keyword-rest on the wire. A positional Hash into THAT def must
/// become `**h`; a consuming `**rest` still flattens to `name = {}`.
#[test]
fn a_splat_into_a_forwarding_keyword_rest_is_restored() {
    use std::collections::HashMap;
    use std::path::PathBuf;
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
    assert!(
        src.contains("def consume(**details)") || src.contains("def consume(**details,"),
        "a forwarding **rest keeps the keyword-rest def:\n{src}"
    );
    assert!(
        src.contains("consume(**details)"),
        "the call into that def restores the splat:\n{src}"
    );
    assert!(
        src.contains("def other(details = {})") || src.contains("def other(details={})"),
        "a consuming **rest still flattens:\n{src}"
    );
}
