//! JSON.dump's destination and limit overloads must survive native dispatch.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
}

const IO: &str = r#"
require "json"
require "stringio"
class JsonSink
  attr_reader :text, :flushes
  def initialize
    @text = +"prefix:"
    @flushes = 0
  end
  def write(text)
    @text << text
    123
  end
  def flush
    @flushes += 1
    self
  end
end
class JsonDestination
  def initialize(io)
    @io = io
  end
  def to_io
    @io
  end
end
class JsonAbsentDestination
  def to_io
    nil
  end
end
class JsonFailingSink
  def write(text)
    raise IOError, "write failed"
  end
  def flush
    raise "flush after failed write"
  end
end
stream = StringIO.new
p JSON.dump({"a" => [1]}, stream).equal?(stream)
p stream.string
sink = JsonSink.new
p JSON.dump([true, nil], sink).equal?(sink)
p sink.text
p sink.flushes
adapter = JsonDestination.new(stream)
p JSON.dump({}, adapter).equal?(stream)
p stream.string
p JSON.dump({}, StringIO.new).write("!")
p JSON.dump({}, JsonAbsentDestination.new)
File.open("dump.json", "w") do |file|
  p JSON.dump({"file" => 2}, file).equal?(file)
end
p File.read("dump.json")
begin
  JSON.dump({}, JsonFailingSink.new)
rescue IOError => dump_error
  puts dump_error.class.to_s
end
"#;

const LIMIT: &str = r#"
require "json"
require "stringio"
p JSON.dump([{"a" => [1]}], nil, 3)
p JSON.dump([{"a" => [1]}], 3)
p JSON.dump([[[1]]], nil, 0)
p JSON.dump([[[1]]], nil, false)
p JSON.dump([[[1]]], nil, "ignored")
stream = StringIO.new
begin
  JSON.dump([{"a" => [1]}], stream, 2)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
begin
  JSON.dump([[[1]]], 1)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
"#;

// The supported MRI 3.4 line bundles json 2.9.1: the fourth argument is
// accepted, but only :strict is a valid option. In particular, an options
// hash cannot override the positional nesting limit as it can in json 2.18.
const OPTIONS: &str = r#"
require "json"
require "stringio"
p JSON.dump({}, nil, nil, {})
p JSON.dump([1], {strict: true})
p JSON.dump([1], nil, {strict: false})
p JSON.dump([1], nil, 1, {strict: true})
[2, 3, 4].each do |position|
  begin
    if position == 2
      JSON.dump([1], {max_nesting: 1})
    elsif position == 3
      JSON.dump([1], nil, {max_nesting: 1})
    else
      JSON.dump([[[1]]], nil, 1, {max_nesting: false})
    end
  rescue ArgumentError => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
end
stream = StringIO.new
begin
  JSON.dump([1], stream, nil, {max_nesting: false, allow_nan: true})
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
p stream.string
begin
  JSON.dump({}, nil, -1, nil)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
begin
  JSON.dump([[1]], nil, 1, {strict: false})
rescue ArgumentError => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
begin
  JSON.dump({}, nil, nil, {}, nil)
rescue ArgumentError => dump_error
  puts dump_error.class.to_s
end
"#;

// :strict must be honored, including nested values and objects with to_json.
// Hash keys retain json's string conversion even in strict mode.
const STRICT: &str = r#"
require "json"
require "stringio"
class JsonCustomValue
  def to_json(state = nil)
    '"custom"'
  end
end
p JSON.dump({a: [true, nil, 1, "s"]}, strict: true)
[JsonCustomValue.new, :value, [JsonCustomValue.new], {a: :value}].each do |value|
  stream = StringIO.new
  begin
    JSON.dump(value, stream, nil, {strict: true})
  rescue JSON::GeneratorError => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
  p stream.string
end
"#;

// A destination can be absent directly or after one to_io conversion. Exercise
// every overload with scalars, arrays, hashes and alternating nested containers.
const FALSEY: &str = r#"
require "json"
class JsonFalseDestination
  attr_reader :conversions
  def initialize
    @conversions = 0
  end
  def to_io
    @conversions += 1
    false
  end
end
class JsonNilDestination
  def to_io
    nil
  end
end
class JsonInheritedFalseDestination < JsonFalseDestination
end
module JsonFalseConversion
  def to_io
    false
  end
end
class JsonIncludedFalseDestination
  include JsonFalseConversion
end
class JsonAliasDestination
  def absent_destination
    false
  end
  alias to_io absent_destination
end
class JsonAliasMethodDestination
  def absent_destination
    nil
  end
  alias_method :to_io, :absent_destination
end
class JsonConvertedDestination
  def initialize(io)
    @io = io
  end
  def to_io
    @io
  end
end
JsonEncoder = JSON
destinations = [false, nil, JsonFalseDestination.new, JsonNilDestination.new,
                JsonInheritedFalseDestination.new, JsonIncludedFalseDestination.new,
                JsonAliasDestination.new, JsonAliasMethodDestination.new]
destinations.each do |destination|
  [nil, false, 7, [], [1], {"a" => [1]}, [{"a" => [1]}]].each do |value|
    p JSON.dump(value, destination)
    p JSON.dump(value, destination, 4)
    p JSON.dump(value, destination, 0)
    p JSON.dump(value, destination, false)
    p JSON.dump(value, destination, {strict: false})
    p JSON.dump(value, destination, nil, {strict: true})
    p JSON.dump(value, destination, 4, {strict: true})
  end
  begin
    JSON.dump([{"a" => [1]}], destination, 1)
  rescue ArgumentError => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
end
adapter = JsonFalseDestination.new
destination_alias = adapter
p ::JSON.dump([1], destination_alias, 2)
p adapter.conversions
p JsonEncoder.dump([1], false, 2)
p JSON.dump(JSON.dump([1], false, 2), false, 2)
# Truthy non-writers still fail, including after exactly one conversion.
[true, 0, "", JsonFalseDestination.new].each do |value|
  begin
    JSON.dump([1], JsonConvertedDestination.new(value), 2)
  rescue NoMethodError => dump_error
    puts dump_error.class.to_s
  end
end
[true, 0, ""].each do |destination|
  begin
    JSON.dump([1], destination, 2)
  rescue NoMethodError => dump_error
    puts dump_error.class.to_s
  end
end
"#;

// These limits used to gain unintended coercions: Float#to_i raises for
// Infinity/NaN, and Rational#to_i invents a limit the oracle does not enforce.
// Fractional Float limits remain an existing bundled-encoder limitation.
const NUMERIC_BOUNDARIES: &str = r#"
require "json"
require "stringio"
[Float::INFINITY, -Float::INFINITY, Float::NAN, Rational(1, 1), Rational(-1, 1)].each do |limit|
  value = [{"a" => [1]}]
  p JSON.dump(value, limit)
  p JSON.dump(value, nil, limit)
  p JSON.dump(value, nil, limit, {strict: false})
  p JSON.dump(value, false, limit, {strict: true})
  stream = StringIO.new
  p JSON.dump(value, stream, limit).equal?(stream)
  p stream.string
  p JSON.dump(value, stream, limit, {strict: false}).equal?(stream)
  p stream.string
end
"#;

// json 2.9 only enforces immediate Integer limits on this 64-bit target.
const INTEGER_BOUNDARIES: &str = r#"
require "json"
require "stringio"
[-1, -(2**62), -(2**62)-1, -(2**64), 2**62-1, 2**62, 2**64].each do |limit|
  puts limit.to_s
  begin
    p JSON.dump([], limit)
    p JSON.dump([], nil, limit)
    p JSON.dump([], nil, limit, {strict: false})
    p JSON.dump([], false, limit, {strict: true})
    stream = StringIO.new
    p JSON.dump([], stream, limit).equal?(stream)
    p stream.string
  rescue ArgumentError => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
end
"#;

const CONVERSION_EXCEPTIONS: &str = r#"
require "json"
class JsonRaisingConversion
  def to_io
    raise JSON::NestingError, "conversion sentinel"
  end
end
class JsonRaisingWriter
  def write(text)
    raise JSON::NestingError, "writer sentinel"
  end
  def flush
    nil
  end
end
[JsonRaisingConversion.new, JsonRaisingWriter.new].each do |destination|
  begin
    JSON.dump([], destination)
  rescue => dump_error
    puts dump_error.class.to_s + ":" + dump_error.message
  end
end
begin
  JSON.dump([[]], nil, 1)
rescue => dump_error
  puts dump_error.class.to_s + ":" + dump_error.message
end
"#;

struct DumpContract {
    name: &'static str,
    script: &'static str,
    golden: &'static str,
    service: Option<(&'static str, &'static str)>,
}

impl DumpContract {
    fn overlay(&self) -> emit_and_run::Overlay {
        match self.service {
            Some((path, source)) => app().write(path, source),
            None => app(),
        }
    }
}

// Generated with MRI 3.4.11 / json 2.9.1, the supported unit-job oracle.
// Native framework jobs have no Ruby prerequisite. The non-ignored oracle
// test verifies every golden, including all application-sidecar probes.
// Regenerate with that Ruby on PATH:
// UPDATE_JSON_DUMP_GOLDENS=1 cargo test --locked --test json_dump cruby_dump_contracts
const DUMP_CONTRACTS: &[DumpContract] = &[
    DumpContract {
        name: "io", script: IO,
        golden: include_str!("json_dump/io.stdout"), service: None,
    },
    DumpContract {
        name: "limit", script: LIMIT,
        golden: include_str!("json_dump/limit.stdout"), service: None,
    },
    DumpContract {
        name: "options", script: OPTIONS,
        golden: include_str!("json_dump/options.stdout"), service: None,
    },
    DumpContract {
        name: "strict", script: STRICT,
        golden: include_str!("json_dump/strict.stdout"), service: None,
    },
    DumpContract {
        name: "falsey", script: FALSEY,
        golden: include_str!("json_dump/falsey.stdout"), service: None,
    },
    DumpContract {
        name: "numeric_boundaries", script: NUMERIC_BOUNDARIES,
        golden: include_str!("json_dump/numeric_boundaries.stdout"), service: None,
    },
    DumpContract {
        name: "integer_boundaries", script: INTEGER_BOUNDARIES,
        golden: include_str!("json_dump/integer_boundaries.stdout"), service: None,
    },
    DumpContract {
        name: "conversion_exceptions", script: CONVERSION_EXCEPTIONS,
        golden: include_str!("json_dump/conversion_exceptions.stdout"), service: None,
    },
    DumpContract {
        name: "falsey_sidecars", script: FALSEY_SIDECARS,
        golden: include_str!("json_dump/falsey_sidecars.stdout"),
        service: Some(("app/services/json_probe.rb", RETURN_TYPES)),
    },
    DumpContract {
        name: "destination_shapes", script: DESTINATION_SHAPES_PROBE,
        golden: include_str!("json_dump/destination_shapes.stdout"),
        service: Some(("app/services/json_destinations.rb", DESTINATION_SHAPES)),
    },
    DumpContract {
        name: "destination_visibility", script: DESTINATION_VISIBILITY_PROBE,
        golden: include_str!("json_dump/destination_visibility.stdout"),
        service: Some(("app/services/json_visibility_probe.rb", DESTINATION_VISIBILITY)),
    },
];

fn compare_native(name: &str) {
    let contract = DUMP_CONTRACTS.iter().find(|contract| contract.name == name).unwrap();
    let native = contract.overlay().run_spinel_with_rbs(contract.script);
    native.assert_passes();
    assert_eq!(
        native.stdout, contract.golden,
        "native JSON.dump differs from the CRuby golden: {name}"
    );
}

#[test]
fn cruby_dump_contracts() {
    let regenerate = std::env::var("UPDATE_JSON_DUMP_GOLDENS").as_deref() == Ok("1");
    for contract in DUMP_CONTRACTS {
        let oracle = contract.overlay().run_ruby(contract.script);
        oracle.assert_passes();
        if regenerate {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/json_dump").join(format!("{}.stdout", contract.name));
            std::fs::write(path, &oracle.stdout).unwrap();
        } else {
            assert_eq!(
                oracle.stdout, contract.golden,
                "CRuby JSON.dump golden drifted: {}", contract.name
            );
        }
    }
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_writes_and_returns_its_destination() {
    compare_native("io");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_honors_the_nesting_limit_before_writing() {
    compare_native("limit");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_distinguishes_options_from_invalid_nesting_and_arity() {
    compare_native("options");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_honors_strict_options_before_writing() {
    compare_native("strict");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_treats_falsey_destinations_as_absent() {
    compare_native("falsey");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_does_not_coerce_noninteger_limits() {
    compare_native("numeric_boundaries");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_ignores_bignum_limits() {
    compare_native("integer_boundaries");
}

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_preserves_conversion_exceptions() {
    compare_native("conversion_exceptions");
}

const RETURN_TYPES: &str = r#"
require "json"
require "stringio"
class JsonDestination
  def to_io
    StringIO.new
  end
end
class JsonAbsentDestination
  def to_io
    nil
  end
end
class JsonFalseDestination
  def to_io
    false
  end
end
class JsonInheritedFalseDestination < JsonFalseDestination
end
module JsonFalseConversion
  def to_io
    false
  end
end
class JsonIncludedFalseDestination
  include JsonFalseConversion
end
class JsonMaybeDestination
  def initialize(flag)
    @flag = flag
  end
  def to_io
    @flag ? StringIO.new : false
  end
end
class JsonThreeWayDestination
  def initialize(writer, absent)
    @writer = writer
    @absent = absent
  end
  def to_io
    @writer ? StringIO.new : (@absent ? false : nil)
  end
end
class JsonFalseOrNilDestination
  def initialize(flag)
    @flag = flag
  end
  def to_io
    @flag ? false : nil
  end
end
class JsonProbe
  JsonEncoder = JSON
  def self.text
    JSON.dump({"a" => 1})
  end
  def self.stream
    JSON.dump({"a" => 1}, StringIO.new)
  end
  def self.limit
    JSON.dump([1], 1)
  end
  def self.options
    JSON.dump([1], {strict: true})
  end
  def self.converted
    JSON.dump({}, JsonDestination.new)
  end
  def self.absent
    JSON.dump({}, JsonAbsentDestination.new)
  end
  def self.maybe_stream(flag)
    JSON.dump({}, flag ? StringIO.new : nil)
  end
  def self.write_count
    JSON.dump({}, StringIO.new).write("more")
  end
  def self.false_destination
    JSON.dump([1], false, 2)
  end
  def self.false_options
    JSON.dump({"a" => [1]}, false, {strict: false})
  end
  def self.false_fourth
    JSON.dump([1], false, nil, {strict: true})
  end
  def self.false_alias
    destination = false
    ::JSON.dump([1], destination, 2)
  end
  def self.receiver_alias
    JsonEncoder.dump([1], false, 2)
  end
  def self.false_adapter
    JSON.dump([1], JsonFalseDestination.new, 2)
  end
  def self.false_inherited
    JSON.dump([1], JsonInheritedFalseDestination.new, 2)
  end
  def self.false_included
    JSON.dump([1], JsonIncludedFalseDestination.new, 2)
  end
  def self.false_nested
    JSON.dump(JSON.dump([1], false, 2), false, 2)
  end
  def self.false_count
    JSON.dump([1], JsonFalseDestination.new, 2).bytesize
  end
  def self.false_or_nil_adapter(flag)
    JSON.dump([1], JsonFalseOrNilDestination.new(flag), 2)
  end
  def self.maybe_false_stream(flag)
    JSON.dump([1], flag ? StringIO.new : false, 2)
  end
  def self.maybe_converted(flag)
    JSON.dump([1], JsonMaybeDestination.new(flag), 2)
  end
  def self.three_way_converted(writer, absent)
    JSON.dump([1], JsonThreeWayDestination.new(writer, absent), 2)
  end
  def self.unknown(io)
    JSON.dump({}, io)
  end
end
"#;

#[test]
fn dump_return_types_follow_the_destination() {
    let (emitted, errors) = app()
        .write("app/services/json_probe.rb", RETURN_TYPES)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(emitted.join("runtime/json_impl.rbs").is_file());
    let rbs = std::fs::read_to_string(emitted.join("app/models/json_probe.rbs")).unwrap();
    for name in [
        "text",
        "limit",
        "options",
        "absent",
        "false_destination",
        "false_options",
        "false_fourth",
        "false_alias",
        "receiver_alias",
        "false_adapter",
        "false_inherited",
        "false_included",
        "false_nested",
    ] {
        assert!(
            rbs.contains(&format!("def self.{name}: () -> String\n")),
            "{rbs}"
        );
    }
    for name in ["stream", "converted"] {
        assert!(
            rbs.contains(&format!("def self.{name}: () -> StringIO\n")),
            "{rbs}"
        );
    }
    assert!(
        rbs.contains("def self.write_count: () -> Integer\n"),
        "{rbs}"
    );
    assert!(
        rbs.contains("def self.false_count: () -> Integer\n"),
        "{rbs}"
    );
    let absent = rbs
        .lines()
        .find(|line| line.contains("def self.false_or_nil_adapter:"))
        .unwrap();
    assert!(absent.ends_with(" -> String"), "{rbs}");
    assert!(
        rbs.contains("def self.unknown: (untyped io) -> untyped\n"),
        "{rbs}"
    );
    for name in [
        "maybe_stream",
        "maybe_false_stream",
        "maybe_converted",
        "three_way_converted",
    ] {
        let optional = rbs
            .lines()
            .find(|line| line.contains(&format!("def self.{name}:")))
            .unwrap();
        assert!(
            optional.contains("String | StringIO") || optional.contains("StringIO | String"),
            "{rbs}"
        );
        assert!(
            !optional.contains("bool") && !optional.contains("nil"),
            "{rbs}"
        );
    }
}

const FALSEY_SIDECARS: &str = r#"
p JsonProbe.false_destination
p JsonProbe.false_options
p JsonProbe.false_fourth
p JsonProbe.false_alias
p JsonProbe.receiver_alias
p JsonProbe.false_adapter
p JsonProbe.false_inherited
p JsonProbe.false_included
p JsonProbe.false_nested
p JsonProbe.false_count
p JsonProbe.false_or_nil_adapter(true)
p JsonProbe.false_or_nil_adapter(false)
p JsonProbe.maybe_false_stream(false)
p JsonProbe.maybe_false_stream(true).string
p JsonProbe.maybe_converted(false)
p JsonProbe.maybe_converted(true).string
p JsonProbe.three_way_converted(false, true)
p JsonProbe.three_way_converted(false, false)
p JsonProbe.three_way_converted(true, false).string
"#;

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_falsey_returns_execute_with_application_sidecars() {
    compare_native("falsey_sidecars");
}

// Equivalent destination types must survive each expression form. Singleton
// conversions deliberately differ from the same class's instance conversion.
const DESTINATION_SHAPES: &str = r#"
require "json"
require "stringio"
class JsonClassAdapter
  def self.to_io
    StringIO.new
  end
  def to_io
    false
  end
end
class JsonClassWriter
  def self.write(text)
    puts text
    text.bytesize
  end
  def self.flush
    nil
  end
end
class JsonDestinations
  STREAM = StringIO.new
  ABSENT = false
  def initialize
    @stream = StringIO.new
    @absent = false
    @class_destination = JsonClassAdapter
  end
  def self.destination
    StringIO.new
  end
  def self.local
    stream = StringIO.new
    JSON.dump([1], stream)
  end
  def self.local_absent
    destination = false
    JSON.dump([1], destination)
  end
  def ivar
    JSON.dump([1], @stream)
  end
  def ivar_absent
    JSON.dump([1], @absent)
  end
  def ivar_class
    JSON.dump([1], @class_destination)
  end
  def self.class_method_call
    JSON.dump([1], JsonDestinations.destination)
  end
  def self.constant
    JSON.dump([1], STREAM)
  end
  def self.constant_absent
    JSON.dump([1], ABSENT)
  end
  def self.absent_destination
    nil
  end
  def self.call_absent
    JSON.dump([1], JsonDestinations.absent_destination)
  end
  def self.unknown(destination)
    JSON.dump([1], destination)
  end
  def self.singleton_conversion
    JSON.dump([1], JsonClassAdapter)
  end
  def self.singleton_alias
    destination = JsonClassAdapter
    JSON.dump([1], destination)
  end
  def self.instance_conversion
    JSON.dump([1], JsonClassAdapter.new)
  end
  def self.singleton_writer
    JSON.dump([1], JsonClassWriter)
  end
  def self.class_destination
    JsonClassAdapter
  end
  def self.returned_class_conversion
    JSON.dump([1], JsonDestinations.class_destination)
  end
end
"#;

fn destination_rbs() -> String {
    let (emitted, errors) = app()
        .write("app/services/json_destinations.rb", DESTINATION_SHAPES)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    std::fs::read_to_string(emitted.join("app/models/json_destinations.rbs")).unwrap()
}

fn assert_destination_returns(returns: &[(&str, &str)]) {
    let rbs = destination_rbs();
    for (method, result) in returns {
        assert!(rbs.contains(&format!("def {method}: () -> {result}\n")), "{rbs}");
    }
}

#[test]
fn dump_local_destination_returns() {
    assert_destination_returns(&[
        ("self.local", "StringIO"),
        ("self.local_absent", "String"),
        ("self.singleton_alias", "StringIO"),
    ]);
}

#[test]
fn dump_ivar_destination_returns() {
    assert_destination_returns(&[
        ("ivar", "StringIO"),
        ("ivar_absent", "String"),
        ("ivar_class", "untyped"),
    ]);
}

#[test]
fn dump_class_method_destination_returns() {
    assert_destination_returns(&[
        ("self.class_method_call", "StringIO"),
        ("self.call_absent", "String"),
        ("self.returned_class_conversion", "StringIO"),
    ]);
}

#[test]
fn dump_constant_destination_returns() {
    assert_destination_returns(&[
        ("self.constant", "StringIO"),
        ("self.constant_absent", "String"),
        ("self.singleton_conversion", "StringIO"),
        ("self.singleton_writer", "untyped"),
        ("self.instance_conversion", "untyped"),
    ]);
}

#[test]
fn dump_unknown_destination_return_stays_gradual() {
    let rbs = destination_rbs();
    assert!(rbs.contains("def self.unknown: (untyped destination) -> untyped\n"), "{rbs}");
}

const DESTINATION_SHAPES_PROBE: &str = r#"
p JsonDestinations.local.string
p JsonDestinations.local_absent
p JsonDestinations.new.ivar.string
p JsonDestinations.new.ivar_absent
p JsonDestinations.new.ivar_class.string
p JsonDestinations.class_method_call.string
p JsonDestinations.call_absent
p JsonDestinations.constant.string
p JsonDestinations.constant_absent
p JsonDestinations.singleton_conversion.string
p JsonDestinations.singleton_alias.string
p JsonDestinations.instance_conversion
p JsonDestinations.singleton_writer.equal?(JsonClassWriter)
p JsonDestinations.returned_class_conversion.string
p JsonDestinations.unknown(StringIO.new).string
p JsonDestinations.unknown(false)
p JsonDestinations.unknown(nil)
"#;

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_destination_shapes_execute_with_application_sidecars() {
    compare_native("destination_shapes");
}

// The consumer stays outside analysis so dump's destination remains untyped.
const DESTINATION_VISIBILITY: &str = r#"
require "json"
require "stringio"
class JsonPrivateConversion
  private
  def to_io
    raise "private conversion called"
  end
end
class JsonPrivateWriter
  private
  def write(text)
    raise "private writer called"
  end
end
class JsonPublicConversion
  def to_io
    StringIO.new
  end
end
class JsonNeitherProtocol
end
class JsonPrivateConversionWriter
  attr_reader :text
  def initialize
    @text = +""
  end
  def write(text)
    @text << text
    text.bytesize
  end
  def flush
    nil
  end
  private
  def to_io
    nil
  end
end
class JsonVisibilityProbe
  def self.dump(destination)
    JSON.dump([1], destination)
  end
  def self.writer
    JSON.dump([1], JsonPrivateConversionWriter.new)
  end
  def self.converted
    JSON.dump([1], JsonPublicConversion.new)
  end
end
"#;

#[test]
fn dump_destination_inference_ignores_private_conversion() {
    let (emitted, errors) = app()
        .write("app/services/json_visibility_probe.rb", DESTINATION_VISIBILITY)
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let rbs = std::fs::read_to_string(emitted.join("app/models/json_visibility_probe.rbs")).unwrap();
    assert!(rbs.contains("def self.dump: (untyped destination) -> untyped\n"), "{rbs}");
    assert!(rbs.contains("def self.writer: () -> JsonPrivateConversionWriter\n"), "{rbs}");
    assert!(rbs.contains("def self.converted: () -> StringIO\n"), "{rbs}");
}

const DESTINATION_VISIBILITY_PROBE: &str = r#"
p JsonVisibilityProbe.dump(JsonPrivateConversion.new)
p JsonVisibilityProbe.dump(JsonPrivateWriter.new)
p JsonVisibilityProbe.dump(JsonPublicConversion.new).string
p JsonVisibilityProbe.dump(JsonNeitherProtocol.new)
writer = JsonPrivateConversionWriter.new
p JsonVisibilityProbe.dump(writer).equal?(writer)
p writer.text
p JsonVisibilityProbe.writer.text
p JsonVisibilityProbe.converted.string
"#;

#[test]
#[ignore = "requires the native Spinel compiler; set SPINEL"]
fn dump_destination_visibility_executes_with_application_sidecars() {
    compare_native("destination_visibility");
}
