//! Type-directed blank-predicate lowering (`lower::apply_blank_lowering`).
//!
//! Shape tests over the grounding table: string/collection receivers
//! ground through `empty?`, nilable receivers pick up the nil guard,
//! never-blank scalars fold, classes with their own predicate keep
//! normal dispatch, and ungroundable receivers survive verbatim with a
//! `blank_unlowered` residue diagnostic.

use roundhouse::App;
use roundhouse::analyze::{Analyzer, Diagnostic};
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;
use roundhouse::lower::apply_blank_lowering;

fn lower_and_emit(source: &str) -> (String, Vec<Diagnostic>) {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    let diags = apply_blank_lowering(&mut app);
    let out = emit_library(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    (out, diags)
}

#[test]
fn string_present_grounds_to_not_empty() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    s = "value"
    return "yes" if s.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "site should be grounded:\n{out}");
    assert!(out.contains("empty?"), "expected empty?-based form:\n{out}");
    assert!(
        diags.is_empty(),
        "typed receiver should not produce residue: {diags:?}"
    );
}

#[test]
fn array_blank_grounds_to_empty() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    a = [1, 2]
    return "none" if a.blank?
    "some"
  end
end
"#,
    );
    assert!(!out.contains("blank?"), "site should be grounded:\n{out}");
    // A collection is `empty?`, not the string form. The nil guard lives
    // on the string arm only; wrapping an array as `(a || "")` was the
    // old Ruby-only `empty?` rewrite.
    assert!(
        out.contains(".empty?"),
        "expected empty?-based form:\n{out}"
    );
    assert!(
        !out.contains("strip"),
        "arrays are not whitespace-blank:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn nilable_string_present_gets_nil_guard() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(flag)
    s = flag ? "value" : nil
    return "yes" if s.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "site should be grounded:\n{out}");
    assert!(
        out.contains("nil?"),
        "nilable receiver needs the nil guard:\n{out}"
    );
    assert!(out.contains("empty?"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn integer_present_folds_to_true() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check
    n = 3
    return "yes" if n.present?
    "no"
  end
end
"#,
    );
    assert!(
        !out.contains("present?"),
        "never-blank scalar should fold:\n{out}"
    );
    assert!(!out.contains("empty?"), "no empty? for scalars:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

/// An ivar read keeps its nil guard even when its stamp is non-nil:
/// an unassigned ivar reads nil, and campfire's detached
/// `ApplicationController.new.perform_caching` raised on
/// `!@response_cache_version.empty?`.
#[test]
fn ivar_present_keeps_the_nil_guard_under_a_non_nil_stamp() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def initialize
    @parts = [1, 2]
    @count = 3
  end

  def check
    return "yes" if @parts.present? && @count.present?
    "no"
  end
end
"#,
    );
    assert!(!out.contains("present?"), "site should be grounded:\n{out}");
    assert!(out.contains("@parts.nil?"), "collection ivar needs the nil guard:\n{out}");
    assert!(out.contains("@count.nil?"), "scalar ivar must not fold to true:\n{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn string_presence_becomes_conditional() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def label(name)
    s = name.to_s
    s.presence || "anonymous"
  end
end
"#,
    );
    assert!(
        !out.contains("presence"),
        "presence should be grounded:\n{out}"
    );
    assert!(out.contains("empty?"), "{out}");
    assert!(diags.is_empty(), "{diags:?}");
}

/// No static type to ground on, so the value goes to the runtime
/// predicate rather than staying a dynamic `Object#present?` send that
/// only CRuby's core_ext reopen could serve.
#[test]
fn untyped_receiver_routes_to_the_runtime_predicate() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(thing)
    return "yes" if thing.present?
    "no"
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport.present?(thing)"),
        "untyped receiver should become a runtime predicate call:\n{out}"
    );
    assert!(
        !out.contains("thing.present?"),
        "the dynamic send must not survive:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// The runtime predicate reads its argument once, so a receiver with
/// effects grounds here where the type-directed forms must refuse it.
#[test]
fn impure_untyped_receiver_grounds_because_the_value_is_an_argument() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(rec)
    return "yes" if rec.save!.present?
    "no"
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport.present?(rec.save!)"),
        "impure receiver should be evaluated once as an argument:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// An app class that defines its own predicate keeps normal dispatch
/// even through a nilable union — the runtime helper knows nothing
/// about the app's definition and must not swallow it.
#[test]
fn nilable_own_predicate_class_still_refuses() {
    let (_out, diags) = lower_and_emit(
        r#"
class Bag
  def present?
    true
  end
end

class Util
  def check(flag)
    b = flag ? Bag.new : nil
    return "yes" if b.present?
    "no"
  end
end
"#,
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code(), "blank_unlowered");
}

/// `def label(name = nil)` called only with an untyped argument has no
/// call-site evidence (the argument's own type is still an inference
/// var, filtered out before it reaches `param_ty_with_default`), so the
/// `nil` default alone must not pin the parameter to `Ty::Nil` — that
/// would fold `name.presence` away to a bare `nil` silently, as if the
/// caller's value could never matter.
#[test]
fn nil_default_with_no_call_site_evidence_keeps_the_runtime_predicate() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def label(name = nil)
    name.presence || "none"
  end

  def call(thing)
    label(thing)
  end
end
"#,
    );
    assert!(
        out.contains("ActiveSupport.presence(name)"),
        "a nil default with no call-site evidence must not fold to a bare nil:\n{out}"
    );
    assert!(
        !out.contains("nil || \"none\""),
        "presence must not be grounded to nil:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn own_predicate_class_keeps_dispatch() {
    let (out, diags) = lower_and_emit(
        r#"
class Wrapper
  def blank?
    false
  end
end

class Util
  def check
    w = Wrapper.new
    return "yes" if w.blank?
    "no"
  end
end
"#,
    );
    assert!(
        out.contains("w.blank?"),
        "class with its own predicate keeps normal dispatch:\n{out}"
    );
    assert!(
        diags.is_empty(),
        "own-predicate dispatch is not residue: {diags:?}"
    );
}

#[test]
fn indexed_read_receiver_is_reevaluable() {
    let (out, diags) = lower_and_emit(
        r#"
class Util
  def check(opts)
    h = { "a" => "x" }
    return "yes" if h["a"].present?
    "no"
  end
end
"#,
    );
    assert!(
        !out.contains("present?"),
        "hash-value read should ground:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// `params[:attachment].blank?` — typed `String?`, and for a form field
/// that is what it is; campfire's bot endpoint receives an
/// UploadedFile there, and the String grounding's `strip` on one was a
/// NoMethodError. A params read is handed to the runtime predicate,
/// which branches on the value the way Rails' `Object#blank?` does.
#[test]
fn a_params_read_is_grounded_at_runtime_whatever_its_type() {
    let (out, diags) = lower_and_emit(
        r#"
class Guard
  def check(params)
    @params = params
    return "no" if @params["attachment"].blank?
    return "no" if params[:body].blank?
    "yes"
  end
end
"#,
    );
    assert_eq!(
        out.matches("ActiveSupport.blank?(").count(),
        2,
        "both params reads should reach the runtime predicate:\n{out}"
    );
    assert!(
        !out.contains("strip"),
        "no String-shaped grounding on a params read:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// `{ host:, protocol: }.compact_blank` — ActiveSupport rejects on the
/// VALUE. campfire's `SetCurrentRequest#default_url_options` is this
/// Hash of nilable Strings; leaving it as a send is an AOT refusal.
#[test]
fn hash_compact_blank_rejects_on_the_value() {
    let (out, diags) = lower_and_emit(
        r#"
class Url
  def options(host, protocol)
    { host: host, protocol: protocol }.compact_blank
  end
end
"#,
    );
    assert!(
        !out.contains("compact_blank"),
        "Hash compact_blank must ground:\n{out}"
    );
    assert!(
        out.contains("reject"),
        "expected a reject over the values:\n{out}"
    );
    assert!(
        out.contains("_k") && out.contains("__cb"),
        "Hash reject binds the key and tests the value:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// `[ String | Content ].compact_blank` must not reject through
/// `ActiveSupport.blank?` — that helper never calls `Content#blank?`.
#[test]
fn union_with_own_predicate_compact_blank_stays_dynamic() {
    let (out, diags) = lower_and_emit(
        r#"
class Content
  def blank?
    false
  end
end

class Titles
  def of(flag)
    item = flag ? "x" : Content.new
    [ item ].compact_blank
  end
end
"#,
    );
    assert!(
        out.contains("compact_blank"),
        "own-predicate union must keep the send:\n{out}"
    );
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code(), "blank_unlowered");
}

/// An Array whose element is still an inference var used to file
/// residue and keep the send. The reject body now calls
/// `ActiveSupport.blank?`, the same runtime predicate an untyped
/// `blank?` already takes — campfire's helper
/// `[ author.name, author.bio ].compact_blank` is this shape.
#[test]
fn untyped_array_compact_blank_uses_the_runtime_predicate() {
    let (out, diags) = lower_and_emit(
        r#"
class Titles
  def of(author)
    [ author.name, author.bio ].compact_blank.join(" – ")
  end
end
"#,
    );
    assert!(
        !out.contains("compact_blank"),
        "must not keep the send:\n{out}"
    );
    assert!(
        out.contains("reject") && out.contains("ActiveSupport.blank?"),
        "expected a reject through the runtime predicate:\n{out}"
    );
    assert!(diags.is_empty(), "{diags:?}");
}

/// The nil guard is in the lowered IR, before any emitter. `emit_expr`
/// prints that IR and does not run a Ruby pre-emit rewrite, so these
/// tests fail while `(r || "")` still lives only in an emitter.
fn is_string_blank_test(expr: &roundhouse::Expr) -> bool {
    let roundhouse::ExprNode::Send { recv: Some(strip_recv), method, args, .. } = &*expr.node
    else {
        return false;
    };
    if method.as_str() != "empty?" || !args.is_empty() {
        return false;
    }
    let roundhouse::ExprNode::Send { recv: Some(guard), method, args, .. } = &*strip_recv.node
    else {
        return false;
    };
    if method.as_str() != "strip" || !args.is_empty() {
        return false;
    }
    let roundhouse::ExprNode::BoolOp {
        op: roundhouse::expr::BoolOpKind::Or,
        right,
        ..
    } = &*guard.node
    else {
        return false;
    };
    matches!(
        &*right.node,
        roundhouse::ExprNode::Lit { value: roundhouse::Literal::Str { value } } if value.is_empty()
    )
}

fn count_string_blank_tests(expr: &roundhouse::Expr) -> usize {
    let mut n = 0;
    fn rec(e: &roundhouse::Expr, n: &mut usize) {
        if is_string_blank_test(e) {
            *n += 1;
        }
        e.node.for_each_child(&mut |c| rec(c, n));
    }
    rec(expr, &mut n);
    n
}

fn lowered_method(source: &str, method: &str) -> roundhouse::Expr {
    let classes = ingest_library_classes(source.as_bytes(), "test.rb").expect("ingest test source");
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    Analyzer::new(&app).analyze(&mut app);
    apply_blank_lowering(&mut app);
    app.library_classes
        .iter()
        .flat_map(|lc| lc.methods.iter())
        .find(|m| m.name.as_str() == method)
        .unwrap_or_else(|| panic!("missing {method}"))
        .body
        .clone()
}

#[test]
fn whitespace_string_blank_is_the_guarded_strip_form_in_ir() {
    let body = lowered_method(
        r#"
class Util
  def check
    s = "   "
    s.blank?
  end
end
"#,
        "check",
    );
    let printed = roundhouse::emit::ruby::emit_expr(&body);
    assert_eq!(
        count_string_blank_tests(&body),
        1,
        "whitespace blank? must already be (s || \"\").strip.empty? in the IR:\n{printed}"
    );
    assert!(!printed.contains("blank?"), "{printed}");
    assert!(printed.contains(".strip.empty?"), "{printed}");
}

#[test]
fn nil_string_blank_carries_the_same_guard_in_ir() {
    let body = lowered_method(
        r#"
class Util
  def check(flag)
    s = flag ? "value" : nil
    s.blank?
  end
end
"#,
        "check",
    );
    let printed = roundhouse::emit::ruby::emit_expr(&body);
    assert_eq!(
        count_string_blank_tests(&body),
        1,
        "a nilable string must carry (s || \"\").strip.empty? before emit:\n{printed}"
    );
    assert!(printed.contains("nil?"), "typed nil still short-circuits:\n{printed}");
    assert!(!printed.contains("blank?"), "{printed}");
}

fn is_nil_guarded_empty(expr: &roundhouse::Expr) -> bool {
    let roundhouse::ExprNode::Send { recv: Some(guard), method, args, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "empty?" || !args.is_empty() {
        return false;
    }
    let roundhouse::ExprNode::BoolOp {
        op: roundhouse::expr::BoolOpKind::Or,
        right,
        ..
    } = &*guard.node
    else {
        return false;
    };
    matches!(
        &*right.node,
        roundhouse::ExprNode::Lit { value: roundhouse::Literal::Str { value } } if value.is_empty()
    )
}

fn count_nil_guarded_empties(expr: &roundhouse::Expr) -> usize {
    let mut n = 0;
    fn rec(e: &roundhouse::Expr, n: &mut usize) {
        if is_nil_guarded_empty(e) {
            *n += 1;
        }
        e.node.for_each_child(&mut |c| rec(c, n));
    }
    rec(expr, &mut n);
    n
}

#[test]
fn view_blank_and_present_guard_nil_without_strip() {
    let mut tree = std::collections::HashMap::new();
    tree.insert(
        std::path::PathBuf::from("app/views/articles/index.html.erb"),
        b"<% if notice.blank? %>blank<% end %>\n<% if notice.present? %>here<% end %>\n<% if @posts.blank? %>none<% end %>\n<% if @articles.any? %>rows<% end %>\n"
            .to_vec(),
    );
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest view");
    let view = app.views.first().expect("view");
    let lc = roundhouse::lower::lower_view_to_library_class(view, &app);
    let body = &lc.methods.first().expect("view method").body;
    let printed = roundhouse::emit::ruby::emit_expr(body);
    assert_eq!(
        count_nil_guarded_empties(body),
        3,
        "notice blank?, notice present?, and @posts.blank? each carry (recv || \"\").empty? in the IR:\n{printed}"
    );
    assert_eq!(
        count_string_blank_tests(body),
        0,
        "an untyped view receiver must not strip; an array has no strip:\n{printed}"
    );
    assert!(
        printed.contains("(notice || \"\").empty?") && printed.contains("(posts || \"\").empty?"),
        "view blank?/present? print the IR guard:\n{printed}"
    );
    assert!(!printed.contains("notice.blank?") && !printed.contains("notice.present?"), "{printed}");
    assert!(
        printed.contains("articles.empty?") && !printed.contains("(articles ||"),
        "any? stays a collection empty? with no nil-guard wrap:\n{printed}"
    );
}
