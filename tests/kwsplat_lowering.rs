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

use roundhouse::analyze::Analyzer;
use roundhouse::diagnostic::Diagnostic;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::kwsplat::apply_kwsplat_expansion;
use roundhouse::App;

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
    assert!(diags.is_empty(), "clean expansion should not ledger: {diags:?}");
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
    assert!(diags.is_empty(), "a correct call must not ledger: {diags:?}");
}

#[test]
fn optional_keyword_is_ledgered_not_expanded() {
    // `k: h[:k]` would pass nil for an absent key where Ruby uses the
    // declared default — a silently different value, so the pass declines
    // and says so.
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
        out.contains("Tag.new(opts)"),
        "optional-keyword callee must be left alone:\n{out}"
    );
    assert_eq!(diags.len(), 1, "expected one ledger line: {diags:?}");
    assert!(
        diags[0].message.contains("optional keyword"),
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
    assert!(diags.is_empty(), "a correct call must not ledger: {diags:?}");
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
    assert!(diags.is_empty(), "no evidence means no ledger line either: {diags:?}");
}
