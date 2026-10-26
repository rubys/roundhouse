//! Constants copied out of a hoisted module or an included helper.
//!
//! A bare name is anchored to the scope it was written in. A name this
//! pass cannot place stays the raise stub.

/// `Box` in an included helper is `::Box`, not a constant the test class defines.
#[test]
fn an_included_helper_constant_stays_absolute() {
    let (_diags, test) = super::box_extend_app(
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  Box = String\n  include OpenHelper\n  test \"lexical constant\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, ::Box.new.ping\n  end\nend\n",
    );
    assert!(
        test.contains("::Box.new"),
        "the helper's Box is absolute:\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "the absolute constant still extends:\n{test}"
    );
    assert!(
        !test.contains("__rh_extend_object_1 = Box.new"),
        "the copied receiver is not the test class constant:\n{test}"
    );
}


/// `LABEL` in the hoisted method is the test class's constant.
#[test]
fn a_hoisted_method_keeps_the_test_class_constant() {
    let (_diags, test) = super::box_extend_app(
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  LABEL = \"from-test\"\n  test \"class constant\" do\n    failing = Module.new { def begin_transport = LABEL }\n    one = Box.new\n    one.extend(failing)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert!(
        test.contains("BoxTest::LABEL"),
        "the hoisted method reads the test constant:\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "the module still extends:\n{test}"
    );
}


/// A name the hoisted method cannot place keeps the test a raise.
#[test]
fn a_hoisted_method_with_an_unresolved_constant_stays_unsupported() {
    super::assert_extend_stub(
        "test_mystery",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"mystery\" do\n    failing = Module.new { def begin_transport = Mystery }\n    one = Box.new\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}


/// The helper default `Note` is the module's constant, not the test class's.
#[test]
fn an_included_helper_default_stays_on_the_module() {
    let mut app = super::app_with(vec![
        ("db/schema.rb", super::SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  Note = \"helper\"\n  def opened(tag = Note)\n    Box.new.tap do |box|\n      box.note = tag\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  Note = \"test\"\n  include OpenHelper\n  test \"default constant\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        diags.iter().all(|d| !d.message.contains("Object#extend")),
        "the default still extends:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("OpenHelper::Note"),
        "the default is the helper constant:\n{test}"
    );
    assert!(
        !test.contains(".note = Note"),
        "the default is not the test class constant:\n{test}"
    );
}


/// `Box::NOTE` in the helper is the top-level class, not `BoxTest::Box`.
#[test]
fn an_included_helper_nested_constant_stays_absolute() {
    let mut app = super::app_with(vec![
        ("db/schema.rb", super::SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  NOTE = \"top\"\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = Box::NOTE\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  Box = String\n  include OpenHelper\n  test \"nested constant\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, ::Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains("::Box::NOTE"),
        "the nested constant is absolute:\n{test}"
    );
    assert!(
        test.contains("::Box.new"),
        "the receiver class is absolute:\n{test}"
    );
    assert!(
        diags.iter().all(|d| !d.message.contains("Object#extend")),
        "the nested constant still extends:\n{diags:?}\n{test}"
    );
}


/// `Note` from an included ancestor is not `::Note`. The test stays a raise.
#[test]
fn an_included_helper_ancestor_constant_stays_unsupported() {
    let mut app = super::app_with(vec![
        ("db/schema.rb", super::SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module Base\n  Note = \"base\"\nend\nmodule OpenHelper\n  include Base\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = Note\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"ancestor constant\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        diags.iter().any(|d| d.message.contains("Object#extend")),
        "an ancestor constant is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_ancestor_constant\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
    assert!(
        !test.contains("::Note"),
        "the ancestor constant is not emitted as absolute:\n{test}"
    );
}


