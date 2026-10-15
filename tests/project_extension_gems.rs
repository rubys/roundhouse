//! Extension facts must describe the gem that runs, not just a typed name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{self, BuildTarget};
use roundhouse::App;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "roundhouse-extension-gems-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(lock: &str, extension: &str, rbi: Option<(&str, &str)>, source: &str) -> App {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        ("roundhouse.yml", "extensions: [config/rh]\n"),
        ("config/rh/gems.yml", extension),
        ("Gemfile.lock", lock),
        ("app/models/report.rb", source),
    ].into_iter().map(|(path, text)| (path.into(), text.as_bytes().to_vec())).collect();
    if let Some((path, text)) = rbi {
        files.insert(path.into(), text.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(files).expect("ingest extension app");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn emit(app: &App, target: BuildTarget, root: &Path) {
    let files = project::target_files(app, Path::new("."), target).expect("emit extension app");
    project::write_to_dir(&files, root).unwrap();
}

fn ruby(root: &Path, home: &Path, script: &str) -> Output {
    Command::new("ruby").arg("-e").arg(script).current_dir(root)
        .env("GEM_HOME", home).env("GEM_PATH", home)
        .env_remove("BUNDLE_GEMFILE").env_remove("RUBYOPT").env_remove("RUBYLIB")
        .output().expect("execute emitted Ruby")
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

const ENGINE_LOCK: &str = "GEM\n  specs:\n    widget-engine (1.0)\n\nDEPENDENCIES\n  widget-engine\n";
const REFUSED: &str = "rails_dependent:\n  - {gem: widget-engine, why: requires the Rails framework}\n";
const ENGINE_RBI: &str = "module Vendor\n  class RemoteClient\n    TOKEN = T.let(T.unsafe(nil), String)\n    sig { returns(String) }\n    def self.label; end\n  end\nend\n";

#[test]
fn adding_an_rbi_does_not_remove_a_rails_dependent_refusal() {
    let rbi = "module WidgetEngine\n  class Client\n    sig { returns(String) }\n    def self.label; end\n  end\nend\n";
    for declaration in [None, Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", rbi))] {
        let app = app(ENGINE_LOCK, REFUSED, declaration,
            "class Report\n  def self.value\n    WidgetEngine::Client.label\n  end\nend\n");
        assert!(diagnose(&app).iter().any(|diagnostic| matches!(&diagnostic.kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "gem_requires_rails")));
        assert!(project::target_files(&app, Path::new("."), BuildTarget::Ruby).is_err());
    }
}

#[test]
fn rbi_classes_and_values_cannot_bypass_the_owning_gem_refusal() {
    for expression in ["Vendor::RemoteClient.label", "Vendor::RemoteClient::TOKEN"] {
        let source = format!("class Report\n  def self.value\n    {expression}\n  end\nend\n");
        let app = app(ENGINE_LOCK, REFUSED,
            Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", ENGINE_RBI)), &source);
        let diagnostics = diagnose(&app);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == Severity::Error && matches!(&diagnostic.kind,
                DiagnosticKind::Unsupported { construct, detail, .. }
                    if construct.as_str() == "gem_requires_rails"
                        && detail.contains("widget-engine")
                        && detail.contains("requires the Rails framework"))
        }), "{diagnostics:#?}");
        for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Rust] {
            assert!(project::target_files(&app, Path::new("."), target).is_err(), "{target:?}");
        }
    }
}

#[test]
fn rbi_refusal_covers_inheritance_but_not_an_app_owned_namesake() {
    let inherited = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", ENGINE_RBI)),
        "class Report < Vendor::RemoteClient\nend\n");
    assert!(diagnose(&inherited).iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "gem_requires_rails")));

    let owned = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", ENGINE_RBI)),
        "module Vendor\n  class RemoteClient\n    def self.label\n      'owned by the app'\n    end\n  end\nend\nclass Report\n  def self.value\n    Vendor::RemoteClient.label\n  end\nend\n");
    assert!(!diagnose(&owned).iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "gem_requires_rails")));
}

const TELEMETRY_LOCK: &str = "GEM\n  specs:\n    opentelemetry-api (1.0)\n\nDEPENDENCIES\n  opentelemetry-api\n";
const TELEMETRY_RBI: &str = "module TelemetryProbe\n  sig { returns(String) }\n  def self.label; end\nend\n";
const TELEMETRY_RBI_PATH: &str = "sorbet/rbi/gems/opentelemetry-api@1.0.rbi";

#[test]
fn extension_facts_select_infrastructure_rbis_without_leaking_to_another_app() {
    let source = "class ReportController < ApplicationController\n  def show\n    @value = TelemetryProbe.label.not_a_string_method\n  end\nend\n";
    let build = |extension: &str| {
        let files = [
            ("roundhouse.yml", "extensions: [config/rh]\n"),
            ("config/rh/gems.yml", extension),
            ("Gemfile.lock", TELEMETRY_LOCK),
            (TELEMETRY_RBI_PATH, TELEMETRY_RBI),
            ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
            ("app/controllers/report_controller.rb", source),
            ("config/routes.rb", "Rails.application.routes.draw { get '/report', to: 'report#show' }\n"),
        ];
        let tree = files.into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).unwrap();
        Analyzer::new(&app).analyze(&mut app);
        app
    };
    let extended = build("runtime_gems:\n  - {constant: TelemetryProbe, gem: opentelemetry-api}\n");
    let bare = build("{}\n");
    let typed = diagnose(&extended);
    assert!(typed.iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::SendDispatchFailed { method, recv_ty }
            if method.as_str() == "not_a_string_method" && *recv_ty == roundhouse::Ty::Str)), "{typed:#?}");
    assert!(!diagnose(&bare).iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "not_a_string_method")));
    // Reanalyzing the first app must not use the second app's facts.
    let mut extended = extended;
    Analyzer::new(&extended).analyze(&mut extended);
    assert!(diagnose(&extended).iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::SendDispatchFailed { method, .. } if method.as_str() == "not_a_string_method")));
}

#[test]
fn a_lock_gated_rbi_has_a_native_runtime_not_a_strict_target_runtime() {
    let app = app(TELEMETRY_LOCK,
        "lock_gated_requires:\n  - {require: telemetry_probe, gem: opentelemetry-api}\n",
        Some((TELEMETRY_RBI_PATH, TELEMETRY_RBI)),
        "class Report\n  def self.value\n    TelemetryProbe.label\n  end\nend\n");
    assert!(!diagnose(&app).iter().any(|diagnostic| diagnostic.severity == Severity::Error));
    assert!(project::target_files(&app, Path::new("."), BuildTarget::Spinel).is_err());
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        assert!(project::target_files(&app, Path::new("."), target).is_ok());
    }
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(directory)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .output().expect("run local Git fixture");
    success(output).trim().to_string()
}

fn git_fixture(scratch: &Scratch) -> (String, String, PathBuf) {
    scratch.write("addon-source/lib/rh_addon.rb", "module RhAddon\n  def self.label\n    'locked Git implementation'\n  end\nend\n");
    let addon = scratch.0.join("addon-source");
    git(&addon, &["init", "-q"]);
    git(&addon, &["add", "."]);
    git(&addon, &["-c", "user.name=Roundhouse tests", "-c", "user.email=roundhouse-tests@example.invalid", "commit", "-qm", "submodule fixture"]);
    scratch.write("repository/components/payments/widgets/rh-widgets.gemspec", "Gem::Specification.new do |spec|\n  spec.name = 'rh-widgets'\n  spec.version = '1.0'\n  spec.summary = 'Git fixture'\n  spec.authors = ['Roundhouse tests']\n  spec.files = ['lib/rh_widgets.rb']\n  spec.require_paths = ['lib']\n  spec.add_runtime_dependency 'rh-addon', '= 1.0'\nend\n");
    scratch.write("repository/components/payments/widgets/rh-addon.gemspec", "Gem::Specification.new do |spec|\n  spec.name = 'rh-addon'\n  spec.version = '1.0'\n  spec.summary = 'Git dependency fixture'\n  spec.authors = ['Roundhouse tests']\n  spec.files = ['vendor/addon/lib/rh_addon.rb']\n  spec.require_paths = ['vendor/addon/lib']\nend\n");
    scratch.write("repository/components/payments/widgets/lib/rh_widgets.rb", "require 'rh_addon'\nmodule RhWidget\n  def self.label\n    RhAddon.label\n  end\nend\n");
    let repository = scratch.0.join("repository");
    git(&repository, &["init", "-q"]);
    git(&repository, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", addon.to_str().unwrap(), "components/payments/widgets/vendor/addon"]);
    git(&repository, &["add", "."]);
    git(&repository, &["-c", "user.name=Roundhouse tests", "-c", "user.email=roundhouse-tests@example.invalid", "commit", "-qm", "fixture"]);
    let revision = git(&repository, &["rev-parse", "HEAD"]);
    let checkout = scratch.0.join("home/bundler/gems").join(format!("repository-{}", &revision[..12]));
    std::fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    git(&scratch.0, &["-c", "protocol.file.allow=always", "clone", "--recurse-submodules", "-q", repository.to_str().unwrap(), checkout.to_str().unwrap()]);
    let lock = format!("GIT\n  remote: {}\n  revision: {revision}\n  glob: components/payments/widgets/*.gemspec\n  submodules: true\n  specs:\n    rh-addon (1.0)\n    rh-widgets (1.0)\n      rh-addon (= 1.0)\n\nDEPENDENCIES\n  rh-widgets!\n", repository.display());
    (lock, revision, checkout)
}

#[test]
fn ruby_family_gems_use_the_locked_git_source_and_execute_its_code() {
    for gated in [false, true] {
        let scratch = Scratch::new();
        let (lock, revision, _) = git_fixture(&scratch);
        let extension = if gated {
            "lock_gated_requires:\n  - {require: rh_widgets, gem: rh-widgets}\n"
        } else {
            "runtime_gems:\n  - {constant: RhWidget, gem: rh-widgets}\n"
        };
        let app = app(&lock, extension,
            Some(("sorbet/rbi/gems/rh-widgets@1.0.rbi", "module RhWidget\n  sig { returns(String) }\n  def self.label; end\nend\n")),
            "class Report\n  def self.value\n    RhWidget.label\n  end\nend\n");
        assert!(!diagnose(&app).iter().any(|diagnostic| diagnostic.severity == Severity::Error));
        for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
            let output = scratch.0.join(target.as_str());
            emit(&app, target, &output);
            let source = success(ruby(&output, &scratch.0.join("home"),
                "require 'bundler'\ndependency = Bundler::Dsl.evaluate('Gemfile', nil, {}).dependencies.find { |entry| entry.name == 'rh-widgets' }\nsource = dependency.source\nprint [source.uri, source.options.fetch('ref'), source.options.fetch('glob'), source.options.fetch('submodules')].join(\"\\n\")\n"));
            assert_eq!(source, format!("{}\n{revision}\ncomponents/payments/widgets/*.gemspec\ntrue", scratch.0.join("repository").display()));
            let explicit_require = if gated { "" } else { "require 'rh_widgets'\n" };
            let script = format!("require_relative 'runtime/gem_facades'\n{explicit_require}require_relative 'app/models/report'\nprint Report.value\n");
            assert_eq!(success(ruby(&output, &scratch.0.join("home"), &script)), "locked Git implementation");
        }
    }
}

fn published_fixture(scratch: &Scratch) {
    scratch.write("home/specifications/rh-widgets-1.0.gemspec", "Gem::Specification.new do |spec|\n  spec.name = 'rh-widgets'\n  spec.version = '1.0'\n  spec.summary = 'Published fixture'\n  spec.authors = ['Roundhouse tests']\n  spec.require_paths = ['lib']\nend\n");
    scratch.write("home/gems/rh-widgets-1.0/lib/rh_widgets.rb", "module RhWidget\n  def self.label\n    'wrong published implementation'\n  end\nend\n");
}

#[test]
fn a_missing_git_checkout_never_falls_back_to_a_published_gem() {
    let scratch = Scratch::new();
    let (lock, revision, checkout) = git_fixture(&scratch);
    std::fs::remove_dir_all(checkout).unwrap();
    published_fixture(&scratch);
    let app = app(&lock, "lock_gated_requires:\n  - {require: rh_widgets, gem: rh-widgets}\n", None,
        "class Report\nend\n");
    let output = scratch.0.join("output");
    emit(&app, BuildTarget::Ruby, &output);
    let result = ruby(&output, &scratch.0.join("home"), "require_relative 'runtime/gem_facades'\nprint RhWidget.label\n");
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("wrong published implementation"));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(scratch.0.join("repository").to_str().unwrap()) && stderr.contains(&revision), "{stderr}");
}

#[test]
fn an_activated_published_gem_cannot_replace_the_locked_git_source() {
    let scratch = Scratch::new();
    let (lock, revision, _) = git_fixture(&scratch);
    published_fixture(&scratch);
    let app = app(&lock, "lock_gated_requires:\n  - {require: rh_widgets, gem: rh-widgets}\n", None,
        "class Report\nend\n");
    let output = scratch.0.join("output");
    emit(&app, BuildTarget::Ruby, &output);
    let result = ruby(&output, &scratch.0.join("home"),
        "require 'rh_widgets'\nrequire_relative 'runtime/gem_facades'\nprint RhWidget.label\n");
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("rh-widgets") && stderr.contains(&revision), "{stderr}");
}

#[test]
fn incomplete_git_metadata_is_an_explicit_emission_error() {
    for metadata in ["  remote: https://example.invalid/widgets.git\n", "  revision: abc123\n"] {
        let lock = format!("GIT\n{metadata}  specs:\n    rh-widgets (1.0)\n\nDEPENDENCIES\n  rh-widgets!\n");
        let app = app(&lock, "runtime_gems:\n  - {constant: RhWidget, gem: rh-widgets}\n", None,
            "class Report\n  def self.value\n    RhWidget.label\n  end\nend\n");
        for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
            let error = project::target_files(&app, Path::new("."), target).expect_err("incomplete Git source must not become a published gem");
            assert!(error.contains("rh-widgets"), "{error}");
        }
    }
}

#[test]
fn optional_missing_features_are_quiet_but_partial_loads_report_the_dependency() {
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.0.join("home")).unwrap();
    scratch.write("broken/rh_optional.rb", "require 'rh_missing_internal_dependency'\n");
    let app = app("GEM\n  specs:\n    rh-optional (1.0)\n\nDEPENDENCIES\n  rh-optional\n",
        "lock_gated_requires:\n  - {require: rh_optional, gem: rh-optional}\n", None, "class Report\nend\n");
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let output = scratch.0.join(target.as_str());
        emit(&app, target, &output);
        let missing = ruby(&output, &scratch.0.join("home"), "require_relative 'runtime/gem_facades'\nprint 'boot continued'\n");
        assert!(missing.stderr.is_empty(), "{}", String::from_utf8_lossy(&missing.stderr));
        assert_eq!(success(missing), "boot continued");
        let script = format!("$LOAD_PATH.unshift({:?})\nrequire_relative 'runtime/gem_facades'\nprint 'boot continued'\n", scratch.0.join("broken").to_str().unwrap());
        let broken = ruby(&output, &scratch.0.join("home"), &script);
        let stderr = String::from_utf8_lossy(&broken.stderr);
        assert!(stderr.contains("rh_optional") && stderr.contains("rh_missing_internal_dependency"), "{stderr}");
        assert_eq!(success(broken), "boot continued");
    }
}

#[test]
fn an_rbi_that_patches_string_does_not_own_the_ruby_core_class() {
    let app = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", "class String\n  def gem_patch; end\nend\n")),
        "class Report\n  def self.value\n    String.new\n  end\nend\n");
    assert!(!diagnose(&app).iter().any(|diagnostic| diagnostic.severity == Severity::Error));
    let scratch = Scratch::new();
    let output = scratch.0.join("output");
    emit(&app, BuildTarget::Ruby, &output);
    assert_eq!(success(ruby(&output, &scratch.0.join("home"),
        "require_relative 'app/models/report'\nprint Report.value.class.name\n")), "String");
}

#[test]
fn ruby_core_exemptions_do_not_cover_gem_owned_child_constants() {
    let app = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", "class String::GemWidget\nend\n")),
        "class Report\n  def self.value\n    ::String::GemWidget.new\n  end\nend\n");
    for path in ["String", "::String"] {
        assert!(roundhouse::gems::rails_dependent_owner(&app, path).is_none(), "{path}");
    }
    for path in ["String::GemWidget", "::String::GemWidget"] {
        let owner = roundhouse::gems::rails_dependent_owner(&app, path)
            .unwrap_or_else(|| panic!("missing gem ownership for {path}"));
        assert_eq!(owner.gem, "widget-engine");
    }
    assert!(diagnose(&app).iter().any(|diagnostic| {
        diagnostic.severity == Severity::Error && matches!(&diagnostic.kind,
            DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "gem_requires_rails")
    }));
    assert!(project::target_files(&app, Path::new("."), BuildTarget::Ruby).is_err());
}

#[test]
fn app_owned_values_take_precedence_over_rbi_ownership() {
    let app = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", ENGINE_RBI)),
        "module Vendor\n  class RemoteClient\n    TOKEN = 'app value'\n  end\nend\nclass Report\n  def self.value\n    Vendor::RemoteClient::TOKEN\n  end\nend\n");
    assert!(!diagnose(&app).iter().any(|diagnostic| diagnostic.severity == Severity::Error));
    let scratch = Scratch::new();
    let output = scratch.0.join("output");
    emit(&app, BuildTarget::Ruby, &output);
    assert_eq!(success(ruby(&output, &scratch.0.join("home"),
        "Dir['app/models/**/*.rb'].sort.each { |file| require File.expand_path(file) }\nprint Report.value\n")), "app value");
}

#[test]
fn an_app_namespace_does_not_claim_a_gem_owned_child() {
    let app = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", ENGINE_RBI)),
        "module Vendor\nend\nclass Report\n  def self.value\n    Vendor::RemoteClient::TOKEN\n  end\nend\n");
    assert!(diagnose(&app).iter().any(|diagnostic| matches!(&diagnostic.kind,
        DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "gem_requires_rails")));
}

#[test]
fn an_rbi_patch_does_not_own_a_framework_class() {
    let app = app(ENGINE_LOCK, REFUSED,
        Some(("sorbet/rbi/gems/widget-engine@1.0.rbi", "class ActiveRecord::Base\n  def gem_patch; end\nend\n")),
        "class Report\n  def self.value\n    ActiveRecord::Base\n  end\nend\n");
    assert!(!diagnose(&app).iter().any(|diagnostic| diagnostic.severity == Severity::Error));
    assert!(project::target_files(&app, Path::new("."), BuildTarget::Ruby).is_ok());
}
