//! Instance `Object#extend` lowering.
//!
//! A traced `extend` of one `.new` is hoisted onto that object. Every
//! other shape stays the raise stub, so a Spinel test file still links.
//! The emitted program is pinned in `tests/emit_and_run/instance_extend.rs`.

use roundhouse::ingest::ingest_app_from_tree;

fn app_with(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

#[path = "instance_extend_lowering/constants.rs"]
mod constants;

#[test]
fn an_instance_extend_stubs_the_test_that_reaches_for_it() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("test/fixtures/rooms.yml", "pets:\n  name: Pets\n"),
        (
            "test/models/room_test.rb",
            "require \"test_helper\"\nclass RoomTest < ActiveSupport::TestCase\n  test \"dynamic\" do\n    room = rooms(:pets).tap { |r| r.extend ActionText::Attachable }\n    assert_equal \"Pets\", room.name\n  end\n\n  test \"static\" do\n    assert_equal \"Pets\", rooms(:pets).name\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let report = diags
        .iter()
        .find(|d| d.message.contains("Object#extend"))
        .expect("the extend is reported");
    assert_eq!(
        report.severity,
        roundhouse::diagnostic::Severity::Warning,
        "a warning, so the strict archive build still writes the tree"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    // The WHOLE test is the stub — what followed the extend was
    // written against the object it produced — and the test beside
    // it is untouched.
    assert!(
        test.contains("  def test_dynamic\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the dynamic test should be the raise:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "no extend survives into the emit:\n{test}"
    );
    assert!(
        test.contains("  def test_static\n    raise \"assert_equal failed\" if \"Pets\" != RoomsFixtures.pets.name\n  end\n"),
        "the static test keeps its body:\n{test}"
    );
}

/// Campfire's TLS-failure test: `Module.new { private def ... }` mixed
/// into one live object, whose receiver is a helper that returns
/// `HTTP.new(...).tap { ... }`. The module becomes a real module, the
/// receiver a single `.new` local, and a sibling that never extends
/// keeps the class method.
#[test]
fn an_instance_extend_of_one_new_keeps_the_sibling() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = tag\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"one object\" do\n    failing = Module.new { private def begin_transport(*) = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n    assert_equal :open, opened(\"b\").ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\n  test \"direct\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal :closed, one.extend(failing).ping\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let extend_warns: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("Object#extend"))
        .collect();
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        extend_warns.is_empty(),
        "a traced extend is supported:\n{extend_warns:?}\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "the extend stays in the emit:\n{test}"
    );
    assert!(
        !test.contains(".extend(RhInstanceExtend1).ping")
            && !test.contains(".extend(RhInstanceExtend2).ping"),
        "spinel flips the class at the extend statement, so the value is the local:\n{test}"
    );
    assert!(
        test.contains("module RhInstanceExtend"),
        "the anonymous module is a real module:\n{test}"
    );
    assert!(
        test.contains("def begin_transport"),
        "the nested def was hoisted:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
    assert!(
        !test.contains("Object#extend not supported"),
        "no stub remains:\n{test}"
    );
}

/// An `extend` under `if` is not pulled out in front of the condition.
/// The test that writes it is the raise; the test beside it is not.
#[test]
fn an_extend_under_a_condition_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"conditional\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    if false\n      one.extend(failing).ping\n    end\n    assert_equal :open, one.ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diags.iter().any(|d| d.message.contains("Object#extend")),
        "the conditional extend is reported"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains("def test_conditional\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the conditional test is the raise:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "the extend is not hoisted out of the if:\n{test}"
    );
}

/// A block and the statement that contains it share one counter. The
/// object the block extends is not the local the statement pings.
#[test]
fn a_block_and_the_statement_around_it_extend_different_objects() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"block\" do\n    closed = Module.new { def begin_transport = :closed }\n    other = Module.new { def begin_transport = :other }\n    assert_equal :closed, Box.new.extend(closed).tap { Box.new.extend(other) }.ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diags.iter().all(|d| !d.message.contains("Object#extend")),
        "both extends are traced:\n{diags:?}"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains("__rh_extend_object_1") && test.contains("__rh_extend_object_2"),
        "the block and the statement must not share a local:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
}

/// `foo(bar(), obj.extend(mod))` evaluates `bar()` first. Pulling the
/// extend out in front of the call would reverse that, so the test is
/// the raise.
#[test]
fn an_extend_after_another_call_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"after a call\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal one.ping, one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diags.iter().any(|d| d.message.contains("Object#extend")),
        "the extend after a call is reported"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains("def test_after_a_call\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "the extend is not pulled ahead of the call:\n{test}"
    );
}

/// Spinel transplants one module. `extend(Mod, other)` is left out of
/// the file, or the extra argument refuses the link.
#[test]
fn an_extend_with_two_arguments_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"two arguments\" do\n    failing = Module.new { def begin_transport = :closed }\n    extra = Module.new { def begin_transport = :other }\n    one = Box.new\n    one.extend(failing, extra).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        diags.iter().any(|d| d.message.contains("Object#extend")),
        "a second argument is reported"
    );
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains("def test_two_arguments\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the two-argument test is the raise:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "the two-argument extend does not stay in the file:\n{test}"
    );
}

fn box_extend_app(test_body: &str) -> (Vec<roundhouse::diagnostic::Diagnostic>, String) {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note, :n\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\n  def delta\n    1\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = tag\n    end\n  end\nend\n",
        ),
        ("test/lib/box_test.rb", test_body),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("box_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    (diags, test)
}

fn assert_extend_stub(test_name: &str, source: &str) {
    let (diags, test) = box_extend_app(source);
    assert!(
        diags.iter().any(|d| d.message.contains("Object#extend")),
        "{test_name} is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains(&format!(
            "def {test_name}\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"
        )),
        "{test_name} is the raise:\n{test}"
    );
    assert!(
        test.contains("def test_sibling"),
        "the sibling test stays:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "no extend stays in the file:\n{test}"
    );
}

/// A write under `if` is not the one definition of the module or the
/// receiver. Hoisting it would mix the module in when the branch did
/// not run.
#[test]
fn an_extend_whose_write_is_conditional_stays_unsupported() {
    assert_extend_stub(
        "test_conditional_module",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"conditional module\" do\n    failing = Module.new { def begin_transport = :closed } if false\n    one = Box.new\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert_extend_stub(
        "test_conditional_receiver",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"conditional receiver\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new if false\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `opened(side()).extend` has to run `side` before `Box.new`. The
/// helper body would run it at the setter, so the test stays a raise.
#[test]
fn an_extend_through_a_helper_call_argument_stays_unsupported() {
    assert_extend_stub(
        "test_helper_argument",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"helper argument\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\".upcase).extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// A read of the module local outside the extend still needs the
/// assignment. Deleting it would raise `NameError`.
#[test]
fn a_module_local_read_outside_the_extend_stays_unsupported() {
    assert_extend_stub(
        "test_other_read",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"other read\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    one.extend(failing).ping\n    assert_kind_of Module, failing if false\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The caller's argument is not the tap parameter. `opened(box)` must
/// set `note` from that local, not from the new object.
#[test]
fn a_helper_argument_keeps_its_name_when_the_tap_parameter_matches() {
    let (_diags, test) = box_extend_app(
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"named argument\" do\n    failing = Module.new { def begin_transport = :closed }\n    box = \"kept\"\n    assert_equal :closed, opened(box).extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert!(
        test.contains(".note = box"),
        "the setter keeps the caller's local:\n{test}"
    );
    assert!(
        !test.contains(".note = __rh_extend_object"),
        "the setter does not receive the new object:\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "this shape still extends:\n{test}"
    );
}

/// `one.n += one.extend(mod).delta` reads `n` before the extend.
#[test]
fn an_extend_on_the_right_of_op_assign_stays_unsupported() {
    assert_extend_stub(
        "test_op_assign",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"op assign\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    one.n += one.extend(failing).delta\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `self.extended` runs at the call. Leaving the extend in the file
/// makes Spinel refuse the link.
#[test]
fn an_extend_hook_stays_unsupported() {
    assert_extend_stub(
        "test_extended_hook",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"extended hook\" do\n    failing = Module.new do\n      def self.extended(base) = base\n      def begin_transport = :closed\n    end\n    Box.new.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// A class that defines `extend` returns that method's value. The call
/// is not `Object#extend`.
#[test]
fn an_extend_the_class_defines_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  def extend(_mod)\n    :custom\n  end\n  def ping\n    :open\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"own extend\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal :custom, one.extend(failing)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "a user-defined extend is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_own_extend\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "the call is not rewritten into Object#extend:\n{test}"
    );
}

/// `(side(); Box.new.extend(mod))` is one expression. `side` runs
/// first. Pulling the extend out in front of the parentheses would
/// reverse that, so the test is the raise.
#[test]
fn an_extend_after_a_paren_statement_stays_unsupported() {
    assert_extend_stub(
        "test_paren_statement",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"paren statement\" do\n    failing = Module.new { def begin_transport = :closed }\n    marker = \"a\"\n    assert_equal :closed, (marker.upcase; Box.new.extend(failing).ping)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The block parameter is not the helper parameter, even when the
/// names match. `map { |tag| tag }` still reads the block's `tag`.
#[test]
fn a_helper_block_parameter_keeps_its_name() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = [tag].map { |tag| tag }.first\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"shadowed tag\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"kept\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "the shadowed block still extends:\n{diags:?}\n{test}"
    );
    assert!(test.contains("|tag|"), "the block parameter stays:\n{test}");
    assert!(
        !test.contains("|tag| \"kept\"") && !test.contains("|tag| kept"),
        "the block body is not the caller argument:\n{test}"
    );
    assert!(
        test.contains("\"kept\""),
        "the array still reads the caller argument:\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "this shape still extends:\n{test}"
    );
}

/// `one.extend(other.extend(mod))` leaves the inner call in value
/// position. Spinel does not yield the receiver from that expression.
#[test]
fn a_nested_extend_stays_unsupported() {
    assert_extend_stub(
        "test_nested_extend",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"nested extend\" do\n    failing = Module.new { def begin_transport = :closed }\n    other = Box.new\n    one = Box.new\n    assert_equal :closed, one.extend(other.extend(failing)).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// Binding `a` to the caller's local `b` must not then rewrite that
/// local when parameter `b` is substituted.
#[test]
fn a_helper_argument_is_not_rewritten_by_a_later_parameter() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(a, b)\n    Box.new.tap do |box|\n      box.note = a\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"two parameters\" do\n    failing = Module.new { def begin_transport = :closed }\n    b = \"kept\"\n    assert_equal :closed, opened(b, \"other\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "two parameters still extend:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains(".note = b"),
        "the setter reads the caller's local:\n{test}"
    );
    assert!(
        !test.contains(".note = \"other\""),
        "the second parameter does not replace it:\n{test}"
    );
}

/// `include` order is Ruby's: the later module's method is the one called.
#[test]
fn the_later_include_supplies_the_helper() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module FirstHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = \"from-a\"\n    end\n  end\nend\nmodule SecondHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = \"from-b\"\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include FirstHelper\n  include SecondHelper\n  test \"later include\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "the later helper still extends:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("__rh_extend_object_1.note = \"from-b\""),
        "the later include is the helper:\n{test}"
    );
    assert!(
        !test.contains("__rh_extend_object_1.note = \"from-a\""),
        "the earlier include is not the setter:\n{test}"
    );
}

/// An `extend` brought in by `include` is that method, not `Object#extend`.
#[test]
fn an_extend_from_an_included_module_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "module CustomExtend\n  def extend(_mod)\n    :custom\n  end\nend\nclass Box\n  include CustomExtend\n  def ping\n    :open\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"included extend\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal :custom, one.extend(failing)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "an included extend is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_included_extend\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
    assert!(
        !test.contains(".extend"),
        "the call is not rewritten into Object#extend:\n{test}"
    );
}

/// `|failing|` is not the module local. Hoisting would extend the
/// hoisted module from inside the block.
#[test]
fn a_block_parameter_is_not_the_module_local() {
    assert_extend_stub(
        "test_shadowed_module",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"shadowed module\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    [failing].each { |failing| one.extend(failing) }\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The assignment is the argument. Replacing it with nil would change
/// what `identity` receives.
#[test]
fn an_assignment_used_as_a_value_stays_unsupported() {
    assert_extend_stub(
        "test_assign_value",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"assign value\" do\n    def identity(mod) = mod\n    one = Box.new\n    identity(failing = Module.new { def begin_transport = :closed })\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `map` uses the block's value. An `extend` expression does not answer
/// the object, so the block answers the local.
#[test]
fn a_block_extend_answers_the_local() {
    let (_diags, test) = box_extend_app(
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"map\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal [one], [0].map { one.extend(failing) }\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert!(
        test.contains(".extend("),
        "the extend stays a statement:\n{test}"
    );
    assert!(
        test.contains("one.extend(RhInstanceExtend1)\n    one }"),
        "the block answers the local:\n{test}"
    );
}

/// A proc called later does not see the write that exists at the
/// `extend` line. The test stays the raise.
#[test]
fn an_extend_inside_a_proc_stays_unsupported() {
    assert_extend_stub(
        "test_proc",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"proc\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    cb = -> { one.extend(failing) }\n    one = Box.new\n    cb.call\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `lambda`, `proc`, and `Proc.new` run when called, same as `->`.
#[test]
fn an_extend_inside_lambda_proc_or_proc_new_stays_unsupported() {
    for (name, maker) in [
        ("test_lambda", "lambda"),
        ("test_proc_method", "proc"),
        ("test_proc_new", "Proc.new"),
    ] {
        assert_extend_stub(
            name,
            &format!(
                "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"{label}\" do\n    failing = Module.new {{ def begin_transport = :closed }}\n    one = Box.new\n    cb = {maker} {{ one.extend(failing) }}\n    one = Box.new\n    cb.call\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
                label = name.trim_start_matches("test_"),
            ),
        );
    }
}

/// `in one` binds `one` to the scrutinee. The earlier `.new` is not the object MRI extends.
#[test]
fn a_pattern_bind_before_extend_stays_unsupported() {
    assert_extend_stub(
        "test_case_in",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"case in\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    other = Box.new\n    case other\n    in one\n    end\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert_extend_stub(
        "test_match_required",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"match required\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    other = Box.new\n    other => one\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The helper's `yield` is the caller's block. Copying it into the test method runs the test block.
#[test]
fn a_helper_that_yields_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = yield\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"yields\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\") { \"from-block\" }.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "a yielding helper is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_yields\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
}

/// A block passed to the helper is not the tap block. Inlining would drop it.
#[test]
fn a_helper_call_with_a_block_stays_unsupported() {
    assert_extend_stub(
        "test_helper_block",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"helper block\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\") { |box| box }.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The nested `def` keeps the keyword it was parsed with.
#[test]
fn a_hoisted_def_keeps_its_keyword() {
    let (_diags, test) = box_extend_app(
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"keyword\" do\n    failing = Module.new { def ping(note: \"x\") = note }\n    one = Box.new\n    one.extend(failing)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
    assert!(
        test.contains("note:"),
        "the keyword is not flattened:\n{test}"
    );
    assert!(
        test.contains(".extend("),
        "the module still extends:\n{test}"
    );
}

/// The module assignment that follows the extend is not the value it reads.
#[test]
fn a_module_assigned_after_extend_stays_unsupported() {
    assert_extend_stub(
        "test_module_after",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"module after\" do\n    one = Box.new\n    one.extend(failing).ping\n    failing = Module.new { def begin_transport = :closed }\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `|one|` is the object MRI extends, not the `.new` before the block.
#[test]
fn a_block_parameter_before_extend_stays_unsupported() {
    assert_extend_stub(
        "test_block_param",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"block param\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    other = Box.new\n    [other].each { |one| one.extend(failing) }\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `rescue => one` binds the exception. The earlier `.new` is not that object.
#[test]
fn a_rescue_binding_before_extend_stays_unsupported() {
    assert_extend_stub(
        "test_rescue_binding",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"rescue binding\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    begin\n      raise \"x\"\n    rescue => one\n    end\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `Kernel.lambda`, `Kernel.proc`, and `Kernel::Proc.new` run when called.
#[test]
fn an_extend_inside_kernel_lambda_stays_unsupported() {
    for (name, maker) in [
        ("test_kernel_lambda", "Kernel.lambda"),
        ("test_kernel_proc", "Kernel.proc"),
        ("test_kernel_proc_new", "Kernel::Proc.new"),
    ] {
        assert_extend_stub(
            name,
            &format!(
                "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"{label}\" do\n    failing = Module.new {{ def begin_transport = :closed }}\n    one = Box.new\n    cb = {maker} {{ one.extend(failing) }}\n    one = Box.new\n    cb.call\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
                label = name.trim_start_matches("test_"),
            ),
        );
    }
}

/// A block that does not run before the call returns hides the extend.
/// Spinel only links a straight-line extend statement.
#[test]
fn an_extend_inside_a_block_that_runs_later_stays_unsupported() {
    for (name, maker) in [
        ("test_define_method", "define_method(:go)"),
        ("test_thread_new", "Thread.new"),
        ("test_module_eval", "Module.new.module_eval"),
        ("test_fiber_new", "Fiber.new"),
        ("test_enumerator_new", "Enumerator.new"),
        ("test_ractor_new", "Ractor.new"),
        ("test_at_exit", "at_exit"),
        ("test_lazy_map", "[0].lazy.map"),
    ] {
        assert_extend_stub(
            name,
            &format!(
                "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"{label}\" do\n    failing = Module.new {{ def begin_transport = :closed }}\n    one = Box.new\n    {maker} {{ one.extend(failing) }}\n    one = Box.new\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
                label = name.trim_start_matches("test_"),
            ),
        );
    }
}

/// A call that returns `Enumerator::Lazy` does not run `map` yet.
#[test]
fn an_extend_inside_a_lazy_returning_call_stays_unsupported() {
    assert_extend_stub(
        "test_lazy_call",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"lazy call\" do\n    def zeros; [0].lazy; end\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    zeros.map { one.extend(failing) }\n    one = Box.new\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// `super` in the helper calls the helper's ancestor, not the test class's.
#[test]
fn a_helper_that_calls_super_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "class Box\n  attr_accessor :note\n  def begin_transport\n    :open\n  end\n  def ping\n    begin_transport\n  end\nend\n",
        ),
        (
            "test/test_helpers/open_helper.rb",
            "module OpenHelper\n  def opened(tag)\n    Box.new.tap do |box|\n      box.note = super\n    end\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  include OpenHelper\n  test \"super\" do\n    failing = Module.new { def begin_transport = :closed }\n    assert_equal :closed, opened(\"a\").extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "a helper that calls super is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_super\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
    assert!(
        !test.contains("__rh_extend_object_"),
        "super is not copied into an inlined receiver:\n{test}"
    );
}

/// `prepend` puts the module's `extend` ahead of `Object#extend`.
#[test]
fn an_extend_from_a_prepended_module_stays_unsupported() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/box.rb",
            "module CustomExtend\n  def extend(_mod)\n    :custom\n  end\nend\nclass Box\n  prepend CustomExtend\n  def ping\n    :open\n  end\nend\n",
        ),
        (
            "test/lib/box_test.rb",
            "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"prepended extend\" do\n    failing = Module.new { def begin_transport = :closed }\n    one = Box.new\n    assert_equal :custom, one.extend(failing)\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
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
        "a prepended extend is reported:\n{diags:?}\n{test}"
    );
    assert!(
        test.contains("def test_prepended_extend\n    (raise \"roundhouse: Object#extend not supported (all targets)\")\n  end\n"),
        "the test is the raise:\n{test}"
    );
}

/// The assignment that follows the extend is not the value it reads.
#[test]
fn an_extend_before_its_assignment_stays_unsupported() {
    assert_extend_stub(
        "test_assign_after",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"assign after\" do\n    failing = Module.new { def begin_transport = :closed }\n    one.extend(failing).ping\n    one = Box.new\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

