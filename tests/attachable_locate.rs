//! `ActionText::Attachment#attachable` dereferences an sgid through
//! `ActionText::Attachable.locate`, which is GENERATED per app: one
//! `when "<Model>"` per model that mixes `ActionText::Attachable` in,
//! written into the emitted tree's `global_id_locator.rb` between its
//! markers. And `obj.extend Mod` on an instance — what campfire's test
//! does to make a `Room` attachable for one assertion — is a raise stub
//! naming the construct, so the file it sits in still links on spinel.

use roundhouse::ingest::ingest_app_from_tree;

fn app_with(files: Vec<(&str, &str)>) -> roundhouse::App {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\n  create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

#[test]
fn the_locator_is_a_case_over_the_attachable_models() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        // campfire's layout: the marker one level down, in a module
        // nested under the model.
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include Mentionable\nend\n",
        ),
        (
            "app/models/user/mentionable.rb",
            "module User::Mentionable\n  include ActionText::Attachable\n\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    let locator = files
        .iter()
        .find(|(p, _)| p.ends_with("global_id_locator.rb"))
        .map(|(_, c)| c.clone())
        .expect("global_id_locator.rb in the tree");
    // The include is resolved through the concern; Room, which does
    // not mix the module in, gets no arm.
    assert!(
        locator.contains(
            "    def self.locate(model_name, id)\n      case model_name\n      when \"User\"\n        User.find_by({ id: id })\n      end\n    end\n"
        ),
        "generated locate:\n{locator}"
    );
    assert!(
        !locator.contains("when \"Room\""),
        "Room is not attachable:\n{locator}"
    );
}

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

/// `Box` in an included helper is `::Box`, not a constant the test class defines.
#[test]
fn an_included_helper_constant_stays_absolute() {
    let (_diags, test) = box_extend_app(
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

/// `LABEL` in the hoisted method is the test class's constant.
#[test]
fn a_hoisted_method_keeps_the_test_class_constant() {
    let (_diags, test) = box_extend_app(
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
    assert_extend_stub(
        "test_mystery",
        "require \"test_helper\"\nclass BoxTest < ActiveSupport::TestCase\n  test \"mystery\" do\n    failing = Module.new { def begin_transport = Mystery }\n    one = Box.new\n    one.extend(failing).ping\n  end\n  test \"sibling\" do\n    assert_equal :open, Box.new.ping\n  end\nend\n",
    );
}

/// The helper default `Note` is the module's constant, not the test class's.
#[test]
fn an_included_helper_default_stays_on_the_module() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
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
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
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
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
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

/// The same test without the extend: what `ActionText::Attachable#
/// attachable_sgid` computes, written out as the `to_sgid(for:)` it is.
/// A shape every target compiles, and Rails mints the identical bytes
/// for it — so `lower::to_sgid` grounds it to the runtime's attachable
/// mint with the model name baked in, read off the fixture call.
#[test]
fn to_sgid_for_the_attachable_locator_is_the_runtime_mint() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("test/fixtures/rooms.yml", "pets:\n  name: Pets\n"),
        (
            "test/models/room_test.rb",
            "require \"test_helper\"\nclass RoomTest < ActiveSupport::TestCase\n  test \"sgid\" do\n    \
             message, signature = rooms(:pets).to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s.split(\"--\")\n    \
             assert_not_nil message\n  end\n\n  test \"other purpose\" do\n    \
             token = rooms(:pets).to_sgid(for: :transfer).to_s\n    assert_not_nil token\n  end\nend\n",
        ),
    ]);
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let test = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room_test.rb"))
        .map(|f| f.content.clone())
        .expect("the emitted test");
    assert!(
        test.contains(
            "ActionText::SignedGlobalId.generate(\"Room\", RoomsFixtures.pets.id).split(\"--\")"
        ),
        "the attachable sgid is the runtime mint:\n{test}"
    );
    // Another purpose is nothing this runtime verifies: reported, left.
    let report = diags
        .iter()
        .find(|d| d.message.contains("not the attachable locator"))
        .expect("the other purpose is reported");
    assert_eq!(report.severity, roundhouse::diagnostic::Severity::Warning);
    assert!(
        test.contains("to_sgid(for: :transfer)"),
        "the declined site is left as written:\n{test}"
    );
}

// campfire's `lib/rails_ext/action_text_attachables.rb`, verbatim: the
// rotated-secret tolerance, as an `on_load` reopen.
const TOLERANT_REOPEN: &str = r#"ActiveSupport.on_load(:action_text_content) do
  class ActionText::Attachment
    class << self
      def from_node(node, attachable = nil)
        new(node, attachable || ActionText::Attachment::OpengraphEmbed.from_node(node) || attachable_from_possibly_expired_sgid(node["sgid"]) || ActionText::Attachable.from_node(node))
      end

      private
        # Our @mentions use ActionText attachments, which are signed. If someone rotates SECRET_KEY_BASE, the existing attachments become invalid.
        # This allows ignoring invalid signatures for User attachments in ActionText.
        ATTACHABLES_PERMITTED_WITH_INVALID_SIGNATURES = %w[ User ]

        def attachable_from_possibly_expired_sgid(sgid)
          if message = sgid&.split("--")&.first
            encoded_message = JSON.parse(decode_base64(message))

            decoded_gid = if data = encoded_message.dig("_rails", "data")
              data
            elsif data = encoded_message.dig("_rails", "message")
              decode_base64(data).match(%r{(gid://campfire/[^/]+/\d+)})&.to_s
            else
              nil
            end

            if model = GlobalID.find(decoded_gid)
              model.model_name.to_s.in?(ATTACHABLES_PERMITTED_WITH_INVALID_SIGNATURES) ? model : nil
            end
          end
        rescue ActiveRecord::RecordNotFound
          nil
        end

        def decode_base64(message)
          Base64.strict_decode64(message)
        rescue => _e
          Base64.urlsafe_decode64(message)
        end
    end
  end
end
"#;

fn locator_of(app: &roundhouse::App) -> String {
    let files = roundhouse::project::spinel_base_files(app, roundhouse::fixtures::real_blog())
        .expect("spinel tree");
    files
        .iter()
        .find(|(p, _)| p.ends_with("global_id_locator.rb"))
        .map(|(_, c)| c.clone())
        .expect("global_id_locator.rb in the tree")
}

#[test]
fn the_tolerant_from_node_reopen_becomes_the_permitted_list() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include ActionText::Attachable\nend\n",
        ),
        ("lib/rails_ext/action_text_attachables.rb", TOLERANT_REOPEN),
    ]);
    assert_eq!(
        app.attachable_unsigned_models
            .iter()
            .map(|m| m.as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["User".to_string()],
        "the %w[] list is the fact the reopen holds"
    );
    // Nothing of the reopen's body becomes a library class: the
    // decode lives in the runtime, once.
    assert!(
        !app.library_classes
            .iter()
            .any(|c| c.name.0.as_str() == "ActionText::Attachment"),
        "the reopen is read, not carried as a class"
    );
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let locator = locator_of(&app);
    assert!(
        locator.contains("    def self.permitted_without_signature\n      [\"User\"]\n    end\n"),
        "generated list:\n{locator}"
    );
}

#[test]
fn a_recognized_reopen_does_not_hide_an_unsupported_include_in_the_same_hook() {
    let source = TOLERANT_REOPEN.replacen(
        "ActiveSupport.on_load(:action_text_content) do",
        "ActiveSupport.on_load(:action_text_content) do\n  include ActionText::HasMarkdown",
        1,
    );
    roundhouse::ingest::survey::activate();
    let app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        ("lib/rails_ext/action_text_attachables.rb", &source),
    ]);
    let gaps = roundhouse::ingest::survey::drain();
    assert_eq!(
        app.attachable_unsigned_models,
        vec![roundhouse::Symbol::from("User")]
    );
    assert_eq!(gaps.len(), 1, "only the include is unsupported: {gaps:?}");
    let message = gaps[0].to_string();
    assert!(
        message.contains("on_load(:action_text_content)"),
        "{message}"
    );
    assert!(
        message.contains("include ActionText::HasMarkdown"),
        "{message}"
    );
    assert!(
        message.contains("mixin installation is unsupported"),
        "{message}"
    );
}

#[test]
fn an_app_without_the_reopen_keeps_the_empty_default() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  include ActionText::Attachable\nend\n",
        ),
    ]);
    assert!(app.attachable_unsigned_models.is_empty());
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let locator = locator_of(&app);
    assert!(
        locator.contains("    def self.permitted_without_signature\n      []\n    end\n"),
        "the scaffold default stands:\n{locator}"
    );
}

#[test]
fn another_on_load_reopen_is_reported_not_dropped() {
    roundhouse::ingest::survey::activate();
    let app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/rails_ext/blob_ext.rb",
            "ActiveSupport.on_load(:active_storage_blob) do\n  class ActiveStorage::Blob\n    def shout\n      filename.to_s.upcase\n    end\n  end\nend\n",
        ),
        // A `from_node` reopen WITHOUT the envelope fingerprint is
        // some other override, and says so.
        (
            "lib/rails_ext/other_from_node.rb",
            "ActiveSupport.on_load(:action_text_content) do\n  class ActionText::Attachment\n    def self.from_node(node)\n      nil\n    end\n  end\nend\n",
        ),
    ]);
    let gaps = roundhouse::ingest::survey::drain();
    assert!(app.attachable_unsigned_models.is_empty());
    let messages: Vec<String> = gaps.iter().map(|g| format!("{g:?}")).collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("on_load(:active_storage_blob)")
                && m.contains("ActiveStorage::Blob")),
        "the blob reopen is named: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("on_load(:action_text_content)")
                && m.contains("ActionText::Attachment")),
        "the unfingerprinted from_node reopen is named: {messages:?}"
    );
}

/// `Content.render_attachment` is generated beside the layout: one arm
/// per attachable model whose `to_attachable_partial_path` names a
/// partial the tree carries, dispatched on the resolved model NAME with
/// the finder and the view module spelled as literals.
#[test]
fn the_attachment_render_is_a_case_over_the_attachables_with_a_partial() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  include Mentionable\nend\n"),
        (
            "app/models/user/mentionable.rb",
            "module User::Mentionable\n  include ActionText::Attachable\n\n  def to_attachable_partial_path\n    \"users/mention\"\n  end\nend\n",
        ),
        // attachable through the marker, but names no partial: no arm.
        ("app/models/room.rb", "class Room < ApplicationRecord\n  include ActionText::Attachable\nend\n"),
        ("app/views/users/_mention.html.erb", "<div class=\"mention\"><%= user.name %></div>\n"),
        ("app/views/layouts/action_text/contents/_content.html.erb", "<div class=\"trix-content\"><%= yield %></div>\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files =
        roundhouse::project::spinel_base_files(&app, std::path::Path::new("fixtures/real-blog"))
            .expect("spinel tree");
    let runtime = files
        .iter()
        .find(|(p, _)| p.ends_with("runtime/action_text.rb"))
        .map(|(_, c)| c.clone())
        .expect("runtime/action_text.rb in the tree");
    assert!(
        runtime.contains(
            "    def self.render_attachment(attachment)\n      case attachment.resolved_model_name\n      when \"User\"\n        user = User.find_by({ id: attachment.resolved_id })\n        user.nil? ? \"\" : Views::Users.mention(user)\n      else\n        \"\"\n      end\n    end\n"
        ),
        "generated render:\n{runtime}"
    );
    assert!(
        !runtime.contains("when \"Room\""),
        "Room names no partial:\n{runtime}"
    );
    assert!(
        // Plus the newline ending Action Text's `_content` partial, which
        // is what the layout yields.
        runtime.contains(
            "Views::Layouts::ActionText::Contents.content(render_action_text_content + \"\\n\")"
        ),
        "the layout wraps the rendered nodes:\n{runtime}"
    );
}

/// A class the node names by CONTENT TYPE — campfire's `OpengraphEmbed`,
/// `attachable_content_type` + a class-side `from_node` + a partial
/// path — gets the arm ahead of the sgid `case`, its partial's local is
/// its `model_name.element`, and the Attachment's `caption` is
/// delegated back onto it through the `attachment=` slot the arm fills.
#[test]
fn a_content_type_attachable_renders_through_its_own_from_node() {
    let mut app = app_with(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "lib/rails_ext/actiontext_opengraph_embeds.rb",
            "class ActionText::Attachment::OpengraphEmbed\n  include ActiveModel::Model\n\n  OPENGRAPH_EMBED_CONTENT_TYPE = \"application/vnd.actiontext.opengraph-embed\"\n\n  class << self\n    def from_node(node)\n      if node[\"content-type\"] == OPENGRAPH_EMBED_CONTENT_TYPE\n        new(href: node[\"href\"], url: node[\"url\"], filename: node[\"filename\"])\n      end\n    end\n  end\n\n  attr_accessor :href, :url, :filename\n\n  def attachable_content_type\n    OPENGRAPH_EMBED_CONTENT_TYPE\n  end\n\n  def to_partial_path\n    \"action_text/attachables/opengraph_embed\"\n  end\nend\n",
        ),
        (
            "app/views/action_text/attachables/_opengraph_embed.html.erb",
            "<figure><%= link_to_if opengraph_embed.href.present?, truncate(opengraph_embed.filename, length: 20), opengraph_embed.href %><span><%= opengraph_embed.caption %></span></figure>\n",
        ),
        ("app/views/layouts/action_text/contents/_content.html.erb", "<div class=\"trix-content\"><%= yield %></div>\n"),
    ]);
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let files =
        roundhouse::project::spinel_base_files(&app, std::path::Path::new("fixtures/real-blog"))
            .expect("spinel tree");
    let text = |suffix: &str| {
        files
            .iter()
            .find(|(p, _)| p.ends_with(suffix))
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("{suffix} in the tree"))
    };
    let runtime = text("runtime/action_text.rb");
    assert!(
        runtime.contains(
            "    def self.render_attachment(attachment)\n      opengraph_embed = ::ActionText::Attachment::OpengraphEmbed.from_node(attachment)\n      unless opengraph_embed.nil?\n        opengraph_embed.attachment = attachment\n        return Views::ActionText::Attachables.opengraph_embed(opengraph_embed)\n      end\n"
        ),
        "generated content-type arm:\n{runtime}"
    );
    let partial = text("app/views/action_text/attachables/_opengraph_embed.rb");
    assert!(
        partial.contains("def self.opengraph_embed_into(io, opengraph_embed, notice, alert)"),
        "the local is the class's element, not the directory's singular:\n{partial}"
    );
    // `link_to_if` is an `if` over the two `link_to` halves, the label
    // escaped once in each.
    assert!(
        partial.contains("if ! (opengraph_embed.href || \"\").empty?"),
        "{partial}"
    );
    assert!(
        partial.contains("ActionView::ViewHelpers.link_to(ActionView::ViewHelpers.truncate("),
        "{partial}"
    );
    assert!(
        !partial.contains("html_escape(ActionView::ViewHelpers.link_to"),
        "the link is not escaped:\n{partial}"
    );
    let class = text("app/models/action_text/attachment/opengraph_embed.rb");
    assert!(
        class.contains("  def attachment=(value)\n    @attachment = value\n  end\n"),
        "{class}"
    );
    assert!(
        class.contains("  def caption\n    a = @attachment\n"),
        "{class}"
    );
}
