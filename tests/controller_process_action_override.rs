//! Controller counterparts of probe-kit's own, parent and concern override
//! families. Compare callback traces with real Rails, with and without super.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const APPLICATION: &str = r#"
class ApplicationController < ActionController::API
  def trace
    @trace ||= ""
  end
  def mark(value)
    @trace = trace + value
  end
end
"#;
const ACTION: &str = r#"
  before_action :track_before
  after_action :track_after
  def index
    mark("action>")
    render plain: trace
  end
  private
  def track_before
    mark("before>")
  end
  def track_after
    mark("after>")
  end
"#;

fn wrapper(calls_super: bool, label: &str, bare: bool) -> String {
    let call = if !calls_super {
        "index"
    } else if bare {
        "super"
    } else {
        "super(action_name)"
    };
    format!(
        r#"
  private
  def process_action(action_name)
    mark("{label}>")
    {call}
    mark("{label}_tail>")
    "handled"
  end
"#
    )
}

fn files(owner: &str, calls_super: bool, bare: bool) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "db/schema.rb".into(),
            "ActiveRecord::Schema.define(version: 1) do\nend\n".into(),
        ),
        (
            "app/controllers/application_controller.rb".into(),
            APPLICATION.into(),
        ),
        (
            "config/routes.rb".into(),
            "Rails.application.routes.draw do\n get '/probe', to: 'probe#index'\nend\n".into(),
        ),
    ];
    let own = wrapper(calls_super, owner, bare);
    let mut action = ACTION;
    let (parent, include, own) = match owner {
        "own" => ("ApplicationController", "", own),
        "parent" => {
            files.push((
                "app/controllers/wrapper_parent_controller.rb".into(),
                format!("class WrapperParentController < ApplicationController\n{own}\nend\n"),
            ));
            ("WrapperParentController", "", String::new())
        }
        "concern" => {
            files.push((
                "app/controllers/concerns/action_wrapper.rb".into(),
                format!("module ActionWrapper\n extend ActiveSupport::Concern\n{own}\nend\n"),
            ));
            (
                "ApplicationController",
                "include ActionWrapper",
                String::new(),
            )
        }
        "chain" => {
            files.push((
                "app/controllers/wrapper_parent_controller.rb".into(),
                format!(
                    "class WrapperParentController < ApplicationController\n{}\nend\n",
                    wrapper(calls_super, "parent", bare)
                ),
            ));
            // An intervening class must not get a synthesized process_action
            // that hides the parent's application override.
            files.push((
                "app/controllers/middle_controller.rb".into(),
                "class MiddleController < WrapperParentController\nend\n".into(),
            ));
            ("MiddleController", "", wrapper(true, "own", bare))
        }
        "inherited_action" => {
            files.push((
                "app/controllers/action_parent_controller.rb".into(),
                format!("class ActionParentController < ApplicationController\n{ACTION}\nend\n"),
            ));
            files[2].1 = "Rails.application.routes.draw do\nget '/probe', to: 'probe#index'\nget '/parent', to: 'action_parent#index'\nend\n".into();
            action = "";
            ("ActionParentController", "", own)
        }
        _ => panic!("unknown owner"),
    };
    files.push((
        "app/controllers/probe_controller.rb".into(),
        format!("class ProbeController < {parent}\n{include}\n{action}\n{own}\nend\n"),
    ));
    files
}

fn fixture(files: &[(String, String)]) -> emit_and_run::Overlay {
    files
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, source)| {
            app.write(path, source)
        })
}

fn emitted_program() -> &'static str {
    r#"require_relative "app/controllers/probe_controller"
c = ProbeController.new
value = c._roundhouse_dispatch_action(:index)
puts [c.body, c.trace, value].join("|")
"#
}

fn ruby_command() -> Command {
    let mut ruby = Command::new(std::env::var("PROBE_RUBY").unwrap_or_else(|_| "ruby".into()));
    if let Ok(gems) = std::env::var("PROBE_GEM_HOME") {
        ruby.env("GEM_HOME", &gems).env("GEM_PATH", gems);
    }
    ruby
}

fn rails_oracle_available() -> bool {
    let script = format!(
        "{}\nexit(RailsOracle.available?(\"actionpack\") ? 0 : 1)\n",
        include_str!("support/rails_oracle.rb"),
    );
    ruby_command()
        .args(["-e", &script])
        .output()
        .is_ok_and(|output| output.status.success())
}

// Also checked against Rails by the CRuby gate. Native CI needs only
// this reference trace and the compiler, not a Rails installation.
fn reference(owner: &str, calls_super: bool) -> String {
    let prefix = if owner == "chain" {
        "own>parent>".to_string()
    } else {
        format!("{owner}>")
    };
    let body = format!(
        "{prefix}{}action>",
        if calls_super { "before>" } else { "" }
    );
    let tail = if owner == "chain" {
        "parent_tail>own_tail>".to_string()
    } else {
        format!("{owner}_tail>")
    };
    let trace = format!("{body}{}{tail}", if calls_super { "after>" } else { "" });
    format!("{body}|{trace}|handled\n")
}

fn require_rails_oracle() -> bool {
    if rails_oracle_available() {
        return true;
    }
    assert_ne!(
        std::env::var("ROUNDHOUSE_CONTROLLER_ORACLE_REQUIRED").as_deref(),
        Ok("1"),
        "the required Rails controller oracle is unavailable"
    );
    eprintln!("skipping controller oracle: the selected Rails actionpack is not installed");
    false
}

fn oracle(files: &[(String, String)]) -> String {
    oracle_program(
        files,
        "ProbeController",
        "puts [c.response.body, c.trace, value].join(\"|\")",
    )
}

fn oracle_program(files: &[(String, String)], controller: &str, output: &str) -> String {
    oracle_call(files, controller, ":index", output)
}

fn oracle_call(
    files: &[(String, String)],
    controller: &str,
    arguments: &str,
    output: &str,
) -> String {
    let mut script = String::from(include_str!("support/rails_oracle.rb"));
    script.push_str(
        r#"
RailsOracle.activate("actionpack")
require "action_controller"
require "action_dispatch/testing/test_request"
require "action_dispatch/testing/test_response"
"#,
    );
    for (_, source) in files.iter().filter(|(p, _)| p.starts_with("app/")) {
        script.push_str(source);
    }
    script.push_str(&format!(
        r#"
c = {controller}.new
c.set_request!(ActionDispatch::TestRequest.create)
c.set_response!(ActionDispatch::TestResponse.new)
value = c.process({arguments})
{output}
"#
    ));
    let result = ruby_command()
        .args(["-e", &script])
        .output()
        .expect("Rails oracle");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

fn run_emitted(files: &[(String, String)]) -> String {
    run_emitted_program(files, emitted_program())
}

fn run_emitted_program(files: &[(String, String)], program: &str) -> String {
    let (emitted, errors) = fixture(files).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let script = format!("require_relative 'boot'\n{program}");
    let result = ruby_command()
        .args(["-e", &script])
        .current_dir(&emitted.as_ref())
        .output()
        .expect("emitted Ruby");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn rails_oracle_own_parent_and_concern_overrides_match_rails_with_and_without_super() {
    if !rails_oracle_available() {
        assert_ne!(
            std::env::var("ROUNDHOUSE_CONTROLLER_ORACLE_REQUIRED").as_deref(),
            Ok("1"),
            "the required Rails controller oracle is unavailable"
        );
        eprintln!("skipping controller oracle: the selected Rails actionpack is not installed");
        return;
    }
    for owner in ["own", "parent", "concern", "chain", "inherited_action"] {
        for calls_super in [false, true] {
            for bare in [false, true] {
                let files = files(owner, calls_super, bare);
                let expected = oracle(&files);
                assert_eq!(
                    expected,
                    reference(owner, calls_super),
                    "Rails reference for {owner}"
                );
                assert_eq!(
                    expected.contains("before>"),
                    calls_super,
                    "{owner}: {expected}"
                );
                assert_eq!(
                    expected.contains("after>"),
                    calls_super,
                    "{owner}: {expected}"
                );
                assert!(expected.ends_with("|handled\n"), "{expected}");
                assert_eq!(
                    run_emitted(&files),
                    expected,
                    "{owner}, super={calls_super}, bare={bare}"
                );
            }
        }
    }
}

#[test]
fn an_inherited_override_keeps_the_python_controller_routable() {
    let (emitted, errors) = fixture(&files("parent", true, false)).emit(BuildTarget::Python);
    assert!(errors.is_empty(), "{errors:?}");
    let registry = std::fs::read_to_string(emitted.as_ref().join("app/v2/dispatch.py")).unwrap();
    assert!(
        registry.contains("\"probe\": ProbeController"),
        "{registry}"
    );
    // Execute the emitted parent hook against a child dispatcher. Isolate
    // lookup from the rest of the Python runtime: a double-leading
    // underscore would bind the call to the parent's mangled name.
    let result = Command::new("python3")
        .args([
            "-c",
            r#"
import ast
import pathlib
import textwrap
tree = ast.parse(pathlib.Path('app/v2/controllers.py').read_text())
classes = {c.name: c for c in tree.body if isinstance(c, ast.ClassDef)}
hook = next(m for m in classes['WrapperParentController'].body
            if isinstance(m, ast.FunctionDef) and m.name == 'process_action')
dispatch = next(m for m in classes['ProbeController'].body
                if isinstance(m, ast.FunctionDef) and m.name.endswith('roundhouse_process_action'))
program = ('from __future__ import annotations\nclass WrapperParentController:\n'
           + textwrap.indent(ast.unparse(hook), '    ')
           + '\nclass ProbeController(WrapperParentController):\n'
           + '    def mark(self, value): pass\n'
           + '    def ' + dispatch.name + '(self, action_name): self.dispatched = action_name\n'
           + 'c = ProbeController()\n'
           + 'assert c.process_action("index") == "handled"\n'
           + 'assert c.dispatched == "index"\n')
exec(program)
"#,
        ])
        .current_dir(emitted.as_ref())
        .output()
        .expect("Python inherited hook");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn synthetic_repro_has_one_source_override_and_a_separate_dispatcher() {
    let files = vec![
        (
            "db/schema.rb".into(),
            "ActiveRecord::Schema.define(version: 1) do\nend\n".into(),
        ),
        (
            "config/routes.rb".into(),
            "Rails.application.routes.draw do\nget '/example', to: 'toy_endpoint#index'\nend\n"
                .into(),
        ),
        (
            "app/controllers/application_controller.rb".into(),
            "class ApplicationController < ActionController::API\nend\n".into(),
        ),
        (
            "app/controllers/toy_endpoint_controller.rb".into(),
            r#"class ToyEndpointController < ApplicationController
  def index
    render json: { ok: true }
  end
  private
  def process_action(action_name)
    index
    "handled"
  end
end
"#
            .into(),
        ),
    ];
    let (emitted, errors) = fixture(&files).emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let rb = std::fs::read_to_string(
        emitted
            .as_ref()
            .join("app/controllers/toy_endpoint_controller.rb"),
    )
    .unwrap();
    let rbs = std::fs::read_to_string(
        emitted
            .as_ref()
            .join("app/controllers/toy_endpoint_controller.rbs"),
    )
    .unwrap();
    assert_eq!(rb.matches("def process_action(").count(), 1, "{rb}");
    assert_eq!(
        rb.matches("def _roundhouse_process_action(").count(),
        1,
        "{rb}"
    );
    assert_eq!(rbs.matches("def process_action:").count(), 1, "{rbs}");
    assert_eq!(
        rbs.matches("def _roundhouse_process_action:").count(),
        1,
        "{rbs}"
    );
    let (ruby_tree, ruby_errors) = fixture(&files).emit(BuildTarget::Ruby);
    assert!(ruby_errors.is_empty(), "{ruby_errors:?}");
    let result = ruby_command().args(["-e", "require_relative 'boot'; require_relative 'app/controllers/toy_endpoint_controller'; c = ToyEndpointController.new; puts c.process_action(:index); puts c.body"])
        .current_dir(&ruby_tree.as_ref()).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "handled\n{\"ok\":true}\n"
    );
}

#[test]
fn reserved_dispatcher_collision_is_an_emit_error() {
    let mut files = files("own", false, false);
    files.push((
        "app/controllers/reserved_controller.rb".into(),
        r#"class ReservedController < ApplicationController
  def process_action(action_name)
    "own"
  end
  def _roundhouse_process_action(action_name)
    "reserved"
  end
end
"#
        .into(),
    ));
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, s)| (p.into(), s.into_bytes()))
        .collect();
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diagnostics.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("controller_dispatch_collision")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_hidden_concern_super_link_is_refused() {
    let mut files = files("own", true, false);
    let module = format!(
        "module ActionWrapper\n extend ActiveSupport::Concern\n{}\n def unrelated_helper\n \"helper\"\n end\nend\n",
        wrapper(true, "concern", false)
    );
    files.push(("app/controllers/concerns/action_wrapper.rb".into(), module));
    let (_, controller) = files
        .iter_mut()
        .find(|(p, _)| p.ends_with("probe_controller.rb"))
        .unwrap();
    *controller = controller.replacen(
        "< ApplicationController\n",
        "< ApplicationController\ninclude ActionWrapper\n",
        1,
    );
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
        .collect();
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error
                && d.message.contains("process_action_concern_super")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_generated_parent_method_cannot_collide_with_the_internal_dispatcher() {
    for definition in [
        "attr_reader :_roundhouse_process_action",
        "alias_method :_roundhouse_process_action, :trace",
    ] {
        let mut files = files("own", false, false);
        files[1].1 = files[1]
            .1
            .replacen("\nend\n", &format!("\n  {definition}\nend\n"), 1);
        let tree = files
            .into_iter()
            .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
            .collect();
        let mut app = ingest_app_from_tree(tree).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::emit::ruby::emit_lowered_controllers(&app)
        });
        assert!(
            diagnostics.iter().any(|d| d.severity == Severity::Error
                && d.message.contains("controller_dispatch_collision")),
            "{definition}: {diagnostics:?}"
        );
    }
}

#[test]
fn unretained_alias_and_implicit_rest_super_chains_are_refused() {
    for (definition, diagnostic) in [
        (
            "def wrapper(action_name)\nindex\nend\nalias_method :process_action, :wrapper",
            "process_action_alias",
        ),
        (
            "def process_action(*args)\nsuper\nend",
            "process_action_super",
        ),
    ] {
        let mut files = files("own", false, false);
        let (_, controller) = files
            .iter_mut()
            .find(|(p, _)| p.ends_with("probe_controller.rb"))
            .unwrap();
        *controller = controller.replace(
            &wrapper(false, "own", false),
            &format!("\nprivate\n{definition}\n"),
        );
        let tree = files
            .into_iter()
            .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
            .collect();
        let mut app = ingest_app_from_tree(tree).unwrap();
        roundhouse::session::analyze_and_lower(&mut app);
        let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
            roundhouse::emit::ruby::emit_lowered_controllers(&app)
        });
        assert!(
            diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains(diagnostic)),
            "{definition}: {diagnostics:?}"
        );
    }
}

#[test]
fn an_aliased_concern_hook_cannot_change_the_name_used_by_super() {
    let mut files = files("concern", true, false);
    let (_, module) = files
        .iter_mut()
        .find(|(p, _)| p.ends_with("action_wrapper.rb"))
        .unwrap();
    *module = module
        .replace(
            "def process_action(action_name)",
            "def wrapper(action_name)",
        )
        .replace(
            "\nend\n",
            "\n  alias_method :process_action, :wrapper\nend\n",
        );
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.into_bytes()))
        .collect();
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let (_, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error && d.message.contains("process_action_alias")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_remaining_method_collision_refuses_both_the_class_and_its_sidecar() {
    let files = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nget '/new', to: 'probe#new'\nget '/other', to: 'probe#new_action'\nend\n",
        ),
        ("app/controllers/application_controller.rb", APPLICATION),
        (
            "app/controllers/probe_controller.rb",
            r#"class ProbeController < ApplicationController
  def new
    render plain: "new"
  end
  def new_action
    render plain: "other"
  end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).unwrap();
    roundhouse::session::analyze_and_lower(&mut app);
    let (emitted, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::emit::ruby::emit_lowered_controllers(&app)
    });
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error
                && d.message.contains("controller_method_collision")),
        "{diagnostics:?}"
    );
    assert!(!emitted.iter().any(
        |f| f.path.ends_with("probe_controller.rb") || f.path.ends_with("probe_controller.rbs")
    ));
}

fn parent_order_files(around_first: bool) -> Vec<(String, String)> {
    let mut files = files("inherited_action", false, false);
    let (_, parent) = files
        .iter_mut()
        .find(|(p, _)| p.ends_with("action_parent_controller.rb"))
        .unwrap();
    *parent = parent.replace("  after_action :track_after\n", "");
    let around = "  around_action :track_around\n";
    *parent = parent.replace(
        "  before_action :track_before\n",
        &if around_first {
            format!("{around}  before_action :track_before\n")
        } else {
            format!("  before_action :track_before\n{around}")
        },
    );
    *parent = parent.replacen("  private\n", "  private\n  def track_around\n    mark(\"around>\")\n    yield\n    mark(\"around_tail>\")\n  end\n", 1);
    files
}

#[test]
fn rails_oracle_a_descendant_hook_does_not_reorder_its_unwrapped_parents_callbacks() {
    if !require_rails_oracle() {
        return;
    }
    for around_first in [true, false] {
        let files = parent_order_files(around_first);
        let expected = oracle_program(&files, "ActionParentController", "puts c.trace");
        assert_eq!(
            expected,
            if around_first {
                "around>before>action>around_tail>\n"
            } else {
                "before>around>action>around_tail>\n"
            }
        );
        assert_eq!(
            run_emitted_program(
                &files,
                "require_relative 'app/controllers/action_parent_controller'\nc = ActionParentController.new\nc.process_action(:index)\nputs c.trace"
            ),
            expected
        );
        // The direct inherited action still bypasses every callback.
        assert_eq!(run_emitted(&files), oracle(&files));
    }
}

fn review_files(owner: &str, hook: &str, action: &str, params: &str) -> Vec<(String, String)> {
    let mut input = files(owner, true, false);
    let original = wrapper(true, owner, false);
    let hook = format!("\n  private\n  def process_action({params})\n{hook}\n  end\n");
    for (_, source) in &mut input {
        *source = source.replace(&original, &hook).replace(ACTION, action);
    }
    input
}

fn assert_refused(files: &[(String, String)], code: &str) {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (_, errors) = fixture(files).emit(target);
        assert!(
            errors.iter().any(|e| e.contains(code)),
            "{target:?}: {errors:?}"
        );
    }
}

struct ReviewCase {
    name: String,
    files: Vec<(String, String)>,
    expected: String,
}

fn review_cases() -> Vec<ReviewCase> {
    let action = ACTION
        .replace(
            "    render plain: trace",
            "    render plain: trace\n    \"action-result\"",
        )
        .replace(
            "    mark(\"after>\")",
            "    mark(\"after>\")\n    \"after-result\"",
        );
    let mut cases = Vec::new();
    for owner in ["own", "parent", "concern"] {
        for (label, hook) in [
            ("explicit", "    super(action_name)"),
            ("bare", "    super"),
            ("no_parens", "    super action_name"),
            ("literal_string", "    super(\"index\")"),
            ("literal_symbol", "    super(:index)"),
        ] {
            for callbacks in [true, false] {
                let source = if callbacks {
                    action.clone()
                } else {
                    action
                        .replace("  before_action :track_before\n", "")
                        .replace("  after_action :track_after\n", "")
                };
                cases.push(ReviewCase {
                    name: format!("{owner}/{label}/callbacks={callbacks}"),
                    files: review_files(owner, hook, &source, "action_name"),
                    expected: if callbacks {
                        "before>action>|before>action>after>|action-result\n"
                    } else {
                        "action>|action>|action-result\n"
                    }
                    .into(),
                });
            }
        }
        cases.push(ReviewCase {
            name: format!("{owner}/string_branch_no_super"),
            files: review_files(owner,
                "    if action_name == \"index\"\n      index\n      \"handled\"\n    else\n      super\n      \"fallback\"\n    end", &action, "action_name"),
            expected: "action>|action>|handled\n".into(),
        });
        cases.push(ReviewCase {
            name: format!("{owner}/request_selector_type"),
            files: review_files(
                owner,
                "    mark(action_name.class.to_s + \">\")\n    super(action_name)\n    \"handled\"",
                &action,
                "action_name",
            ),
            expected: "String>before>action>|String>before>action>after>|handled\n".into(),
        });
        cases.push(ReviewCase {
            name: format!("{owner}/explicit_action_return"),
            files: review_files(
                owner,
                "    super(action_name)",
                &action.replace("    \"action-result\"", "    return \"action-result\""),
                "action_name",
            ),
            expected: "before>action>|before>action>after>|action-result\n".into(),
        });
        cases.push(ReviewCase {
            name: format!("{owner}/implicit_render_preserves_result"),
            files: review_files(
                owner,
                "    super",
                "  def index\n    \"action-result\"\n  end\n",
                "action_name",
            ),
            expected: "||action-result\n".into(),
        });
    }
    for (name, action, expected) in [
        (
            "render_result",
            "  def index\n    render plain: \"render-result\"\n  end\n",
            "render-result||render-result\n",
        ),
        (
            "head_result",
            "  def index\n    head :ok\n  end\n",
            "||true\n",
        ),
        (
            "redirect_result",
            "  def index\n    redirect_to \"/other\"\n  end\n",
            "||302\n",
        ),
    ] {
        cases.push(ReviewCase {
            name: name.into(),
            files: review_files("own", "    super", action, "action_name"),
            expected: expected.into(),
        });
    }
    let switched_action = action.replace("before_action :track_before", "before_action :track_before, only: :index")
        .replace("after_action :track_after", "after_action :track_after, only: :index")
        .replace("  private\n", "  def show\n    mark(\"show>\")\n    render plain: trace\n    \"show-result\"\n  end\n  private\n");
    let mut switched = review_files(
        "own",
        "    super(\"show\")",
        &switched_action,
        "action_name",
    );
    let routes = &mut switched
        .iter_mut()
        .find(|(p, _)| p == "config/routes.rb")
        .unwrap()
        .1;
    *routes = routes.replacen("end\n", " get '/show', to: 'probe#show'\nend\n", 1);
    cases.push(ReviewCase {
        name: "changed_selector_keeps_request_callback_scopes".into(),
        files: switched,
        expected: "before>show>|before>show>after>|show-result\n".into(),
    });
    let mut chain = review_files(
        "parent",
        "    mark(\"parent:\" + action_name.class.to_s + \">\")\n    super(action_name)",
        &action,
        "action_name",
    );
    let probe = &mut chain
        .iter_mut()
        .find(|(p, _)| p.ends_with("probe_controller.rb"))
        .unwrap()
        .1;
    let end = probe.rfind("\nend\n").unwrap();
    probe.insert_str(end, "\n  private\n  def process_action(action_name)\n    mark(\"own:\" + action_name.class.to_s + \">\")\n    super(:index)\n  end\n");
    cases.push(ReviewCase { name: "source_parent_super_retains_its_explicit_symbol_argument".into(), files: chain,
        expected: "own:String>parent:Symbol>before>action>|own:String>parent:Symbol>before>action>after>|action-result\n".into() });
    let around = "  around_action :track_around\n  def track_around\n    mark(\"around>\")\n    yield\n    mark(\"around_tail>\")\n    \"around-result\"\n  end\n";
    cases.push(ReviewCase {
        name: "around_result_does_not_replace_action_result".into(),
        files: review_files(
            "own",
            "    super",
            &(around.to_string() + &action.replace("  after_action :track_after\n", "")),
            "action_name",
        ),
        expected: "around>before>action>|around>before>action>around_tail>|action-result\n".into(),
    });
    cases.push(ReviewCase {
        name: "around_halt_result".into(),
        files: review_files(
            "own",
            "    super",
            &(around.to_string()
                + &action.replace("  after_action :track_after\n", "").replace(
                    "    mark(\"before>\")",
                    "    mark(\"before>\")\n    render plain: trace",
                )),
            "action_name",
        ),
        expected: "around>before>|around>before>around_tail>|false\n".into(),
    });
    cases.push(ReviewCase {
        name: "halt_result".into(),
        files: review_files(
            "own",
            "    super",
            &action.replace(
                "    mark(\"before>\")",
                "    mark(\"before>\")\n    render plain: trace",
            ),
            "action_name",
        ),
        expected: "before>|before>|false\n".into(),
    });
    cases.push(ReviewCase {
        name: "rescue_result".into(),
        files: review_files("own", "    super", &format!("  rescue_from StandardError, with: :handle_error\n{}\n  def handle_error(error)\n    mark(\"rescued>\")\n    render plain: trace\n    \"rescued-result\"\n  end\n", action.replace("    mark(\"action>\")", "    raise StandardError, \"failed\"")), "action_name"),
        expected: "before>rescued>|before>rescued>|failed\n".into(),
    });
    // super() remains an ordinary source-parent call until the final
    // framework boundary: the parent's default is a valid selector.
    let mut input = review_files(
        "parent",
        "    mark(\"parent>\")\n    super(action_name)",
        &action,
        "action_name = :index",
    );
    let probe = &mut input
        .iter_mut()
        .find(|(p, _)| p.ends_with("probe_controller.rb"))
        .unwrap()
        .1;
    let end = probe.rfind("\nend\n").unwrap();
    probe.insert_str(
        end,
        "\n  private\n  def process_action(action_name)\n    super()\n  end\n",
    );
    cases.push(ReviewCase {
        name: "source_parent_default_super_empty".into(),
        files: input,
        expected: "parent>before>action>|parent>before>action>after>|action-result\n".into(),
    });
    cases
}

#[test]
fn rails_oracle_review_selector_and_return_examples_match_the_rails_oracle() {
    if !require_rails_oracle() {
        return;
    }
    for case in review_cases() {
        let expected = oracle(&case.files);
        assert_eq!(expected, case.expected, "Rails reference: {}", case.name);
        assert_eq!(run_emitted(&case.files), expected, "{}\n{}", case.name, {
            let (tree, _) = fixture(&case.files).emit(BuildTarget::Ruby);
            std::fs::read_to_string(tree.as_ref().join("app/controllers/probe_controller.rb"))
                .unwrap()
        });
        let (tree, errors) = fixture(&case.files).emit(BuildTarget::Spinel);
        assert!(errors.is_empty(), "{}: {errors:?}", case.name);
        let sidecar =
            std::fs::read_to_string(tree.as_ref().join("app/controllers/probe_controller.rbs"))
                .unwrap();
        let dispatch = sidecar
            .lines()
            .find(|s| s.contains("def _roundhouse_process_action:"))
            .unwrap();
        assert!(dispatch.contains("(String "), "{}: {dispatch}", case.name);
        if !case.name.contains("halt_result") {
            assert!(
                dispatch.contains("String") && !dispatch.ends_with("-> nil"),
                "{}: {dispatch}",
                case.name
            );
        }
    }
}

#[test]
fn rails_oracle_a_hook_owning_ancestor_cannot_hide_a_user_defined_internal_name() {
    if !require_rails_oracle() {
        return;
    }
    for name in ["_roundhouse_process_action", "_roundhouse_dispatch_action"] {
        let mut input = review_files(
            "parent",
            &format!("    {name}\n    index\n    \"handled\""),
            ACTION,
            "action_name",
        );
        let source = &mut input
            .iter_mut()
            .find(|(p, _)| p.ends_with("wrapper_parent_controller.rb"))
            .unwrap()
            .1;
        *source = source.replace(
            "  private\n",
            &format!("  attr_reader :{name}\n  private\n"),
        );
        assert_eq!(oracle(&input), "action>|action>|handled\n");
        assert_refused(&input, "controller_dispatch_collision");
        let source = &mut input
            .iter_mut()
            .find(|(p, _)| p.ends_with("wrapper_parent_controller.rb"))
            .unwrap()
            .1;
        *source = source.replace(
            &format!("  attr_reader :{name}\n"),
            &format!("  def self.{name}\n    nil\n  end\n  delegate :{name}, to: :class\n"),
        );
        assert_eq!(oracle(&input), "action>|action>|handled\n");
        assert_refused(&input, "controller_dispatch_collision");
    }
    // An application with no source hook needs no reserved dispatcher:
    // the same delegate declaration must not acquire a new diagnostic.
    let mut control = files("own", false, false);
    for (_, source) in &mut control {
        *source = source.replace(&wrapper(false, "own", false), "");
    }
    control[1].1 = control[1].1.replacen("\nend\n", "\n  def self._roundhouse_process_action\n    nil\n  end\n  delegate :_roundhouse_process_action, to: :class\nend\n", 1);
    let expected = oracle_program(
        &control,
        "ProbeController",
        "puts [c.response.body, c.trace].join(\"|\")",
    );
    assert_eq!(
        run_emitted_program(
            &control,
            "require_relative 'app/controllers/probe_controller'\nc = ProbeController.new\nc.process_action(:index)\nputs [c.body, c.trace].join(\"|\")"
        ),
        expected
    );
}

fn inherited_alias_files(name: &str, declaration: &str) -> Vec<(String, String)> {
    let mut input = review_files(
        "parent",
        &format!("    {name}\n    index\n    \"handled\""),
        ACTION,
        "action_name",
    );
    let parent = &mut input
        .iter_mut()
        .find(|(p, _)| p.ends_with("wrapper_parent_controller.rb"))
        .unwrap()
        .1;
    *parent = parent.replacen("  private\n", &format!("  {declaration}\n  private\n"), 1);
    input
}

#[test]
fn rails_oracle_inherited_alias_destinations_cannot_hide_reserved_dispatch_methods() {
    if !require_rails_oracle() {
        return;
    }
    for name in ["_roundhouse_process_action", "_roundhouse_dispatch_action"] {
        for declaration in [
            format!("alias_method :{name}, :trace"),
            format!("alias {name} trace"),
            format!("alias_method \"{name}\", \"trace\""),
        ] {
            let input = inherited_alias_files(name, &declaration);
            assert_eq!(oracle(&input), "action>|action>|handled\n", "{declaration}");
            if declaration.starts_with("alias ") {
                // Keyword aliases already fail ingest. Do not ask the emit
                // helper (which requires successful ingest) to check them.
                let tree = input
                    .iter()
                    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
                    .collect();
                let error = ingest_app_from_tree(tree)
                    .err()
                    .expect("keyword alias must refuse ingest");
                assert!(
                    format!("{error:?}").contains("AliasMethodNode"),
                    "{error:?}"
                );
            } else {
                assert_refused(&input, "controller_dispatch_collision");
            }
        }
    }
}

#[test]
fn rails_oracle_unrouted_framework_super_selectors_are_refused() {
    if !require_rails_oracle() {
        return;
    }
    let action = ACTION.replace("  private\n", "  def show\n    mark(\"show>\")\n    render plain: trace\n    \"show-result\"\n  end\n  private\n");
    for owner in ["own", "parent", "concern"] {
        for selector in ["\"show\"", ":show"] {
            let input = review_files(
                owner,
                &format!("    super({selector})"),
                &action,
                "action_name",
            );
            assert_eq!(
                oracle(&input),
                "before>show>|before>show>after>|show-result\n"
            );
            assert_refused(&input, "process_action_super");
        }
        // The same selector is supported when this receiving controller has
        // the dispatch arm. A parent's own (possibly empty) route set is not
        // the contract of an inherited framework-super call.
        let mut input = review_files(owner, "    super(\"show\")", &action, "action_name");
        let routes = &mut input
            .iter_mut()
            .find(|(p, _)| p == "config/routes.rb")
            .unwrap()
            .1;
        *routes = routes.replacen("end\n", " get '/show', to: 'probe#show'\nend\n", 1);
        assert_eq!(run_emitted(&input), oracle(&input));
    }
    // Check the selected value through an ordinary source-parent call too.
    let mut input = review_files("parent", "    super(action_name)", &action, "action_name");
    let probe = &mut input
        .iter_mut()
        .find(|(p, _)| p.ends_with("probe_controller.rb"))
        .unwrap()
        .1;
    let end = probe.rfind("\nend\n").unwrap();
    probe.insert_str(
        end,
        "\n  private\n  def process_action(action_name)\n    super(\"show\")\n  end\n",
    );
    assert_eq!(
        oracle(&input),
        "before>show>|before>show>after>|show-result\n"
    );
    assert_refused(&input, "process_action_super");
}

#[test]
fn rails_oracle_unsupported_framework_super_selector_types_are_refused() {
    if !require_rails_oracle() {
        return;
    }
    for owner in ["own", "parent", "concern"] {
        for call in [
            "super(123)",
            "super(nil)",
            "super(Object.new)",
            "action_name = 123; super(action_name)",
            "[123].each { |action_name| super(action_name) }",
        ] {
            let input = review_files(
                owner,
                &format!(
                    "    begin\n      {call}\n    rescue TypeError\n      render plain: trace\n      \"rescued\"\n    end"
                ),
                ACTION,
                "action_name",
            );
            assert_eq!(
                oracle(&input),
                "before>|before>|rescued\n",
                "{owner}: {call}"
            );
            assert_refused(&input, "process_action_super");
        }
        // A dynamic helper's supported selector type/value is not proven at
        // this boundary, even if this particular Rails execution succeeds.
        let action = ACTION.replace(
            "  private\n",
            "  def selected_action\n    \"index\"\n  end\n  private\n",
        );
        let input = review_files(owner, "    super(selected_action)", &action, "action_name");
        assert_eq!(
            oracle(&input),
            "before>action>|before>action>after>|before>action>\n"
        );
        assert_refused(&input, "process_action_super");
    }
    let mut input = review_files("parent", "    super(action_name)", ACTION, "action_name");
    let probe = &mut input
        .iter_mut()
        .find(|(p, _)| p.ends_with("probe_controller.rb"))
        .unwrap()
        .1;
    let end = probe.rfind("\nend\n").unwrap();
    probe.insert_str(end, "\n  private\n  def process_action(action_name)\n    begin\n      super(123)\n    rescue TypeError\n      render plain: trace\n      \"rescued\"\n    end\n  end\n");
    assert_eq!(oracle(&input), "before>|before>|rescued\n");
    assert_refused(&input, "process_action_super");
}

#[test]
fn rails_oracle_unsupported_framework_super_argument_shapes_have_oracle_refusal_controls() {
    if !require_rails_oracle() {
        return;
    }
    let action = ACTION
        .replace("def index", "def index(value = \"default\")")
        .replace("mark(\"action>\")", "mark(value)");
    for owner in ["own", "parent", "concern"] {
        for (params, hook, trace) in [
            (
                "action_name",
                "    super(action_name, \"forwarded\")",
                "before>forwarded|before>forwardedafter>|",
            ),
            (
                "action_name, value = \"forwarded\"",
                "    super",
                "before>forwarded|before>forwardedafter>|",
            ),
            (
                "action_name, *args",
                "    super(action_name, *args)",
                "before>default|before>defaultafter>|",
            ),
            (
                "action_name",
                "    super(*[action_name])",
                "before>default|before>defaultafter>|",
            ),
        ] {
            let input = review_files(owner, hook, &action, params);
            let expected = oracle(&input);
            if hook.contains("*args") {
                let forwarded = oracle_call(
                    &input,
                    "ProbeController",
                    ":index, \"forwarded\"",
                    "puts [c.response.body, c.trace, value].join(\"|\")",
                );
                assert!(
                    forwarded.starts_with("before>forwarded|before>forwardedafter>|"),
                    "{forwarded}"
                );
            }
            assert!(expected.starts_with(trace), "{owner}, {hook}: {expected}");
            assert_refused(&input, "process_action_super");
        }
        let input = review_files(
            owner,
            "    begin\n      super()\n    rescue ArgumentError\n      render plain: trace\n      \"rescued\"\n    end",
            ACTION,
            "action_name",
        );
        assert_eq!(oracle(&input), "before>|before>|rescued\n");
        assert_refused(&input, "process_action_super");
    }
}

#[test]
fn rails_oracle_request_dispatch_passes_the_rails_string_selector_to_a_source_hook() {
    if !require_rails_oracle() {
        return;
    }
    let input = review_files(
        "own",
        "    mark(action_name.class.to_s + \">\")\n    index\n    \"handled\"",
        ACTION,
        "action_name",
    );
    let expected = oracle_program(&input, "ProbeController", "puts c.response.body");
    assert_eq!(expected, "String>action>\n");
    let actual = run_emitted_program(
        &input,
        r#"require_relative 'main'
require 'stringio'
Db.configure(':memory:')
ActiveRecord.adapter = SqliteAdapter
env = { 'REQUEST_METHOD' => 'GET', 'PATH_INFO' => '/probe', 'QUERY_STRING' => '',
        'SERVER_NAME' => 'localhost', 'SERVER_PORT' => '80', 'rack.url_scheme' => 'http' }
status, body, content_type, location, cookies, headers = Main.dispatch_core(env, StringIO.new(''))
raise status.to_s unless status == 200
puts body
"#,
    );
    assert_eq!(actual, expected);
    let (tree, errors) = fixture(&input).emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let harness = std::fs::read_to_string(tree.as_ref().join("test/test_helper.rb")).unwrap();
    assert!(
        harness.contains("matched.controller == :probe"),
        "Ruby test requests must retain lazy controller loading"
    );
    assert!(
        harness.contains("controller._roundhouse_dispatch_action(action)"),
        "request tests must use the same adapter"
    );
}

// Keep native CI to four representative builds. The full family/value table
// remains in the required Rails/Ruby comparisons and Spinel emit checks above.
#[test]
#[ignore = "needs the native compiler"]
fn representative_overrides_compile_with_their_final_sidecars() {
    eprintln!("native smoke: own/no_super");
    let run = fixture(&files("own", false, true)).run_spinel_with_rbs(emitted_program());
    run.assert_passes();
    assert_eq!(run.stdout, reference("own", false));

    let cases = review_cases();
    for name in [
        "parent/literal_string/callbacks=true",
        "concern/request_selector_type",
    ] {
        eprintln!("native smoke: {name}");
        let case = cases.iter().find(|c| c.name == name).unwrap();
        let run = fixture(&case.files).run_spinel_with_rbs(emitted_program());
        run.assert_passes();
        assert_eq!(run.stdout, case.expected, "{name}");
    }

    eprintln!("native smoke: parent/around_before");
    let run = fixture(&parent_order_files(true)).run_spinel_with_rbs(
        "require_relative 'app/controllers/probe_controller'\nc = ActionParentController.new\nc.process_action(:index)\nputs c.trace"
    );
    run.assert_passes();
    assert_eq!(run.stdout, "around>before>action>around_tail>\n");

    // A refused input must stop before a native compiler is invoked.
    eprintln!("native refusal: inherited reserved alias (emit only)");
    let collision = inherited_alias_files(
        "_roundhouse_dispatch_action",
        "alias_method :_roundhouse_dispatch_action, :trace",
    );
    let (_, errors) = fixture(&collision).emit(BuildTarget::Spinel);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("controller_dispatch_collision")),
        "{errors:?}"
    );
}
