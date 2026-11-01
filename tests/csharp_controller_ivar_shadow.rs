//! #1625: an app controller's ivar named like the runtime
//! `ActionController::Base`'s own state (`@status`, `@location`,
//! `@headers`) is a property of the C# controller, which shares one
//! namespace with the runtime base's `Status`, `Location` and its
//! `Headers()` method: the app assigned its String to them, or declared a
//! `Headers` property beside the base's method. The app's ivar takes
//! another name in the emitted controller, as on TypeScript (#1614) and
//! Crystal (#1638). The build and a run are
//! `csharp_toolchain::controller_ivars_named_like_base_state_*`.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;
#[path = "support/ts_controller_ivars.rs"]
mod ts_controller_ivars;

use roundhouse::project::BuildTarget;

#[test]
fn an_app_ivar_shadowing_base_state_is_renamed_in_the_csharp_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let read = |p: &str| std::fs::read_to_string(emitted.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let probes = read("app/controllers/ProbesController.cs");
    let application = read("app/controllers/ApplicationController.cs");
    for (file, text) in [("probes", &probes), ("application", &application)] {
        for name in ["Status", "Location", "Headers"] {
            assert!(!text.contains(&format!("this.{name} =")), "{file} still assigns this.{name}:\n{text}");
            assert!(!text.contains(&format!("string {name} {{")), "{file} still declares {name}:\n{text}");
        }
    }
    assert!(probes.contains("this.AppStatus = \"app status\""), "{probes}");
    assert!(probes.contains("this.AppHeaders = \"app headers\""), "{probes}");
    assert!(application.contains("this.AppLocation = "), "{application}");
    assert!(probes.contains("Probes.Show(AppHeaders, AppLocation, AppStatus,"), "{probes}");
}

/// The runtime's `performed?` is `PerformedPred()` in C#, so an app
/// `@is_performed` (`IsPerformed`) names no runtime member and keeps its name.
#[test]
fn an_ivar_naming_no_csharp_base_member_keeps_its_name() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/ProbesController.cs")).unwrap();
    assert!(probes.contains("this.IsPerformed = \"app flag\""), "{probes}");
}

/// With no app/controllers/application_controller.rb, the C# emit writes
/// `public class ApplicationController : ActionControllerBase` itself; a
/// controller under it is still a controller, and its ivars are renamed.
#[test]
fn an_app_ivar_is_renamed_under_a_synthesized_application_controller() {
    let (emitted, errors) = ts_controller_ivars::overlay()
        .remove("app/controllers/application_controller.rb")
        .write("app/views/probes/show.html.erb", &ts_controller_ivars::SHOW.replace("<p><%= @location %></p>\n", ""))
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let probes = std::fs::read_to_string(emitted.join("app/controllers/ProbesController.cs")).unwrap();
    assert!(!probes.contains("this.Status ="), "probes still assigns this.Status:\n{probes}");
    assert!(probes.contains("this.AppStatus = \"app status\""), "{probes}");
}

/// ApplicationController's `set_location`, a `before_action` that isn't
/// inlined, is a value method in C# (`object? SetLocation()`). Its last
/// statement is an ivar assignment, which in Ruby answers the assigned
/// value, so the method returns that value (held once, since the ivar's
/// property can be wider than the method's return type); with no
/// `return` the C# doesn't build (CS0161).
#[test]
fn a_method_ending_in_an_ivar_assignment_returns_it() {
    let (emitted, errors) = ts_controller_ivars::overlay().emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let application = std::fs::read_to_string(emitted.join("app/controllers/ApplicationController.cs")).unwrap();
    assert!(
        application.contains("var __rhRet = \"app location\";\n        this.AppLocation = __rhRet;\n        return __rhRet;"),
        "{application}"
    );
}

/// `begin … rescue … else … end`: the else branch's value is the
/// block's, so a value method returns it and the body is run as
/// statements, whether it ends in an assignment or in a value.
#[test]
fn a_begin_with_an_else_returns_the_else_value() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", WIDGET)
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/Widget.cs")).unwrap();
    assert!(widget.contains("try {\n            this.Tag = 1L;\n            return 3L;\n        }"), "{widget}");
    assert!(widget.contains("ValueElse() {\n        try {\n            return 3L;\n        }"), "{widget}");
}

const WIDGET: &str = "class Widget < ApplicationRecord
  def assign_else
    begin
      @tag = 1
    rescue
      2
    else
      3
    end
  end

  def value_else
    begin
      1
    rescue
      2
    else
      3
    end
  end
end
";

/// An ivar a `String` in one method and an Integer in another is a
/// `string` property; the method ending in the Integer assignment returns
/// `long`, so it returns the value it assigned, not the property.
#[test]
fn a_method_returns_the_value_it_assigned_not_the_property() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def label\n    @name_tag = \"none\"\n  end\n\n  def renumber\n    @name_tag = 2\n  end\nend\n")
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/Widget.cs")).unwrap();
    assert!(widget.contains("public string NameTag"), "{widget}");
    assert!(widget.contains("long Renumber() {\n        var __rhRet = 2L;\n        this.NameTag = __rhRet;\n        return __rhRet;\n    }"), "{widget}");
}

/// A value expression in a `begin` body with an `else` is discarded but
/// still evaluated (`_ = n + 1L;`, C# takes no bare `n + 1L;`), and a
/// method ending in a local's assignment returns the assigned value, not
/// the local, which `x = nil` first makes `long?`.
#[test]
fn a_discarded_value_and_a_nil_first_local_compile_as_csharp() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  def plus_else\n    n = 1\n    begin\n      n + 1\n    rescue\n      2\n    else\n      3\n    end\n  end\n\n  def tally\n    x = nil\n    x = 1\n  end\nend\n")
        .emit(BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let widget = std::fs::read_to_string(emitted.join("app/models/Widget.cs")).unwrap();
    assert!(widget.contains("try {\n            _ = n + 1L;\n            return 3L;\n        }"), "{widget}");
    assert!(
        widget.contains("long? x = null;\n        var __rhRet = 1L;\n        x = __rhRet;\n        return __rhRet;"),
        "{widget}"
    );
}
