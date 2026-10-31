//! `roundhouse check` at the process boundary: the exit code is the
//! contract a CI gate reads, so the cases that must NOT report clean
//! are pinned here.

use std::process::Command;

fn check(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .arg("check")
        .args(args)
        .output()
        .expect("spawn roundhouse");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_missing_path_is_not_a_clean_app() {
    let (code, err) = check(&["/nonexistent/rails/app"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not a directory"), "{err}");
    assert!(!err.contains("0 error(s)"), "must not print a summary: {err}");
}

#[test]
fn a_directory_without_app_is_not_a_clean_app() {
    // The repo root: a directory, but not a Rails app.
    let (code, err) = check(&[env!("CARGO_MANIFEST_DIR")]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("does not look like a Rails app"), "{err}");
}

#[test]
fn malformed_test_path_configuration_is_not_a_clean_app() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse_bad_test_paths_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("app")).expect("create app directory");
    std::fs::write(root.join("roundhouse.yml"), "test_paths: test/unit\n").expect("write config");
    let app = root.to_str().expect("temporary path");

    for mode in ["--strict", "--continue"] {
        let (code, error) = check(&[mode, app]);
        assert_eq!(code, 2, "{error}");
        assert!(error.contains("roundhouse.yml"), "{error}");
    }

    std::fs::remove_dir_all(root).expect("remove temporary app");
}

#[test]
fn the_store_fixture_checks_clean() {
    let store = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/store");
    let (code, err) = check(&[store]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("0 parse error(s), 0 error(s), 0 warning(s), 0 gap-attributed note(s), 0 survey gap(s)"),
        "{err}"
    );
}

#[test]
fn continue_writes_inventory_json_and_prints_hotspots() {
    let root = std::env::temp_dir().join(format!(
        "roundhouse-inventory-{}",
        std::process::id()
    ));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    @missing_thing\n  end\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let inventory = root.join("inventory.json");
    let app = root.to_str().unwrap();
    let inv = inventory.to_str().unwrap();
    let (code, err) = check(&["--continue", "--inventory", inv, app]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("── Hotspots:"), "expected hotspot report: {err}");
    assert!(err.contains("ivar"), "{err}");
    let json = std::fs::read_to_string(&inventory).expect("inventory written");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(parsed["schema"], 1);
    assert!(parsed["summary"]["errors"].as_u64().unwrap() >= 1);
    assert!(parsed["hotspots"].as_array().unwrap().len() >= 1);
    std::fs::remove_dir_all(root).ok();
}

/// #207's original nullable ActiveRecord source-property repro must remain
/// unsupported, but survey mode must ledger the declaration and its provider.
#[test]
fn alba_rejections_are_ledgered_without_reporting_executable_support() {
    let root = std::env::temp_dir().join(format!("roundhouse-207-{}", std::process::id()));
    for (path, source) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n  def show\n    render json: {article: ArticleResource.new(Article.find(params[:id])).to_h}\n  end\nend\n"),
        ("app/resources/application_resource.rb", "class ApplicationResource\n  include Alba::Resource\nend\n"),
        ("app/resources/article_resource.rb", "class ArticleResource < ApplicationResource\n  attributes :id, :title\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\n  create_table :articles do |t|\n    t.string :title\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :show\nend\n"),
        ("Gemfile.lock", include_str!("../fixtures/gem-capabilities/historical/locks/alba.lock")),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let path = root.to_str().unwrap();
    let (code, strict) = check(&["--strict", path]);
    assert_eq!(code, 1, "{strict}");
    assert!(strict.contains("error[unsupported]: alba_serialization"), "{strict}");
    assert!(strict.contains("0 survey gap(s)"), "{strict}");

    let (code, surveyed) = check(&["--continue", path]);
    assert_eq!(code, 1, "{surveyed}");
    assert!(surveyed.contains("error[unsupported]: alba_serialization"), "{surveyed}");
    assert!(surveyed.contains("the `alba` gem"), "{surveyed}");
    assert!(surveyed.contains("1 error(s), 0 warning(s), 0 gap-attributed note(s), 1 survey gap(s)"), "{surveyed}");
    assert!(surveyed.contains("Survey: 1 ingest gap(s)"), "{surveyed}");
    assert!(surveyed.contains("app/resources/article_resource.rb"), "{surveyed}");

    // A syntactically valid DSL outside the modeled declaration subset is
    // still unsupported. Strict mode fails fast; survey mode ledgers the
    // declaration, leaves the class unlowered, and continues to the summary.
    std::fs::write(root.join("app/resources/article_resource.rb"), "class ArticleResource < ApplicationResource\n  attributes :id, :title, if: :visible?\nend\n").unwrap();
    let (code, rejected) = check(&["--strict", path]);
    assert_eq!(code, 2, "{rejected}");
    assert!(rejected.contains("ingest failed"), "{rejected}");
    assert!(rejected.contains("Alba source-property subset: unsupported Alba declaration"), "{rejected}");
    assert!(!rejected.contains("error(s)"), "strict refusal must not print an analysis summary: {rejected}");

    let (code, continued) = check(&["--continue", path]);
    assert_ne!(code, 2, "a ledgered Alba declaration must not abort --continue: {continued}");
    assert!(!continued.contains("ingest failed"), "{continued}");
    assert!(continued.contains("Survey: 1 ingest gap(s)"), "{continued}");
    assert!(continued.contains("Alba source-property subset: unsupported Alba declaration"), "{continued}");
    assert!(continued.contains("app/resources/article_resource.rb"), "{continued}");
    assert!(continued.contains("error(s)"), "analysis summary must be printed: {continued}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn gem_declarations_are_opt_in_inert_and_follow_named_unknown_policy() {
    let root = std::env::temp_dir().join(format!("roundhouse-declaration-input-{}", std::process::id()));
    let canary = root.join("executed");
    let gem = format!(
        "module Zork\nFile.write({:?}, 'executed')\nclass Client; end\nend\n",
        canary.to_str().unwrap()
    );
    for (path, source) in [
        ("Gemfile.lock", "GEM\n  specs:\n    zork (1.0)\n\nDEPENDENCIES\n  zork\n"),
        ("Gemfile", "raise 'Gemfile must not run'\n"),
        ("local/specifications/zork-1.0.gemspec", "raise 'gemspec must not run'\n"),
        ("local/gems/zork-1.0/lib/zork.rb", gem.as_str()),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end\n"),
        ("app/controllers/things_controller.rb", "class ThingsController < ApplicationController\ndef show\n@known = Zork::Client.new\n@unknown = Zork::MISSING\nend\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let app = root.to_str().unwrap();
    let (code, off) = check(&["--continue", "--no-hotspots", app]);
    assert_eq!(code, 1, "{off}");
    assert!(!off.contains("gem declarations:"), "{off}");

    let inventory = root.join("inventory.json");
    let local = root.join("local");
    let (code, on) = check(&[
        "--continue", "--no-hotspots", "--inventory", inventory.to_str().unwrap(),
        "--gem-declarations", "--gem-declarations-path", local.to_str().unwrap(), app,
    ]);
    let policy = roundhouse::diagnostic::GEM_DECLARATION_UNKNOWN_SEVERITY;
    let expected_code = if policy == roundhouse::diagnostic::Severity::Error { 1 } else { 0 };
    assert_eq!(code, expected_code, "{on}");
    assert!(!canary.exists(), "gem code ran");
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(inventory).unwrap()).unwrap();
    let unknown_count = data["diagnostics"].as_array().unwrap().iter()
        .filter(|d| d["code"] == "constant_lookup_unknown").count();
    let expected_errors = if expected_code == 1 { unknown_count } else { 0 };
    assert_eq!(data["summary"]["errors"], expected_errors, "{on}");
    let unknown = data["diagnostics"].as_array().unwrap().iter()
        .find(|d| d["code"] == "constant_lookup_unknown").expect("named unknown");
    assert_eq!(unknown["severity"], serde_json::to_value(policy).unwrap());
    assert!(unknown["message"].as_str().unwrap().contains("Zork::MISSING"));
    assert!(unknown["message"].as_str().unwrap().contains("incomplete namespace `Zork`"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn declaration_rejections_keep_reasons_and_never_supply_partial_names() {
    let root = std::env::temp_dir().join(format!("roundhouse-declaration-rejections-{}", std::process::id()));
    for (path, source) in [
        ("Gemfile.lock", b"GEM\n  specs:\n    zork (1.0)\n\nDEPENDENCIES\n  zork\n".as_slice()),
        ("local/gems/zork-1.0/lib/zork.rb", b"module Zork; class Client; end; end\n"),
        ("local/gems/zork-1.0/lib/legacy.rb", b"# coding: ISO-8859-1\nclass RejectedHeader; end\n"),
        ("local/gems/zork-1.0/lib/shebang.rb", b"#! coding: ISO-8859-1\n# encoding: UTF-8\nclass RejectedShebang; end\n"),
        ("contracts/import.rbs", b"use Zork::Client as Imported\nclass RejectedUse\nend\n"),
        ("contracts/bytes.rbs", b"class RejectedBytes\nend\n\xff"),
        ("app/controllers/application_controller.rb", b"class ApplicationController < ActionController::Base; end\n"),
        ("app/controllers/things_controller.rb", b"class ThingsController < ApplicationController\ndef show\n@values = [Zork::Client, RejectedHeader, RejectedShebang, RejectedUse, RejectedBytes]\nend\nend\n"),
        ("config/routes.rb", b"Rails.application.routes.draw do\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let local = root.join("local");
    let contracts = root.join("contracts");
    let inventory = root.join("inventory.json");
    let (code, output) = check(&[
        "--continue", "--no-hotspots", "--inventory", inventory.to_str().unwrap(),
        "--gem-declarations-path", local.to_str().unwrap(),
        "--rbs-declarations", contracts.to_str().unwrap(), root.to_str().unwrap(),
    ]);
    assert_eq!(code, if roundhouse::diagnostic::GEM_DECLARATION_UNKNOWN_SEVERITY == roundhouse::diagnostic::Severity::Error { 1 } else { 0 }, "rejected inputs must preserve uncertainty: {output}");
    let report: serde_json::Value = serde_json::from_str(output.lines()
        .find_map(|line| line.strip_prefix("gem declarations: ")).expect("declaration report")).unwrap();
    assert_eq!(report["rejected_files"], 4, "{report}");
    assert_eq!(report["ruby_files"], 1, "{report}");
    assert_eq!(report["rbs_files"], 0, "{report}");
    let reasons = report["rejection_reasons"].as_object().unwrap();
    for fragment in ["unsupported Ruby source encoding ISO-8859-1", "encoding declaration in shebang", "RBS `use`", "non-UTF-8 RBS"] {
        assert!(reasons.keys().any(|reason| reason.contains(fragment)), "lost reason {fragment}: {report}");
    }
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(inventory).unwrap()).unwrap();
    for name in ["RejectedHeader", "RejectedShebang", "RejectedUse", "RejectedBytes"] {
        assert!(data["diagnostics"].as_array().unwrap().iter().any(|diagnostic|
            diagnostic["code"] == "constant_lookup_unknown" && diagnostic["message"].as_str().unwrap().contains(name)),
            "partial declaration admitted for {name}: {output}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn emission_rejects_every_check_only_declaration_option() {
    let root = std::env::temp_dir().join(format!("roundhouse-declaration-emit-{}", std::process::id()));
    for target in roundhouse::project::BuildTarget::TRANSPILE.iter().map(|t| t.as_str())
        .chain(["fractal"])
    {
        for flag in ["--gem-declarations", "--gem-declarations-path", "--rbs-declarations"] {
            let output = root.join(format!("{target}{flag}"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_roundhouse"));
            command.args(["--target", target, flag]);
            if flag != "--gem-declarations" { command.arg(&root); }
            let result = command.arg(env!("CARGO_MANIFEST_DIR"))
                .arg("-o").arg(&output).output().unwrap();
            assert!(!result.status.success(), "accepted {target} {flag}");
            assert!(!output.exists(), "emitted {target} {flag}");
            assert!(String::from_utf8_lossy(&result.stderr).contains("unknown flag"),
                "{}", String::from_utf8_lossy(&result.stderr));
        }
    }
}

#[test]
fn declaration_paths_require_arguments_and_no_cache_option_is_exposed() {
    for flag in ["--gem-declarations-path", "--rbs-declarations"] {
        let (code, error) = check(&[flag]);
        assert_eq!(code, 2, "{error}");
        assert!(error.contains("requires a PATH argument"), "{error}");
    }
    let (code, error) = check(&["--gem-declarations-cache"]);
    assert_eq!(code, 2, "{error}");
    assert!(error.contains("unknown flag"), "{error}");
}


#[test]
fn declaration_header_only_findings_follow_the_named_unknown_policy() {
    let root = std::env::temp_dir().join(format!("roundhouse-declaration-headers-{}", std::process::id()));
    for (path, source) in [
        ("Gemfile.lock", "GEM\n  specs:\n    wrapper (1.0)\n      zork\n    zork (1.0)\n\nDEPENDENCIES\n  wrapper\n"),
        ("config/routes.rb", "Rails.application.routes.draw do; end"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let local = root.join("local");
    let gem = local.join("gems/zork-1.0/lib/zork.rb");
    std::fs::create_dir_all(gem.parent().unwrap()).unwrap();
    let inventory = root.join("inventory.json");
    let policy = roundhouse::diagnostic::GEM_DECLARATION_UNKNOWN_SEVERITY;
    for (path, source) in [
        ("app/lib/child.rb", "class Child < Zork::Base; end"),
        ("app/models/child.rb", "class Child < Zork::Base; end"),
        ("app/controllers/child_controller.rb", "class ChildController < Zork::Base; end"),
        ("app/lib/child.rb", "class Child; include Zork::Mix; end"),
        ("app/models/child.rb", "class Child < ActiveRecord::Base; prepend Zork::Mix; end"),
        ("app/controllers/child_controller.rb", "class ChildController < ApplicationController; extend Zork::Mix; end"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
        for unknown in [false, true] {
            std::fs::write(&gem, if unknown { "module Zork; eval('class Base; end; module Mix; end'); end" }
                else { "module Zork; class Base; end; module Mix; end; end" }).unwrap();
            let (off_code, off) = check(&["--continue", "--no-hotspots", root.to_str().unwrap()]);
            if source.contains("Zork::Base") { assert_eq!(off_code, 0, "header-only control: {path}: {off}"); }
            else { assert!(matches!(off_code, 0 | 1), "mixin control: {path}: {off}"); }
            assert!(!off.contains("constant_lookup_unknown"));
            let (code, output) = check(&["--continue", "--no-hotspots", "--inventory", inventory.to_str().unwrap(),
                "--gem-declarations-path", local.to_str().unwrap(), root.to_str().unwrap()]);
            let expected = if unknown && policy == roundhouse::diagnostic::Severity::Error { 1 } else { 0 };
            assert_eq!(code, expected, "{path}, unknown={unknown}: {output}");
            let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&inventory).unwrap()).unwrap();
            let wanted = if unknown { "constant_lookup_unknown" } else { "constant_loading_unverified" };
            let diagnostic = data["diagnostics"].as_array().unwrap().iter().find(|d| d["code"] == wanted)
                .unwrap_or_else(|| panic!("missing header finding at {path}: {output}"));
            assert!(diagnostic["path"].as_str().unwrap().ends_with(path));
            assert_eq!(diagnostic["severity"], serde_json::to_value(if unknown { policy } else { roundhouse::diagnostic::Severity::Warning }).unwrap());
        }
        std::fs::remove_file(file).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_rbs_without_a_lockfile_is_read_and_reported() {
    let root = std::env::temp_dir().join(format!("roundhouse-rbs-without-lock-{}", std::process::id()));
    for (path, source) in [
        ("config/routes.rb", "Rails.application.routes.draw do; end"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end"),
        ("app/controllers/things_controller.rb", "class ThingsController < ApplicationController; def show; @a = External::Thing.new; end; end"),
        ("contracts/thing.rbs", "module External\nclass Thing\nend\nend\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    assert!(!root.join("Gemfile.lock").exists());
    let (off_code, off) = check(&["--continue", "--no-hotspots", root.to_str().unwrap()]);
    assert_eq!(off_code, 1, "{off}");
    let contract = root.join("contracts");
    let (code, output) = check(&["--continue", "--no-hotspots", "--rbs-declarations", contract.to_str().unwrap(), root.to_str().unwrap()]);
    assert_eq!(code, 0, "{output}");
    let report: serde_json::Value = serde_json::from_str(output.lines().find_map(|line| line.strip_prefix("gem declarations: ")).unwrap()).unwrap();
    assert_eq!(report["trusted_rbs_inputs"], 1);
    assert_eq!(report["rbs_files"], 1);
    assert_eq!(report["gems_found"], 0);
    std::fs::remove_dir_all(root).unwrap();
}

fn round2_effect_exit_status(label: &str, effect: &str) {
    let root = std::env::temp_dir().join(format!("roundhouse-round2-{label}-{}", std::process::id()));
    for (path, source) in [
        ("Gemfile.lock", "GEM\n  specs:\n    wrapper (1.0)\n      zork\n    zork (1.0)\n\nDEPENDENCIES\n  wrapper\n"),
        ("config/routes.rb", "Rails.application.routes.draw do; end"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end"),
        ("app/lib/child.rb", "class Parent; TOKEN = 'parent'; end\nclass Child < Parent; include Zork::Mix; end"),
        ("app/controllers/things_controller.rb", "class ThingsController < ApplicationController; def show; @a = Child::TOKEN.upcase; end; end"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let local = root.join("local");
    let gem = local.join("gems/zork-1.0/lib/zork.rb");
    std::fs::create_dir_all(gem.parent().unwrap()).unwrap();
    let inventory = root.join("inventory.json");
    let policy = roundhouse::diagnostic::GEM_DECLARATION_UNKNOWN_SEVERITY;
    for member in ["TOKEN", "OTHER"] {
        std::fs::write(&gem, format!("module Zork; module Mix; end; module Other; end; {}; end", effect.replace("TOKEN", member))).unwrap();
        let (_, off) = check(&["--no-hotspots", root.to_str().unwrap()]);
        assert!(!off.contains("unknown constant"), "input-off must not use declarations: {off}");
        let (code, output) = check(&["--no-hotspots", "--inventory", inventory.to_str().unwrap(),
            "--gem-declarations-path", local.to_str().unwrap(), root.to_str().unwrap()]);
        let expected = if member == "TOKEN" && policy == roundhouse::diagnostic::Severity::Error { 1 } else { 0 };
        assert_eq!(code, expected, "{label}, changed={member}, policy={policy:?}: {output}");
        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&inventory).unwrap()).unwrap();
        let unknowns: Vec<_> = data["diagnostics"].as_array().unwrap().iter()
            .filter(|d| d["code"] == "constant_lookup_unknown").collect();
        if member == "TOKEN" {
            assert!(!unknowns.is_empty(), "lost uncertain Child::TOKEN read: {output}");
            assert!(unknowns.iter().all(|d| d["severity"] == serde_json::to_value(policy).unwrap()));
            assert!(data["named_unknowns"]["groups"].as_array().unwrap().iter()
                .any(|g| g["name"] == "Child::TOKEN" && g["through"] == "Zork::Mix"), "{data}");
        } else {
            assert!(unknowns.is_empty(), "an unrelated name must not obscure TOKEN: {output}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_round2_singleton_foreign_receiver_policy_controls_exit_status() {
    round2_effect_exit_status("singleton-receiver", "class << Other; Mix.const_set(:TOKEN, 42); end");
}

#[test]
fn review_round2_singleton_qualified_write_policy_controls_exit_status() {
    round2_effect_exit_status("singleton-write", "class << Other; Mix::TOKEN = 42; end");
}

#[test]
fn review_round2_conditional_qualified_write_policy_controls_exit_status() {
    round2_effect_exit_status("conditional-write", "if true; Mix::TOKEN = 42; end");
}

#[test]
fn review_round2_initializer_qualified_write_policy_controls_exit_status() {
    round2_effect_exit_status("initializer-write", "HOLDER = (Mix::TOKEN = 42)");
}

#[test]
fn named_unknown_console_groups_five_reads_into_two_pairs_and_bounds_large_lists() {
    let root = std::env::temp_dir().join(format!("roundhouse-round2-groups-{}", std::process::id()));
    for (path, source) in [
        ("Gemfile.lock", "GEM\n  specs:\n    wrapper (1.0)\n      zork\n    zork (1.0)\n\nDEPENDENCIES\n  wrapper\n"),
        ("config/routes.rb", "Rails.application.routes.draw do; end"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base; end"),
        ("local/gems/zork-1.0/lib/zork.rb", "module Zork; module Mix; eval('TOKEN=42'); end; module Other; eval('TOKEN=42'); end; module Known; end; end"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let local = root.join("local");
    let controller = root.join("app/controllers/things_controller.rb");
    let inventory = root.join("inventory.json");
    let expected = if roundhouse::diagnostic::GEM_DECLARATION_UNKNOWN_SEVERITY == roundhouse::diagnostic::Severity::Error { 1 } else { 0 };
    for many in [false, true] {
        let reads = if many { (0..7).map(|n| format!("@a = Zork::Mix::NAME{n}")).collect::<Vec<_>>() }
            else { ["Zork::Mix::TOKEN", "Zork::Mix::TOKEN", "Zork::Mix::TOKEN", "Zork::Other::TOKEN", "Zork::Other::TOKEN"]
                .iter().map(|name| format!("@a = {name}")).collect::<Vec<_>>() };
        std::fs::write(&controller, format!("class ThingsController < ApplicationController\ndef show\n{}\n@known = Zork::Known\nend\nend", reads.join("\n"))).unwrap();
        let (groups, occurrences) = if many { (7, 7) } else { (2, 5) };
        for detail in [false, true] {
            let mut args = vec!["--no-hotspots", "--inventory", inventory.to_str().unwrap(), "--gem-declarations-path", local.to_str().unwrap()];
            if detail { args.push("--continue"); }
            args.push(root.to_str().unwrap());
            let (code, output) = check(&args);
            assert_eq!(code, expected, "detail={detail}: {output}");
            assert!(output.contains(&format!("{groups} name/namespace group(s), {occurrences} source occurrence(s)")), "{output}");
            assert_eq!(output.matches("source occurrences; first at").count(), if detail { groups } else { groups.min(5) }, "{output}");
            assert_eq!(output.contains("listing abbreviated"), many && !detail, "{output}");
            let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&inventory).unwrap()).unwrap();
            assert_eq!(data["named_unknowns"]["group_count"], groups);
            assert_eq!(data["named_unknowns"]["source_occurrences"], occurrences);
            assert_eq!(data["named_unknowns"]["groups"].as_array().unwrap().len(), groups);
            assert_eq!(data["diagnostics"].as_array().unwrap().iter().filter(|d| d["code"] == "constant_lookup_unknown").count(), occurrences);
            assert!(data["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "constant_loading_unverified"), "loading-only edges must remain separate");
            if !many {
                let counts: Vec<_> = data["named_unknowns"]["groups"].as_array().unwrap().iter().map(|g| g["occurrences"].as_u64().unwrap()).collect();
                assert_eq!(counts, vec![3, 2]);
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
