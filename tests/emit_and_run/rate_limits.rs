//! Independent rate-limit options, compared with real Rails requests.
//! The adapters expose the same status/body pair; Rails' default exception
//! is the 429 that ActionDispatch's exception middleware would return.

use std::process::Command;
use super::emit_and_run;

const STORES: &str = r#"
class RateStores
  CUSTOM = ActiveSupport::Cache::MemoryStore.new
  CALLS = [0]
  def self.build
    CALLS[0] += 1
    CUSTOM
  end
end
class ReviewStores
  FIRST = RateStores::CUSTOM
  SECOND = ActiveSupport::Cache::MemoryStore.new
end
"#;

const EMITTED_REQUEST: &str = r#"
def request_for(klass)
  controller = klass.new
  controller.request = ActionDispatch::Request.for({"REQUEST_METHOD" => "GET", "REMOTE_ADDR" => "127.0.0.1"})
  controller.process_action(:index)
  [controller.status, controller.body]
end
"#;

const RAILS_REQUEST: &str = r#"
def request_for(klass)
  env = Rack::MockRequest.env_for("/", "REMOTE_ADDR" => "127.0.0.1")
  status, _headers, body = klass.action(:index).call(env)
  chunks = []
  body.each { |chunk| chunks << chunk }
  [status, chunks.join]
rescue ActionController::TooManyRequests
  [429, ""]
end
"#;

fn oracle_command() -> Command {
    let ruby = std::env::var_os("PROBE_RUBY").unwrap_or_else(|| "ruby".into());
    let mut cmd = Command::new(ruby);
    if let Some(gems) = std::env::var_os("PROBE_GEM_HOME") {
        cmd.env("GEM_HOME", &gems).env("GEM_PATH", &gems);
    }
    cmd
}

fn controller_oracle_available(mut cmd: Command, helper: &std::path::Path) -> bool {
    // Unit partitions do not install these optional Rails components.
    // Keep ingest-only tests active and use the shared oracle convention.
    let probe = cmd.args(["-e", &format!(
        "require {helper:?}\nputs RailsOracle.available?(\"actionpack\", \"actionview\")"
    )]).output().expect("Rails oracle availability probe");
    let available = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    assert!(probe.status.success() && matches!(available.as_str(), "true" | "false"),
        "Rails oracle availability probe failed: {available}\n{}", String::from_utf8_lossy(&probe.stderr));
    available == "true"
}

#[test]
fn controller_oracle_skips_when_the_controller_gems_are_missing() {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let empty = std::env::temp_dir().join(format!("roundhouse-rate-limit-no-gems-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("empty gem directory");
    let mut cmd = oracle_command();
    cmd.env("RH_RAILS_ORACLE", "8.1.4").env("GEM_HOME", &empty).env("GEM_PATH", &empty);
    let available = controller_oracle_available(cmd, &helper);
    std::fs::remove_dir(&empty).expect("remove empty gem directory");
    assert!(!available, "the unit gem view must skip the optional controller oracle");
}

fn compare(controllers: &[(&str, &str)], program: &str, expected: serde_json::Value) {
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    if !controller_oracle_available(oracle_command(), &helper) {
        eprintln!("skipping: rate-limit request oracle requires the selected Rails actionpack and actionview components");
        return;
    }
    let mut native = format!(r#"
require {helper:?}
RailsOracle.activate("actionpack")
require "action_controller"
require "action_controller/api"
require "rack/mock"
require "json"
warn "oracle: #{{RUBY_DESCRIPTION}}; Rails #{{ActionPack::VERSION::STRING}}"
ActionController::API.cache_store = ActiveSupport::Cache::MemoryStore.new
{STORES}
{RAILS_REQUEST}
"#);
    let mut routes = String::from("Rails.application.routes.draw do\n");
    for (path, _) in controllers {
        if let Some(path) = path.strip_prefix("app/controllers/").and_then(|p| p.strip_suffix("_controller.rb")) {
            if !path.starts_with("concerns/") {
                routes.push_str(&format!("  get '/{path}', to: '{path}#index'\n"));
            }
        }
    }
    routes.push_str("end\n");
    let mut tree = std::collections::HashMap::from([
        (std::path::PathBuf::from("config/routes.rb"), routes.as_bytes().to_vec()),
        (std::path::PathBuf::from("app/lib/rate_stores.rb"), STORES.as_bytes().to_vec()),
    ]);
    for (path, source) in controllers {
        tree.insert(std::path::PathBuf::from(path), source.as_bytes().to_vec());
    }
    roundhouse::ingest::survey::activate();
    let ingested = roundhouse::ingest::ingest_app_from_tree(tree);
    let gaps = roundhouse::ingest::survey::drain();
    ingested.expect("probe reaches ingest");
    assert!(!gaps.iter().any(|gap| matches!(gap, roundhouse::ingest::IngestError::Unsupported { message, .. } if message.contains("rate_limit"))), "the request must exercise a lowered limiter, not a dropped macro: {gaps:?}");
    let mut overlay = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n")
        .write("config/routes.rb", &routes)
        .write("app/lib/rate_stores.rb", STORES);
    for (path, source) in controllers {
        native.push_str(source);
        native.push('\n');
        overlay = overlay.write(path, source);
    }
    native.push_str(program);
    native.insert_str(0, "# encoding: utf-8\n");
    let mut cmd = oracle_command();
    let oracle = cmd.args(["-e", &native]).output().expect("Rails oracle");
    eprint!("{}", String::from_utf8_lossy(&oracle.stderr));
    assert!(oracle.status.success(), "Rails oracle failed:\n{}", String::from_utf8_lossy(&oracle.stderr));
    let oracle: serde_json::Value = serde_json::from_slice(&oracle.stdout).expect("Rails JSON");
    assert_eq!(oracle, expected, "the native control must exercise its intended limits");

    // Require explicitly as well as routing, so load order is observable.
    let loads = controllers.iter().filter(|(p, _)| p.starts_with("app/controllers/") && !p.starts_with("app/controllers/concerns/"))
        .map(|(p, _)| format!("require_relative {p:?}\n")).collect::<String>();
    let emitted = overlay.run_ruby(&format!("require 'json'\n{loads}\n{EMITTED_REQUEST}\n{program}"));
    emitted.assert_passes();
    let actual: serde_json::Value = serde_json::from_str(&emitted.stdout).expect("emitted JSON");
    assert_eq!(actual, oracle, "requests, bodies, counters and declaration-time effects must match Rails");
}

#[test]
fn an_explicit_store_is_evaluated_once_at_class_load_and_keeps_its_counter() {
    compare(&[("app/controllers/store_controller.rb", r#"
class StoreController < ActionController::API
  rate_limit to: 3, within: 60, store: RateStores.build
  def index = render plain: "ok"
end
"#)], r#"
loaded = RateStores::CALLS[0]
default = StoreController.cache_store
responses = Array.new(5) { request_for(StoreController) }
key = "rate-limit:store:127.0.0.1"
puts JSON.generate([loaded, RateStores::CALLS[0], responses, RateStores::CUSTOM.read(key).to_i, default.read(key).to_i])
"#, serde_json::json!([1, 1, [[200,"ok"],[200,"ok"],[200,"ok"],[429,""],[429,""]], 5, 0]));
}

#[test]
fn a_constructor_store_is_shared_by_requests_instead_of_recreated() {
    compare(&[("app/controllers/store_controller.rb", r#"
class StoreController < ActionController::API
  rate_limit to: 3, within: 60, store: ActiveSupport::Cache::MemoryStore.new
  def index = render plain: "ok"
end
"#)], r#"
responses = Array.new(5) { request_for(StoreController) }
puts JSON.generate([responses, StoreController.cache_store.read("rate-limit:store:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"],[429,""],[429,""]], 0]));
}

#[test]
fn an_explicit_cache_store_read_is_captured_before_later_configuration_changes() {
    compare(&[("app/controllers/store_controller.rb", r#"
class StoreController < ActionController::API
  rate_limit to: 3, within: 60, store: cache_store
  def index = render plain: "ok"
end
"#)], r#"
captured = StoreController.cache_store
replacement = ActiveSupport::Cache::MemoryStore.new
StoreController.cache_store = replacement
responses = Array.new(5) { request_for(StoreController) }
key = "rate-limit:store:127.0.0.1"
puts JSON.generate([responses, captured.read(key).to_i, replacement.read(key).to_i])
"#, serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"],[429,""],[429,""]],5,0]));
}

#[test]
fn a_store_that_returns_no_counter_does_not_throttle() {
    compare(&[("app/controllers/null_controller.rb", r#"
class NullController < ActionController::API
  rate_limit to: -1, within: 60, store: ActiveSupport::Cache::NullStore.new
  def index = render plain: "ok"
end
"#)], r#"
responses = Array.new(3) { request_for(NullController) }
puts JSON.generate([responses, NullController.cache_store.read("rate-limit:null:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"]],0]));
}

#[test]
fn the_custom_store_exposes_integer_counters_deletion_and_the_first_windows_expiry() {
    compare(&[("app/controllers/store_controller.rb", r#"
class StoreController < ActionController::API
  rate_limit to: 3, within: 60, store: RateStores::CUSTOM
  def index = render plain: "ok"
end
"#)], r#"
store = RateStores::CUSTOM
first = store.increment("counter", 2, expires_in: 1)
sleep 0.1
second = store.increment("counter", 3, expires_in: 60)
read = store.read("counter")
sleep 1.05
expired = store.read("counter")
restart = store.increment("counter", 1, expires_in: 60)
deleted = store.delete("counter")
missing = store.read("counter")
store.increment("counter", 1, expires_in: 60)
store.clear
puts JSON.generate([first, second, read, expired, restart, deleted, missing, store.read("counter")])
"#, serde_json::json!([2,5,5,null,1,true,null,null]));
}

#[test]
fn a_symbol_calls_the_private_handler_after_the_allowed_requests() {
    compare(&[("app/controllers/symbol_controller.rb", r#"
class SymbolController < ActionController::API
  rate_limit to: 3, within: 60, with: :limited
  def index = render plain: "ok"
  private
  def limited = render plain: "slow", status: :too_many_requests
end
"#)], r#"
responses = Array.new(5) { request_for(SymbolController) }
puts JSON.generate([responses, SymbolController.cache_store.read("rate-limit:symbol:127.0.0.1").to_i, RateStores::CUSTOM.read("rate-limit:symbol:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"],[429,"slow"],[429,"slow"]], 5, 0]));
}

#[test]
fn non_identifier_names_preserve_literal_keys_and_all_sibling_limits() {
    for name in ["short-term", "a / b!?", "東京", "", "#@budget #$budget #{literal}", "a'b\"c\\d", "line\nbreak"] {
        let literal = format!("'{}'", name.replace('\\', "\\\\").replace('\'', "\\'"));
        let source = format!(r#"
class NamedController < ActionController::API
  rate_limit to: 3, within: 60, name: {literal}
  rate_limit to: 10, within: 60, name: "other-limit"
  def index = render plain: "ok"
end
"#);
        let program = format!(r#"
responses = Array.new(5) {{ request_for(NamedController) }}
key = "rate-limit:named:" + {literal} + ":127.0.0.1"
puts JSON.generate([responses, NamedController.cache_store.read(key).to_i, NamedController.cache_store.read("rate-limit:named:other-limit:127.0.0.1").to_i])
"#);
        compare(&[("app/controllers/named_controller.rb", &source)], &program,
            serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"],[429,""],[429,""]], 5, 3]));
    }
}

#[test]
fn sanitized_names_do_not_replace_each_other_or_an_existing_method() {
    compare(&[("app/controllers/named_controller.rb", r#"
class NamedController < ActionController::API
  rate_limit to: 3, within: 60, name: "short-term"
  rate_limit to: 10, within: 60, name: "short_term"
  rate_limit to: 100, within: 60, name: "short_term_2"
  def index = render plain: "ok"
  private
  def rate_limit_short_term = head :forbidden
end
"#)], r#"
responses = Array.new(5) { request_for(NamedController) }
keys = ["short-term", "short_term", "short_term_2"]
counts = keys.map { |key| NamedController.cache_store.read("rate-limit:named:#{key}:127.0.0.1").to_i }
puts JSON.generate([responses, counts])
"#, serde_json::json!([[[200,"ok"],[200,"ok"],[200,"ok"],[429,""],[429,""]], [5,3,3]]));
}

#[test]
fn the_combined_declaration_uses_the_scope_store_and_symbol_handler() {
    compare(&[
        ("config/initializers/rate_limits.rb", "RATE_LIMITS = ActiveSupport::Cache::MemoryStore.new\n"),
        ("app/controllers/combined_controller.rb", r#"
class CombinedController < ActionController::API
  rate_limit to: 100, within: 1.minute, by: -> { request.remote_ip }, scope: "api", store: RATE_LIMITS, with: :rate_limited
  def index = render plain: "ok"
  private
  def rate_limited = head :too_many_requests
end
"#)], r#"
responses = Array.new(102) { request_for(CombinedController) }
key = "rate-limit:api:127.0.0.1"
puts JSON.generate([responses.count { |s, b| s == 200 && b == "ok" }, responses.last(2), RATE_LIMITS.read(key).to_i, CombinedController.cache_store.read(key).to_i])
"#, serde_json::json!([100, [[429,""],[429,""]], 102, 0]));
}

#[test]
fn an_initializer_store_is_shared_across_controllers_and_constructed_once() {
    compare(&[
        ("config/initializers/rate_limits.rb", "RATE_LIMITS = RateStores.build\n"),
        ("app/controllers/first_controller.rb", r#"
class FirstController < ActionController::API
  rate_limit to: 2, within: 60, store: RATE_LIMITS, scope: "shared"
  def index = render plain: "ok"
end
"#),
        ("app/controllers/second_controller.rb", r#"
class SecondController < ActionController::API
  rate_limit to: 2, within: 60, store: RATE_LIMITS, scope: "shared"
  def index = render plain: "ok"
end
"#)
    ], r#"
loaded = RateStores::CALLS[0]
responses = [request_for(FirstController), request_for(SecondController), request_for(FirstController), request_for(SecondController)]
puts JSON.generate([loaded, RateStores::CALLS[0], responses, RATE_LIMITS.read("rate-limit:shared:127.0.0.1").to_i, FirstController.cache_store.read("rate-limit:shared:127.0.0.1").to_i])
"#, serde_json::json!([1,1,[[200,"ok"],[200,"ok"],[429,""],[429,""]],4,0]));
}

#[test]
fn an_inherited_limit_keeps_its_store_when_the_child_declares_another() {
    compare(&[
        ("app/controllers/parent_controller.rb", r#"
class ParentController < ActionController::API
  rate_limit to: 1, within: 60, store: RateStores.build, name: "parent", scope: "parent"
  def index = render plain: "ok"
end
"#),
        ("app/controllers/child_controller.rb", r#"
class ChildController < ParentController
  rate_limit to: 10, within: 60, store: ActiveSupport::Cache::MemoryStore.new, name: "child"
  def index = render plain: "ok"
end
"#)], r#"
responses = Array.new(3) { request_for(ChildController) }
puts JSON.generate([responses, RateStores::CALLS[0], RateStores::CUSTOM.read("rate-limit:parent:parent:127.0.0.1").to_i, ChildController.cache_store.read("rate-limit:parent:parent:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[429,""],[429,""]], 1, 3, 0]));
}

#[test]
fn symbol_handlers_follow_own_parent_and_concern_method_lookup() {
    for (extra, handler) in [("", "parent"), ("def limited = render plain: 'own', status: :forbidden", "own")] {
        let child = format!("class ChildController < ParentController\n  def index = render plain: 'ok'\n{extra}\nend\n");
        compare(&[
            ("app/controllers/parent_controller.rb", r#"
class ParentController < ActionController::API
  rate_limit to: 1, within: 60, with: :limited, scope: "parent"
  def index = render plain: "ok"
  private
  def limited = render plain: "parent", status: :forbidden
end
"#),
            ("app/controllers/child_controller.rb", &child)
        ], r#"
responses = Array.new(2) { request_for(ChildController) }
puts JSON.generate([responses, ChildController.cache_store.read("rate-limit:parent:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[403,handler]],2]));
    }
    compare(&[
        ("app/controllers/concerns/limited_handler.rb", r#"
module LimitedHandler
  extend ActiveSupport::Concern
  private
  def limited = render plain: "concern", status: :forbidden
end
"#),
        ("app/controllers/concern_controller.rb", r#"
class ConcernController < ActionController::API
  include LimitedHandler
  rate_limit to: 1, within: 60, with: :limited
  def index = render plain: "ok"
end
"#)
    ], r#"
responses = Array.new(2) { request_for(ConcernController) }
puts JSON.generate([responses, ConcernController.cache_store.read("rate-limit:concern:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[403,"concern"]],2]));
}

#[test]
fn unsupported_options_keep_explicit_gaps_without_losing_supported_siblings() {
    use roundhouse::dialect::ControllerBodyItem;
    use roundhouse::ingest::{ingest_app_from_tree, survey};
    use std::collections::HashMap;
    use std::path::PathBuf;
    for option in ["with: handler", "with: :'not-a-method'", "store: @store", "by: :identity", "scope: scope_for_limits", "name: computed_name", "bogus: true"] {
        let source = format!(r#"class ProbeController < ActionController::API
  rate_limit to: 3, within: 60, {option}
  rate_limit to: 10, within: 60, name: "supported-sibling"
  def index = head :ok
end
"#);
        survey::activate();
        let app = ingest_app_from_tree(HashMap::from([
            (PathBuf::from("app/controllers/probe_controller.rb"), source.into_bytes()),
            (PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
        ]));
        let gaps = survey::drain();
        let app = app.expect("ingest");
        assert!(gaps.iter().any(|g| matches!(g, roundhouse::ingest::IngestError::Unsupported { message, .. } if message.contains("rate_limit"))), "missing gap for {option}: {gaps:?}");
        let c = app.controllers.iter().find(|c| c.name.0.as_str() == "ProbeController").unwrap();
        assert_eq!(c.filters().count(), 1, "supported sibling lost for {option}");
        assert!(c.body.iter().any(|i| matches!(i, ControllerBodyItem::Unknown { expr, .. } if matches!(&*expr.node, roundhouse::expr::ExprNode::Send { method, .. } if method.as_str() == "rate_limit"))));
    }
}

#[test]
fn unsupported_boot_store_declarations_keep_a_rate_limit_gap() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use roundhouse::ingest::{ingest_app_from_tree, survey};
    for initializer in [
        "if true\nRATE_LIMITS = ActiveSupport::Cache::MemoryStore.new\nend\n",
        "RATE_LIMITS = ActiveSupport::Cache::MemoryStore.new\nRATE_LIMITS = ActiveSupport::Cache::MemoryStore.new\n",
        "RATE_LIMITS = ActiveSupport::Cache::MemoryStore.new\nSomething.configure\n",
        "RATE_LIMITS = make_store\n",
        "RATE_LIMITS = OTHER_STORE\n",
    ] {
        survey::activate();
        let app = ingest_app_from_tree(HashMap::from([
            (PathBuf::from("config/initializers/rate_limits.rb"), initializer.as_bytes().to_vec()),
            (PathBuf::from("app/controllers/probe_controller.rb"), b"class ProbeController < ActionController::API\nrate_limit to: 1, within: 60, store: RATE_LIMITS\ndef index = head :ok\nend\n".to_vec()),
        ]));
        let gaps = survey::drain();
        let app = app.expect("ingest");
        assert!(gaps.iter().any(|g| matches!(g, roundhouse::ingest::IngestError::Unsupported { message, .. } if message.contains("rate_limit"))), "{initializer}: {gaps:?}");
        assert!(app.controllers.iter().all(|c| c.filters().count() == 0));
    }
    for target in ["RATE_LIMITS", "::RATE_LIMITS", "Object::RATE_LIMITS", "::Object::RATE_LIMITS"] {
        for write in [
            format!("{target} &&= ReviewStores::SECOND\n"),
            format!("{target} ||= ReviewStores::SECOND\n"),
            format!("{target} += ReviewStores::SECOND\n"),
            format!("{target}, OTHER = ReviewStores::SECOND, nil\n"),
            format!("OTHER, ({target}, *TAIL) = nil, [ReviewStores::SECOND]\n"),
            "class StoreSetup\nObject::RATE_LIMITS &&= ReviewStores::SECOND\nend\n".to_string(),
            "module StoreSetup\n::RATE_LIMITS = ReviewStores::SECOND\nend\n".to_string(),
        ] {
            for reference in ["RATE_LIMITS", "::RATE_LIMITS", "Object::RATE_LIMITS", "::Object::RATE_LIMITS"] {
                let source = format!("class ProbeController < ActionController::API\nrate_limit to: 1, within: 60, store: {reference}\ndef index = render plain: \"ok\"\nend\n");
                assert_store_refused(&[
                    ("config/initializers/a_store.rb", "RATE_LIMITS = ReviewStores::FIRST\n"),
                    ("config/initializers/b_store.rb", &write),
                    ("app/controllers/probe_controller.rb", &source),
                ], 0);
            }
        }
    }
}

#[test]
fn root_qualified_initializer_stores_are_loaded_before_the_capture() {
    for reference in ["Object::RATE_LIMITS", "::Object::RATE_LIMITS", "::RATE_LIMITS"] {
        let source = format!(r#"class ProbeController < ActionController::API
  rate_limit to: 1, within: 60, store: {reference}
  def index = render plain: "ok"
end
"#);
        compare(&[
            ("config/initializers/rate_limits.rb", "RATE_LIMITS = ReviewStores::FIRST\n"),
            ("app/controllers/probe_controller.rb", &source),
        ], r#"
responses = Array.new(2) { request_for(ProbeController) }
puts JSON.generate([responses, ReviewStores::FIRST.read("rate-limit:probe:127.0.0.1").to_i,
  ReviewStores::SECOND.read("rate-limit:probe:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[429,""]],2,0]));
    }
}

#[test]
fn explicitly_global_store_references_ignore_local_and_ancestor_shadows() {
    for inherited in [false, true] {
        for reference in ["::RATE_LIMITS", "Object::RATE_LIMITS", "::Object::RATE_LIMITS", "RATE_LIMITS"] {
            let parent = if inherited {
                "class ParentController < ActionController::API\nRATE_LIMITS = ReviewStores::SECOND\nend\n"
            } else { "class ParentController < ActionController::API\nend\n" };
            let local = if inherited { "" } else { "RATE_LIMITS = ReviewStores::SECOND" };
            let source = format!(r#"class ProbeController < ParentController
  {local}
  rate_limit to: 1, within: 60, store: {reference}
  def index = render plain: "ok"
end
"#);
            let counts = if reference == "RATE_LIMITS" { [0,2] } else { [2,0] };
            compare(&[
                ("config/initializers/rate_limits.rb", "RATE_LIMITS = ReviewStores::FIRST\n"),
                ("app/controllers/parent_controller.rb", parent),
                ("app/controllers/probe_controller.rb", &source),
            ], r#"
responses = Array.new(2) { request_for(ProbeController) }
puts JSON.generate([responses, ReviewStores::FIRST.read("rate-limit:probe:127.0.0.1").to_i,
  ReviewStores::SECOND.read("rate-limit:probe:127.0.0.1").to_i])
"#, serde_json::json!([[[200,"ok"],[429,""]],counts[0],counts[1]]));
        }
    }
}

#[test]
fn a_bare_store_name_uses_lexical_nesting_before_ancestors_and_boot() {
    for form in ["nested", "nested_with_ancestor", "compact"] {
        let parent = if form == "nested_with_ancestor" {
            "class ParentController < ActionController::API\nRATE_LIMITS = ReviewStores::FIRST\nend\n"
        } else { "class ParentController < ActionController::API\nend\n" };
        let source = if form == "compact" {
            r#"class RateNamespace::ProbeController < ActionController::API
  rate_limit to: 1, within: 60, store: RATE_LIMITS
  def index = render plain: "ok"
end
"#
        } else {
            r#"module RateNamespace
  class ProbeController < ::ParentController
    rate_limit to: 1, within: 60, store: RATE_LIMITS
    def index = render plain: "ok"
  end
end
"#
        };
        let counts = if form == "compact" { [2, 0] } else { [0, 2] };
        compare(&[
            ("app/lib/rate_namespace.rb", "module RateNamespace\nRATE_LIMITS = ReviewStores::SECOND\nend\n"),
            ("config/initializers/rate_limits.rb", "RATE_LIMITS = ReviewStores::FIRST\n"),
            ("app/controllers/parent_controller.rb", parent),
            ("app/controllers/rate_namespace/probe_controller.rb", source),
        ], r#"
responses = Array.new(2) { request_for(RateNamespace::ProbeController) }
key = "rate-limit:rate_namespace/probe:127.0.0.1"
puts JSON.generate([responses, ReviewStores::FIRST.read(key).to_i, ReviewStores::SECOND.read(key).to_i])
"#, serde_json::json!([[[200,"ok"],[429,""]],counts[0],counts[1]]));
    }
}

// Keep the unrooted regression and rooted control in the same optional
// controller-oracle partition as every other request comparison.
fn compare_object_store(reference: &str, expected_counts: [i32; 2]) {
    for shadow in ["controller", "ancestor", "namespace", "lexical_before_ancestor"] {
        let inherited = matches!(shadow, "ancestor" | "lexical_before_ancestor");
        let namespace = matches!(shadow, "namespace" | "lexical_before_ancestor");
        let parent = if inherited {
            "class ParentController < ActionController::API\nObject = ShadowStores\nend\n"
        } else { "class ParentController < ActionController::API\nend\n" };
        let namespace_source = if shadow == "lexical_before_ancestor" {
            "module RateNamespace\nObject = ::Object\nend\n"
        } else if namespace {
            "module RateNamespace\nObject = ShadowStores\nend\n"
        } else { "" };
        let local = if shadow == "controller" { "Object = ShadowStores" } else { "" };
        let (open, close, class_name, key) = if namespace {
            ("module RateNamespace\n", "end\n", "RateNamespace::ProbeController", "rate-limit:rate_namespace/probe:127.0.0.1")
        } else { ("", "", "ProbeController", "rate-limit:probe:127.0.0.1") };
        let base = if inherited { "::ParentController" } else { "ActionController::API" };
        let path = if namespace { "app/controllers/rate_namespace/probe_controller.rb" } else { "app/controllers/probe_controller.rb" };
        let source = format!(r#"{open}class ProbeController < {base}
  {local}
  rate_limit to: 1, within: 60, store: {reference}
  def index = render plain: "ok"
end
{close}"#);
        let counts = if shadow == "lexical_before_ancestor" { [2, 0] } else { expected_counts };
        compare(&[
            ("app/lib/shadow_stores.rb", "class ShadowStores\nRATE_LIMITS = ReviewStores::SECOND\nend\n"),
            ("app/lib/rate_namespace.rb", namespace_source),
            ("config/initializers/rate_limits.rb", "RATE_LIMITS = ReviewStores::FIRST\n"),
            ("app/controllers/parent_controller.rb", parent),
            (path, &source),
        ], &format!(r#"
responses = Array.new(2) {{ request_for({class_name}) }}
puts JSON.generate([responses, ReviewStores::FIRST.read({key:?}).to_i,
  ReviewStores::SECOND.read({key:?}).to_i])
"#), serde_json::json!([[[200,"ok"],[429,""]],counts[0],counts[1]]));
    }
}

#[test]
fn an_unrooted_object_store_resolves_controller_namespace_and_ancestor_shadows() {
    compare_object_store("Object::RATE_LIMITS", [0, 2]);
}

#[test]
fn an_explicitly_rooted_object_store_ignores_object_shadows() {
    compare_object_store("::Object::RATE_LIMITS", [2, 0]);
}

#[test]
fn an_unresolved_object_store_keeps_an_explicit_rate_limit_gap() {
    assert_store_refused(&[("app/controllers/probe_controller.rb", r#"class ProbeController < ActionController::API
  Object = choose_store_class
  rate_limit to: 1, within: 60, store: Object::RATE_LIMITS
  def index = render plain: "ok"
end
"#)], 0);
}

#[test]
fn an_unresolved_bare_store_keeps_an_explicit_rate_limit_gap() {
    assert_store_refused(&[("app/controllers/probe_controller.rb", r#"class ProbeController < ActionController::API
  rate_limit to: 1, within: 60, store: MISSING_STORE
  def index = render plain: "ok"
end
"#)], 0);
}

#[test]
fn a_namespace_store_alias_retains_a_discarded_configuration_gap() {
    assert_store_refused(&[
        ("app/controllers/parent_controller.rb", r#"class ParentController < ActionController::API
  self.cache_store = ReviewStores::FIRST
  CAPTURED_STORE = cache_store
end
"#),
        ("app/lib/rate_namespace.rb", r#"module RateNamespace
  RATE_LIMITS = ParentController::CAPTURED_STORE
end
"#),
        ("app/controllers/probe_controller.rb", r#"class ProbeController < ActionController::API
  CAPTURED_NAMESPACE_STORE = RateNamespace::RATE_LIMITS
  rate_limit to: 1, within: 60, store: CAPTURED_NAMESPACE_STORE
  def index = render plain: "ok"
end
"#),
    ], 0);
}

fn assert_store_refused(files: &[(&str, &str)], expected_filters: usize) {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use roundhouse::dialect::ControllerBodyItem;
    use roundhouse::expr::ExprNode;
    use roundhouse::ingest::{ingest_app_from_tree, survey};
    let mut tree = HashMap::from([
        (PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\nend\n".to_vec()),
        (PathBuf::from("app/lib/rate_stores.rb"), STORES.as_bytes().to_vec()),
    ]);
    tree.extend(files.iter().map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec())));
    survey::activate();
    let app = ingest_app_from_tree(tree);
    let gaps = survey::drain();
    let app = app.expect("review fixture reaches ingest");
    assert!(gaps.iter().any(|gap| matches!(gap,
        roundhouse::ingest::IngestError::Unsupported { message, .. } if message.contains("rate_limit"))),
        "unsupported store must retain a rate-limit gap: {files:?}: {gaps:?}");
    let controller = app.controllers.iter().find(|c| c.name.0.as_str() == "ProbeController").unwrap();
    assert_eq!(controller.filters().count(), expected_filters, "{files:?}");
    assert!(controller.body.iter().any(|item| matches!(item,
        ControllerBodyItem::Unknown { expr, .. } if matches!(&*expr.node,
            ExprNode::Send { method, .. } if method.as_str() == "rate_limit"))),
        "the refused declaration must survive: {files:?}");
}

#[test]
fn discarded_controller_configuration_keeps_dependent_stores_refused() {
    assert_store_refused(&[("app/controllers/probe_controller.rb", r#"class ProbeController < ActionController::API
  self.cache_store = ReviewStores::FIRST
  rate_limit to: 1, within: 60, store: cache_store
  self.cache_store = ReviewStores::SECOND
  def index = render plain: "ok"
end
"#)], 0);
    assert_store_refused(&[("app/controllers/probe_controller.rb", r#"class ProbeController < ActionController::API
  self.cache_store = ReviewStores::FIRST
  CAPTURED_STORE = ReviewStores::FIRST
  CAPTURED_STORE = cache_store
  rate_limit to: 1, within: 60, store: CAPTURED_STORE
  self.cache_store = ReviewStores::SECOND
  def index = render plain: "ok"
end
"#)], 0);
    for inherited in [false, true] {
        let parent = if inherited {
            "class ParentController < ActionController::API\nself.cache_store = ReviewStores::FIRST\nCAPTURED_STORE = cache_store\nend\n"
        } else { "class ParentController < ActionController::API\nend\n" };
        let config = if inherited { "" } else {
            "self.cache_store = ReviewStores::FIRST\nCAPTURED_STORE = cache_store"
        };
        let later_config = if inherited { "" } else { "self.cache_store = ReviewStores::SECOND" };
        for reference in ["cache_store", "self.cache_store", "ProbeController.cache_store", "CAPTURED_STORE"] {
            let source = format!(r#"class ProbeController < ParentController
  {config}
  INDEPENDENT_STORE = ReviewStores::FIRST
  rate_limit to: 1, within: 60, store: {reference}
  {later_config}
  rate_limit to: 10, within: 60, store: INDEPENDENT_STORE, name: "independent"
  def index = render plain: "ok"
end
"#);
            assert_store_refused(&[
                ("app/controllers/parent_controller.rb", parent),
                ("app/controllers/probe_controller.rb", &source),
            ], 1);
        }
    }
}
