//! Test-local indexers share the native ancestor ABI and retain source bodies.
//! Native probes compile verbatim emitted helper classes and Base accessors.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

const HELPERS: &str = r#"require "test_helper"
class LocalRecord < ActiveRecord::Base
  def [](name)
    case name
    when :title then "okay"
    when "other" then "string"
    when 7 then "integer"
    when 2.5 then "float"
    when false then "false"
    when nil then "nil"
    else nil
    end
  end
  def []=(name, value)
    case name
    when :title then @written = "yes"
    else @written = "no"
    end
    nil
  end
  def written
    @written
  end
end
class MiddleRecord < ActiveRecord::Base
end
class AliasedRecord < MiddleRecord
  def [](name)
    key = name
    other = key
    case other
    when :title then "aliased"
    else nil
    end
  end
end
class OverrideRecord < LocalRecord
  def [](name)
    case name
    when :title
      case name
      when :title then "nested"
      else nil
      end
    else nil
    end
  end
end
class WriterRecord < MiddleRecord
  def []=(name, value)
    key = name
    case key
    when "title" then @written = "yes"
    else @written = "no"
    end
    nil
  end
  def written
    @written
  end
end
class LocalRecordTest < ActiveSupport::TestCase
  test "reads helper keys" do
    assert_equal "okay", LocalRecord.new[:title]
    assert_equal "aliased", AliasedRecord.new[:title]
    assert_equal "nested", OverrideRecord.new[:title]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("test/models/local_record_test.rb", HELPERS)
        .write(
            "test/models/a_remote_child_test.rb",
            r#"require "test_helper"
require_relative "z_remote_parent_test"
class RemoteChildRecord < RemoteParentRecord
  def [](name)
    case name
    when :title then "remote"
    else nil
    end
  end
end
class RemoteChildTest < ActiveSupport::TestCase
  test "reads a remote helper" do
    assert_equal "remote", RemoteChildRecord.new[:title]
  end
end
"#,
        )
        .write(
            "test/models/z_remote_parent_test.rb",
            r#"require "test_helper"
require_relative "local_record_test"
class RemoteParentRecord < LocalRecord
end
class RemoteParentTest < ActiveSupport::TestCase
  test "reads a parent helper" do
    assert_equal "okay", RemoteParentRecord.new[:title]
  end
end
"#,
        )
}

fn read(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap()
}

fn block(source: &str, needle: &str) -> String {
    let position = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}: {source}"));
    let start = source[..position].rfind('\n').map_or(0, |p| p + 1);
    let brace = position + source[position..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[brace..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=brace + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated {needle}");
}

fn classes(root: &Path, swift: bool) -> String {
    let prefix = if swift { "Tests/AppTests" } else { "tests" };
    let extension = if swift { "swift" } else { "cs" };
    // Native declaration order is topological; Ruby ingest and emitter
    // registration are deliberately tested with the opposite file order.
    [
        ("LocalRecordTest", "LocalRecord"),
        ("LocalRecordTest", "MiddleRecord"),
        ("LocalRecordTest", "AliasedRecord"),
        ("LocalRecordTest", "OverrideRecord"),
        ("LocalRecordTest", "WriterRecord"),
        ("RemoteParentTest", "RemoteParentRecord"),
        ("RemoteChildTest", "RemoteChildRecord"),
    ]
    .iter()
    .map(|(file, name)| {
        let source = read(root, &format!("{prefix}/{file}.{extension}"));
        block(
            &source,
            &format!("class {name}{}", if swift { ":" } else { " :" }),
        )
    })
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn source_helpers_resolve_dynamic_keys_across_ancestors_and_test_files() {
    for (target, swift, declaration) in [
        (BuildTarget::CSharp, false, "this[object? name]"),
        (BuildTarget::Swift, true, "subscript(name: Any?)"),
    ] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let helpers = classes(&emitted, swift);
        assert_eq!(helpers.matches(declaration).count(), 5, "{helpers}");
    }
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn return_helpers() -> (emit_and_run::Emitted, String) {
    let (emitted, errors) = app()
        .write(
            "test/models/z_return_parent_test.rb",
            r#"require "test_helper"
class ReturnParent
  attr_reader :reader
  attr_writer :writer
  def initialize
    @reader = 0
    @writer = 0
    @state = 0
  end
  def label; 0; end
  def argument(key); 0; end
  def optional(key = 0); 0; end
  def from_nil; nil; end
  def to_nil; 0; end
  def state_label; @state; end
  def self.class_label; 0; end
  def [](key); 0; end
  def []=(key, value); nil; end
end
class ReturnDirect < ReturnParent
  def label; "direct"; end
end
class ReturnMiddle < ReturnParent
end
class ReturnGetterParent
  def [](key); 0; end
end
class ReturnSetterParent
  def []=(key, value); nil; end
end
class ReturnParentTest < ActiveSupport::TestCase
  test "parent return" do
    assert_equal 0, ReturnParent.new.label
  end
end
"#,
        )
        .write(
            "test/models/a_return_child_test.rb",
            r#"require "test_helper"
require_relative "z_return_parent_test"
class ReturnChild < ReturnMiddle
  attr_reader :reader
  attr_writer :writer
  def initialize
    @reader = "reader"
    @writer = "writer"
    @state = "state"
  end
  def label; "child"; end
  def argument(key); "argument"; end
  def optional(key = 0); "optional"; end
  def from_nil; "value"; end
  def to_nil; nil; end
  def state_label; @state; end
  def self.class_label; "class"; end
  def [](key); "indexer"; end
  def []=(key, value); nil; end
end
class ReturnLeaf < ReturnChild
  def label; false; end
end
class ReturnGetterChild < ReturnGetterParent
  def [](key); "getter"; end
end
class ReturnSetterChild < ReturnGetterParent
  def []=(key, value); nil; end
end
class ReturnReaderChild < ReturnSetterParent
  def [](key); "reader"; end
end
class ReturnChildTest < ActiveSupport::TestCase
  test "child return" do
    assert_equal "child", ReturnChild.new.label
  end
end
"#,
        )
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let helpers = [
        ("ReturnParentTest", "ReturnParent"),
        ("ReturnParentTest", "ReturnDirect"),
        ("ReturnParentTest", "ReturnMiddle"),
        ("ReturnParentTest", "ReturnGetterParent"),
        ("ReturnParentTest", "ReturnSetterParent"),
        ("ReturnChildTest", "ReturnChild"),
        ("ReturnChildTest", "ReturnLeaf"),
        ("ReturnChildTest", "ReturnGetterChild"),
        ("ReturnChildTest", "ReturnSetterChild"),
        ("ReturnChildTest", "ReturnReaderChild"),
    ]
    .iter()
    .map(|(file, name)| {
        block(&read(&emitted, &format!("tests/{file}.cs")), &format!("public class {name} "))
    })
    .collect::<Vec<_>>()
    .join("\n");
    (emitted, helpers)
}

#[test]
fn helper_registration_preserves_independent_member_return_types() {
    let (_emitted, helpers) = return_helpers();
    assert!(!helpers.contains("override "), "{helpers}");
    for declaration in [
        "public virtual long Label()",
        "public virtual string Label()",
        "public virtual bool Label()",
        "public virtual void FromNil()",
        "public virtual string FromNil()",
        "public virtual void ToNil()",
        "public virtual long this[",
        "public virtual string this[",
        "public string Reader",
        "public string Writer",
        "public string State",
    ] {
        assert!(helpers.contains(declaration), "missing {declaration}: {helpers}");
    }
}

#[test]
#[ignore = "requires .NET 8; compiles helpers with differing member return types"]
fn helper_member_return_types_compile_and_run_on_csharp() {
    let (emitted, helpers) = return_helpers();
    let native = emitted.join("native-return-probe");
    std::fs::create_dir(&native).unwrap();
    std::fs::write(native.join("Native.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><Nullable>enable</Nullable></PropertyGroup></Project>"#).unwrap();
    std::fs::write(native.join("Program.cs"), format!(r#"using System;
namespace Roundhouse;
{helpers}
public static class Program {{
    public static void Main() {{
        if (new ReturnParent().Label() != 0L) throw new Exception("parent");
        if (new ReturnDirect().Label() != "direct") throw new Exception("direct");
        var child = new ReturnChild();
        if (child.Label() != "child") throw new Exception("indirect");
        if (child.Argument(null) != "argument") throw new Exception("argument");
        if (child.Optional() != "optional") throw new Exception("optional");
        if (child.FromNil() != "value") throw new Exception("from nil");
        child.ToNil();
        if (child.Reader != "reader") throw new Exception("reader");
        child.Writer = "written";
        if (child.Writer != "written") throw new Exception("writer");
        if (child.StateLabel() != "state") throw new Exception("body ivar");
        if (ReturnChild.ClassLabel() != "class") throw new Exception("class method");
        if (child["key"] != "indexer") throw new Exception("indexer");
        child["key"] = "written";
        if (new ReturnGetterChild()["key"] != "getter") throw new Exception("getter only");
        new ReturnSetterChild()["key"] = "written";
        if (new ReturnReaderChild()["key"] != "reader") throw new Exception("setter parent");
        if (new ReturnLeaf().Label()) throw new Exception("second override");
        Console.WriteLine("helper return types passed");
    }}
}}
"#)).unwrap();
    let dotnet = std::env::var("DOTNET").unwrap_or_else(|_| "dotnet".into());
    run(Command::new(dotnet)
        .args(["run", "--project", "Native.csproj"])
        .current_dir(native));
}

#[test]
#[ignore = "requires .NET 8; compiles source-defined helper indexers"]
fn source_helper_indexers_compile_and_run_on_csharp() {
    let (emitted, errors) = app().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    let base = block(
        &read(&emitted, "app/runtime/ActiveRecordBase.cs"),
        " object? this[",
    );
    let helpers = classes(&emitted, false);
    std::fs::write(native.join("Native.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><Nullable>enable</Nullable></PropertyGroup></Project>"#).unwrap();
    std::fs::write(
        native.join("Program.cs"),
        format!(
            r#"using System;
using System.Collections.Generic;
namespace Roundhouse;
public class ActiveRecordBase {{
{base}
}}
{helpers}
public static class Program {{
    public static void Main() {{
        var model = new LocalRecord();
        ActiveRecordBase record = model;
        foreach (var (key, expected) in new (object?, string?)[] {{
            ("title", "okay"), ("other", "string"), (7L, "integer"),
            (2.5, "float"), (false, "false"), (null, "nil"), ("missing", null)
        }}) if (!Equals(record[key], expected)) throw new Exception("key dispatch");
        object dynamicKey = new string("title".ToCharArray());
        record[dynamicKey] = "ignored";
        if (!Equals(model.Written(), "yes")) throw new Exception("setter branch");
        record["missing"] = "ignored";
        if (!Equals(model.Written(), "no")) throw new Exception("setter default");
        foreach (var (helper, expected) in new (ActiveRecordBase, string)[] {{
            (new AliasedRecord(), "aliased"), (new OverrideRecord(), "nested"),
            (new RemoteParentRecord(), "okay"), (new RemoteChildRecord(), "remote")
        }}) {{
            object? key = "title";
            if (!Equals(helper[key], expected)) throw new Exception("helper ancestry");
            if (helper["missing"] != null) throw new Exception("helper default");
        }}
        var writer = new WriterRecord();
        ((ActiveRecordBase)writer)[dynamicKey] = "ignored";
        if (!Equals(writer.Written(), "yes")) throw new Exception("setter-only");
        Console.WriteLine("source helper indexers passed");
    }}
}}
"#
        ),
    )
    .unwrap();
    let dotnet = std::env::var("DOTNET").unwrap_or_else(|_| "dotnet".into());
    run(Command::new(dotnet)
        .args(["run", "--project", "Native.csproj"])
        .current_dir(native));
}

#[test]
#[ignore = "requires Swift; compiles source-defined helper indexers"]
fn source_helper_indexers_compile_and_run_on_swift() {
    let (emitted, errors) = app().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    let base = block(
        &read(&emitted, "Sources/App/ActiveRecordBase.swift"),
        "subscript(",
    );
    let helpers = classes(&emitted, true);
    std::fs::write(
        native.join("main.swift"),
        format!(
            r#"import Foundation
class ActiveRecordBase {{
{base}
}}
{helpers}
let model = LocalRecord()
let record: ActiveRecordBase = model
for (key, expected): (Any?, String?) in [
    ("title", "okay"), ("other", "string"), (7, "integer"),
    (2.5, "float"), (false, "false"), (nil, "nil"), ("missing", nil)
] {{ precondition(record[key] as? String == expected) }}
record["title"] = "ignored"
precondition(model.written as? String == "yes")
record["missing"] = "ignored"
precondition(model.written as? String == "no")
for (helper, expected): (ActiveRecordBase, String) in [
    (AliasedRecord(), "aliased"), (OverrideRecord(), "nested"),
    (RemoteParentRecord(), "okay"), (RemoteChildRecord(), "remote")
] {{
    let key: Any? = "title"
    precondition(helper[key] as? String == expected)
    precondition(helper["missing"] == nil)
}}
let writer = WriterRecord()
(writer as ActiveRecordBase)["title"] = "ignored"
precondition(writer.written as? String == "yes")
print("source helper indexers passed")
"#
        ),
    )
    .unwrap();
    run(Command::new("swiftc")
        .args(["main.swift", "-o", "probe"])
        .arg("-module-cache-path")
        .arg(native.join("module-cache"))
        .current_dir(&native));
    run(Command::new(native.join("probe")).current_dir(&native));
}

fn scalar_cases() -> String {
    let source = br#"class ScalarCases
  def initialize
    @calls = 0
    @seen = ""
  end
  def calls
    @calls
  end
  def next_key(key)
    @calls = @calls + 1
    key
  end
  def classify(key)
    __rh_case_value = "preserved"
    case next_key(key)
    when :title then __rh_case_value
    when 9007199254740993 then "large"
    when 7 then "integer"
    when 2.5 then "float"
    when false then "false"
    when nil then "nil"
    else "default"
    end
  end
  def from_array(keys)
    case keys[0]
    when "title" then "array"
    else "default"
    end
  end
  def conditional(key)
    case true ? key : "other"
    when :title then "conditional"
    else "default"
    end
  end
  def statement(key)
    case key
    when :title then @seen = "statement"
    else @seen = "default"
    end
    @seen
  end
  def without_else(key)
    case key
    when :title then "title"
    end
  end
end
"#;
    let rbs = r#"class ScalarCases
  @calls: Integer
  @seen: String
  def initialize: () -> void
  def calls: () -> Integer
  def next_key: (untyped key) -> untyped
  def classify: (untyped key) -> String
  def from_array: (Array[untyped] keys) -> String
  def conditional: (untyped key) -> String
  def statement: (untyped key) -> String
  def without_else: (untyped key) -> (String | nil)
end
"#;
    let classes =
        roundhouse::runtime_src::parse_library_with_rbs(source, rbs, "scalar_cases.rb").unwrap();
    roundhouse::emit::swift::emit_library_class_result(&classes[0]).unwrap()
}

#[test]
fn swift_literal_patterns_preserve_once_only_evaluation_and_local_names() {
    let emitted = scalar_cases();
    assert_eq!(emitted.matches("nextKey(key)").count(), 1, "{emitted}");
    assert!(
        emitted.contains("case let __rhCaseValue_ where"),
        "{emitted}"
    );
    assert!(emitted.contains("return __rhCaseValue\n"), "{emitted}");
}

#[test]
#[ignore = "requires Swift; executes dynamic literal patterns and scrutinee effects"]
fn scalar_literal_patterns_compile_and_run_on_swift() {
    let scratch = emit_and_run::empty_app()
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .emit(BuildTarget::Swift)
        .0;
    let source = scalar_cases();
    std::fs::write(
        scratch.join("main.swift"),
        format!(
            r#"import Foundation
{source}
let probe = ScalarCases()
let cases: [(Any?, String)] = [
    ("title", "preserved"), (7, "integer"), (7.0, "integer"),
    (2.5, "float"), (false, "false"), (nil, "nil"),
    ("7", "default"), (true, "default"), ("missing", "default"),
    (9007199254740993, "large"), (9007199254740992.0, "default")
]
for (index, item) in cases.enumerated() {{
    precondition(probe.classify(item.0) == item.1)
    precondition(probe.calls == index + 1)
}}
precondition(probe.fromArray(["title"]) == "array")
precondition(probe.fromArray([7]) == "default")
precondition(probe.conditional("title") == "conditional")
precondition(probe.conditional(7) == "default")
precondition(probe.statement("title") == "statement")
precondition(probe.statement(7) == "default")
precondition(probe.withoutElse("title") == "title")
precondition(probe.withoutElse("missing") == nil)
let present: Any?? = .some(.some("title"))
precondition(probe.classify(present) == "preserved")
print("scalar literal patterns passed")
"#
        ),
    )
    .unwrap();
    run(Command::new("swiftc")
        .args(["main.swift", "-o", "probe"])
        .arg("-module-cache-path")
        .arg(scratch.join("module-cache"))
        .current_dir(&scratch));
    run(Command::new(scratch.join("probe")).current_dir(&scratch));
}

fn statement_cases() -> String {
    let source = br#"class StatementCases
  def initialize
    @calls = 0
    @seen = ""
  end
  def calls
    @calls
  end
  def next_key(key)
    @calls = @calls + 1
    key
  end
  def dispatch(key)
    __rh_case_value = "preserved"
    case next_key(key)
    when :other then @seen = "other"
    when "title" then @seen = __rh_case_value
    when 7 then @seen = "integer"
    when 2.0 then @seen = "float"
    when 9007199254740993 then @seen = "large"
    else @seen = "default"
    end
    @seen
  end
  def loop_case
    @seen = ""
    while true
      case "title"
      when "title" then break
      end
      @seen = "wrong"
      break
    end
    @seen
  end
  def unused_parameter(__rh_case_value, key)
    case key
    when "title" then @seen = "unused"
    else @seen = "default"
    end
    @seen
  end
  def later_local(key)
    case key
    when "title" then @seen = "late"
    else @seen = "default"
    end
    __rh_case_value = "unused"
    @seen
  end
end
"#;
    let rbs = r#"class StatementCases
  @calls: Integer
  @seen: String
  def initialize: () -> void
  def calls: () -> Integer
  def next_key: (untyped key) -> untyped
  def dispatch: (untyped key) -> String
  def loop_case: () -> String
  def unused_parameter: (untyped __rh_case_value, untyped key) -> String
  def later_local: (untyped key) -> String
end
"#;
    let classes =
        roundhouse::runtime_src::parse_library_with_rbs(source, rbs, "statement_cases.rb").unwrap();
    roundhouse::emit::csharp::emit_library_class_result(&classes[0]).unwrap()
}

#[test]
fn csharp_statement_patterns_preserve_once_only_evaluation_and_local_names() {
    let emitted = statement_cases();
    assert_eq!(emitted.matches("NextKey(key)").count(), 1, "{emitted}");
    assert!(
        emitted.contains("object.Equals(__rhCaseValue_,"),
        "{emitted}"
    );
}

#[test]
#[ignore = "requires .NET 8; executes value matching, effects and loop control"]
fn statement_literal_patterns_compile_and_run_on_csharp() {
    let scratch = emit_and_run::empty_app()
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .emit(BuildTarget::CSharp)
        .0;
    let source = statement_cases();
    std::fs::write(scratch.join("Native.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><Nullable>enable</Nullable><EnableDefaultCompileItems>false</EnableDefaultCompileItems></PropertyGroup><ItemGroup><Compile Include="Program.cs" /></ItemGroup></Project>"#).unwrap();
    std::fs::write(scratch.join("Program.cs"), format!(r#"using System;
using System.Collections.Generic;
namespace Roundhouse;
{source}
public static class Program {{
    public static void Main() {{
        var probe = new StatementCases();
        object key = new string("title".ToCharArray());
        if (probe.Dispatch(key) != "preserved" || probe.Calls() != 1L) throw new Exception("value/effect");
        if (probe.Dispatch("other") != "other" || probe.Calls() != 2L) throw new Exception("first arm");
        if (probe.Dispatch("missing") != "default" || probe.Calls() != 3L) throw new Exception("default/effect");
        foreach (var (value, expected) in new (object, string)[] {{
            (7L, "integer"), (7.0, "integer"), (2L, "float"), (2.0, "float"),
            (9007199254740993L, "large"), (9007199254740992.0, "default"),
            (false, "default"), (0L, "default")
        }}) if (probe.Dispatch(value) != expected) throw new Exception("scalar dispatch");
        if (probe.Calls() != 11L) throw new Exception("scalar effects");
        if (probe.LoopCase() != "") throw new Exception("loop break");
        if (probe.UnusedParameter("ignored", key) != "unused") throw new Exception("parameter hygiene");
        if (probe.LaterLocal(key) != "late") throw new Exception("later local hygiene");
        Console.WriteLine("statement literal patterns passed");
    }}
}}
"#)).unwrap();
    let dotnet = std::env::var("DOTNET").unwrap_or_else(|_| "dotnet".into());
    run(Command::new(dotnet)
        .args(["run", "--project", "Native.csproj"])
        .current_dir(&scratch));
}
