//! Dynamic arguments must survive the native boundary of String | Symbol APIs.
//! Compile unchanged generated runtime/controller files with a minimal host that
//! keeps ActionControllerBase's actual parameter storage type on each target.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;
use std::process::Command;

const CONTROLLER: &str = r#"class NoticesController < ApplicationController
  def show
    key = params[:kind]
    flash[key] = "message"
    render plain: flash[key].to_s
  end

  def inline
    flash[params[:kind]] = "inline"
    render plain: flash[params[:kind]].to_s
  end

  def aliased
    store = flash
    key = params[:kind]
    other = key
    store[other] = "aliased"
    render plain: store[other].to_s
  end

  def forwarded
    flash[dynamic_key] = "forwarded"
    render plain: flash[dynamic_key].to_s
  end

  def nested
    keys = [params[:kind]]
    flash[keys[0]] = "nested"
    render plain: flash[keys[0]].to_s
  end

  def selected
    key = params[:kind]
    flash[key.to_s == "notice" ? key : :alert] = "selected"
    render plain: flash[key.to_s == "notice" ? key : :alert].to_s
  end

  def stored
    store = session
    key = params[:kind]
    store[key] = "stored"
    render plain: store[key].to_s
  end

  def methods
    key = params[:kind]
    flash[key] = "methods"
    render plain: flash[key].to_s
  end

  private

  def dynamic_key
    params[:kind]
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/notices_controller.rb", CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn runtime_union_keys_keep_the_dynamic_native_boundary() {
    for (target, prefix, suffix, declarations) in [
        (
            BuildTarget::CSharp,
            "app/runtime",
            "cs",
            [
                "this[object? key]",
                "Fetch(object? key",
                "KeyPred(object? key",
                "HasKeyPred(object? key",
                "IncludePred(object? key",
                "Delete(object? key",
            ],
        ),
        (
            BuildTarget::Swift,
            "Sources/App",
            "swift",
            [
                "subscript(key: Any?)",
                "fetch(_ key: Any?",
                "keyPred(_ key: Any?",
                "hasKeyPred(_ key: Any?",
                "includePred(_ key: Any?",
                "delete(_ key: Any?",
            ],
        ),
    ] {
        let (emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        for class in ["Flash", "Session"] {
            let code = read(&emitted, &format!("{prefix}/{class}.{suffix}"));
            for declaration in declarations {
                assert!(
                    code.contains(declaration),
                    "{target:?} {class} lost {declaration}"
                );
            }
        }
        // FlashNow is emitted in the same file and must retain the same key ABI.
        let flash = read(&emitted, &format!("{prefix}/Flash.{suffix}"));
        assert_eq!(
            flash.matches(declarations[0]).count(),
            2,
            "{target:?}: FlashNow"
        );
    }
}

fn copy(root: &Path, dir: &Path, path: &str) {
    std::fs::copy(
        root.join(path),
        dir.join(Path::new(path).file_name().unwrap()),
    )
    .unwrap();
}

fn run(command: &mut Command) {
    let output = command.output().expect("native toolchain");
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires .NET 8; compiles generated runtime and controller files"]
fn parameter_derived_keys_compile_and_run_on_csharp() {
    let (emitted, errors) = app().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    for path in [
        "app/runtime/Flash.cs",
        "app/runtime/Session.cs",
        "app/controllers/NoticesController.cs",
    ] {
        copy(&emitted, &native, path);
    }
    std::fs::write(native.join("Native.csproj"), r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><Nullable>enable</Nullable></PropertyGroup></Project>"#).unwrap();
    std::fs::write(native.join("Host.cs"), CSHARP_HOST).unwrap();
    let dotnet = std::env::var("DOTNET").unwrap_or_else(|_| "dotnet".into());
    run(Command::new(dotnet)
        .args(["run", "--project", "Native.csproj"])
        .current_dir(&native));
}

#[test]
#[ignore = "requires Swift; compiles generated runtime and controller files"]
fn parameter_derived_keys_compile_and_run_on_swift() {
    let (emitted, errors) = app().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    for path in [
        "Sources/App/Flash.swift",
        "Sources/App/Session.swift",
        "Sources/App/runtime/RhString.swift",
        "Sources/App/app/controllers/NoticesController.swift",
    ] {
        copy(&emitted, &native, path);
    }
    std::fs::write(native.join("main.swift"), SWIFT_HOST).unwrap();
    run(Command::new("swiftc")
        .args([
            "Flash.swift",
            "Session.swift",
            "RhString.swift",
            "NoticesController.swift",
            "main.swift",
            "-o",
            "probe",
        ])
        .arg("-module-cache-path")
        .arg(native.join("module-cache"))
        .current_dir(&native));
    run(Command::new(native.join("probe")).current_dir(&native));
}

fn column_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :title, null: false\n    t.integer :visits, null: false\n  end\nend\n")
}

// Keep the generated accessor declarations and bodies byte-for-byte. The
// isolated host supplies only storage and inheritance, avoiding a full app build.
fn indexer(source: &str, needle: &str) -> String {
    let position = source.find(needle).expect("generated indexer");
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
    panic!("unterminated generated indexer");
}

#[test]
#[ignore = "requires .NET 8; compiles generated Base and model accessors"]
fn generated_column_accessors_compile_and_run_on_csharp() {
    let (emitted, errors) = column_app().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    let base = indexer(
        &read(&emitted, "app/runtime/ActiveRecordBase.cs"),
        " object? this[",
    );
    let model = indexer(&read(&emitted, "app/models/Widget.cs"), " object? this[");
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
public class ApplicationRecord : ActiveRecordBase {{}}
public class Widget : ApplicationRecord {{
    public long Id {{ get; set; }}
    public string Title {{ get; set; }} = "";
    public long Visits {{ get; set; }}
{model}
}}
public static class Program {{
    public static void Main() {{
        var model = new Widget();
        ActiveRecordBase record = model;
        var keys = new Dictionary<string, object?> {{ ["column"] = "title" }};
        object? key = keys.GetValueOrDefault("column");
        record[key] = "alice";
        if (!Equals(record[key], "alice")) throw new Exception("dynamic read/write");
        model["visits"] = 7L;
        if (!Equals(record["visits"], 7L)) throw new Exception("integer read/write");
        record["title"] = "changed";
        if (!Equals(model[key], "changed")) throw new Exception("base/model dispatch");
        if (model["missing"] != null) throw new Exception("missing key");
        Console.WriteLine("column accessors passed");
    }}
}}
"#
        ),
    )
    .unwrap();
    let dotnet = std::env::var("DOTNET").unwrap_or_else(|_| "dotnet".into());
    run(Command::new(dotnet)
        .args(["run", "--project", "Native.csproj"])
        .current_dir(&native));
}

#[test]
#[ignore = "requires Swift; compiles generated Base and model accessors"]
fn generated_column_accessors_compile_and_run_on_swift() {
    let (emitted, errors) = column_app().emit(BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let native = emitted.join("native-probe");
    std::fs::create_dir(&native).unwrap();
    copy(&emitted, &native, "Sources/App/runtime/RhString.swift");
    let base = indexer(
        &read(&emitted, "Sources/App/ActiveRecordBase.swift"),
        "subscript(",
    );
    let model = indexer(
        &read(&emitted, "Sources/App/app/models/Widget.swift"),
        "subscript(",
    );
    std::fs::write(
        native.join("main.swift"),
        format!(
            r#"import Foundation
class ActiveRecordBase {{
{base}
}}
class ApplicationRecord: ActiveRecordBase {{}}
class Widget: ApplicationRecord {{
    var id: Int = 0
    var title: String = ""
    var visits: Int = 0
{model}
}}
let model = Widget()
let record: ActiveRecordBase = model
let keys: [String: Any?] = ["column": "title"]
let key = keys["column"]
record[key] = "alice"
precondition(RhString.s(record[key]) == "alice")
model["visits"] = 7
precondition(record["visits"] as? Int == 7)
record["title"] = "changed"
precondition(RhString.s(model[key]) == "changed")
precondition(model["missing"] == nil)
print("column accessors passed")
"#
        ),
    )
    .unwrap();
    run(Command::new("swiftc")
        .args(["RhString.swift", "main.swift", "-o", "probe"])
        .arg("-module-cache-path")
        .arg(native.join("module-cache"))
        .current_dir(&native));
    run(Command::new(native.join("probe")).current_dir(&native));
}

const CSHARP_HOST: &str = r#"using System;
using System.Collections.Generic;
namespace Roundhouse;
public class ApplicationController {
    // Same storage as emitted ActionControllerBase; values are not Strings.
    public Dictionary<string, object?> Params { get; set; } = new();
    private Flash flash = new();
    private Session session = new();
    public string Body = "";
    public virtual void ProcessAction(string actionName) {}
    public void VerifyAuthenticityToken() {}
    public bool PerformedPred() => false;
    public Flash Flash() => flash;
    public Session Session() => session;
    public void Render(string body, string contentType = "text/html") { Body = body; }
    public virtual string ControllerName() => "";
    public virtual string ControllerPath() => "";
}
public static class Program {
    static void Check(bool value) { if (!value) throw new Exception("dynamic key contract"); }
    public static void Main() {
        foreach (var key in new[] { "notice", "alert" }) {
            var controller = new NoticesController();
            controller.Params["kind"] = key;
            foreach (var (action, expected) in new (Action, string)[] {
                (controller.Show, "message"), (controller.Inline, "inline"),
                (controller.Aliased, "aliased"), (controller.Forwarded, "forwarded"),
                (controller.Nested, "nested"), (controller.Selected, "selected"),
                (controller.Stored, "stored"),
                (controller.Methods, "methods")
            }) { action(); Check(controller.Body == expected); }
            object dynamicKey = key;
            var flash = controller.Flash();
            Check(flash.Fetch(dynamicKey) == "methods");
            Check(flash.KeyPred(dynamicKey) && flash.HasKeyPred(dynamicKey) && flash.IncludePred(dynamicKey));
            Check(flash.Delete(dynamicKey) == "methods");
            Check(flash.Fetch(dynamicKey, "fallback") == "fallback");
            flash.Now()[dynamicKey] = "now";
            Check(flash.Now()[dynamicKey] == "now");
            Check(!flash.ToPersisted().ContainsKey(key));
            var session = controller.Session();
            session[dynamicKey] = "session";
            Check(Equals(session.Fetch(dynamicKey), "session"));
            Check(session.KeyPred(dynamicKey) && session.HasKeyPred(dynamicKey) && session.IncludePred(dynamicKey));
            session.Delete(dynamicKey);
            Check(!session.KeyPred(dynamicKey));
            Check(Equals(session.Fetch(dynamicKey, "fallback"), "fallback"));
        }
        var absent = new NoticesController();
        absent.Show();
        Check(absent.Body == "");
        object? missing = absent.Params.GetValueOrDefault("missing");
        Check(absent.Flash().Fetch(missing, "fallback") == "fallback");
        absent.Session()[missing] = "empty-key";
        Check(Equals(absent.Session()[""], "empty-key"));
        Console.WriteLine("dynamic key contracts passed");
    }
}
"#;

const SWIFT_HOST: &str = r#"import Foundation
class ApplicationController {
    // Same storage as emitted ActionControllerBase; a lookup produces Any??.
    var params: [String: Any?] = [:]
    let flashStorage = Flash()
    let sessionStorage = Session()
    var body = ""
    func processAction(_ actionName: String) throws {}
    func verifyAuthenticityToken() {}
    func performedPred() -> Bool { false }
    func flash() -> Flash { flashStorage }
    func session() -> Session { sessionStorage }
    func render(_ body: String, _ status: String, _ contentType: String) { self.body = body }
    func controllerName() -> String { "" }
    func controllerPath() -> String { "" }
}
for key in ["notice", "alert"] {
    let controller = NoticesController()
    controller.params["kind"] = key
    let actions: [(()->Void, String)] = [
        (controller.show, "message"), (controller.inline, "inline"),
        (controller.aliased, "aliased"), (controller.forwarded, "forwarded"),
        (controller.nested, "nested"), (controller.selected, "selected"),
        (controller.stored, "stored"),
        (controller.methods, "methods")
    ]
    for (action, expected) in actions { action(); precondition(controller.body == expected) }
    let dynamicKey = controller.params["kind"]
    let flash = controller.flash()
    precondition(flash.fetch(dynamicKey) == "methods")
    precondition(flash.keyPred(dynamicKey) && flash.hasKeyPred(dynamicKey) && flash.includePred(dynamicKey))
    precondition(flash.delete(dynamicKey) == "methods")
    precondition(flash.fetch(dynamicKey, "fallback") == "fallback")
    flash.now()[dynamicKey] = "now"
    precondition(flash.now()[dynamicKey] == "now")
    precondition(flash.toPersisted()[key] == nil)
    let session = controller.session()
    session[dynamicKey] = "session"
    precondition(RhString.s(session.fetch(dynamicKey)) == "session")
    precondition(session.keyPred(dynamicKey) && session.hasKeyPred(dynamicKey) && session.includePred(dynamicKey))
    _ = session.delete(dynamicKey)
    precondition(!session.keyPred(dynamicKey))
    precondition(RhString.s(session.fetch(dynamicKey, "fallback")) == "fallback")
}
let absent = NoticesController()
absent.show()
precondition(absent.body == "")
let missing = absent.params["missing"]
precondition(absent.flash().fetch(missing, "fallback") == "fallback")
absent.session()[missing] = "empty-key"
precondition(RhString.s(absent.session()[""]) == "empty-key")
print("dynamic key contracts passed")
"#;
