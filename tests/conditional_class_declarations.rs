//! A class declared under a load-time conditional in a gem exists only in
//! the branch Ruby takes.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn native(files: &[(&str, &str)], script: &str, env: &[(&str, &str)]) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("rh_conditional_class_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let file = root.join("lib").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let mut command = emit_and_run::ruby();
    command.arg("-I").arg(root.join("lib")).arg("-e").arg(script);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn same_as_native(files: &[(&str, &str)], entry: &str, script: &str, runs: &[(&[(&str, &str)], &str)]) {
    for (env, expected) in runs {
        assert_eq!(native(files, &format!("require {entry:?}\n{script}"), env), *expected, "native control under {env:?}");
    }
    for (env, expected) in runs {
        let mut overlay = emit_and_run::empty_app()
            .write("roundhouse.yml", "")
            .write("Gemfile.lock", "PATH\n  remote: vendor/gems/gqlc-1.0.0\n  specs:\n    gqlc (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  gqlc!\n")
            .write("config/application.rb", &format!("require {entry:?}\n"));
        for (path, text) in files {
            overlay = overlay.write(&format!("vendor/gems/gqlc-1.0.0/lib/{path}"), text);
        }
        let run = overlay.run_ruby_env(script, env);
        assert!(run.success, "emitted tree failed under {env:?} in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
        assert!(!run.stderr.contains("not_carried"), "{}", run.stderr);
        assert_eq!(run.stdout, *expected, "under {env:?}: {}", run.stderr);
    }
}

const VISITOR: &str = "module Gqlc\n  class Visitor\n    def initialize(doc)\n      @doc = doc\n    end\n\n    def visit = [:base, @doc]\n  end\nend\n";

/// query_typename.rb's shape: the class and the method that builds it
/// in one branch, the other branch's method alone, and a method after
/// the conditional.
const QUERY_TYPENAME: &str = "require \"gqlc/visitor\"\n\nmodule Gqlc\n  module QueryTypename\n    if ENV[\"GQLC_LEGACY\"].nil?\n      class InsertVisitor < Gqlc::Visitor\n        def initialize(doc, types:)\n          @types = types\n          super(doc)\n        end\n\n        def visit = [:insert, @types, *super]\n      end\n\n      def self.insert(doc) = InsertVisitor.new(doc, types: 1).visit\n    else\n      def self.insert(doc) = [:legacy, doc]\n    end\n\n    def self.flatten = 1\n  end\nend\n";

#[test]
fn a_class_declared_in_a_module_body_conditional_exists_only_in_its_branch() {
    same_as_native(
        &[("gqlc/visitor.rb", VISITOR), ("gqlc/query_typename.rb", QUERY_TYPENAME)],
        "gqlc/query_typename",
        "p Gqlc::QueryTypename.insert(:d), defined?(Gqlc::QueryTypename::InsertVisitor), Gqlc::QueryTypename.flatten\n",
        &[
            (&[], "[:insert, 1, :base, :d]\n\"constant\"\n1\n"),
            (&[("GQLC_LEGACY", "1")], "[:legacy, :d]\nnil\n1\n"),
        ],
    );
}

/// `elsif` / `else` arms declaring a class, a module and a class beside
/// a method, then an `unless` declaring a class alone. The gem's entry
/// requires the file, as client.rb requires query_typename.rb.
const SHAPES: &str = "module Gqlc\n  module Shapes\n    if ENV[\"GQLC_SHAPE\"] == \"a\"\n      class Alpha\n        def self.tag = :alpha\n      end\n    elsif ENV[\"GQLC_SHAPE\"] == \"b\"\n      module Beta\n        def self.tag = :beta\n      end\n    else\n      class Gamma\n        def self.tag = :gamma\n      end\n\n      def self.fallback = Gamma.tag\n    end\n\n    unless ENV[\"GQLC_SHAPE\"] == \"a\"\n      class Delta\n        def self.tag = :delta\n      end\n    end\n\n    def self.names = constants.sort\n  end\nend\n";

#[test]
fn elsif_else_and_unless_arms_declare_what_their_branch_runs() {
    same_as_native(
        &[("gqlc.rb", "require \"gqlc/shapes\"\n"), ("gqlc/shapes.rb", SHAPES)],
        "gqlc",
        "p Gqlc::Shapes.names, Gqlc::Shapes.respond_to?(:fallback)\np Gqlc::Shapes.names.map { |n| Gqlc::Shapes.const_get(n).tag }\n",
        &[
            (&[("GQLC_SHAPE", "a")], "[:Alpha]\nfalse\n[:alpha]\n"),
            (&[("GQLC_SHAPE", "b")], "[:Beta, :Delta]\nfalse\n[:beta, :delta]\n"),
            (&[], "[:Delta, :Gamma]\ntrue\n[:delta, :gamma]\n"),
        ],
    );
}

/// The same at file level, which the file program already carried.
const TOP: &str = "if ENV[\"GQLC_TOP\"]\n  module Gqlc\n    class Top\n      def self.tag = :top\n    end\n  end\nend\n\nmodule Gqlc\n  def self.top = const_defined?(:Top) ? Top.tag : nil\nend\n";

#[test]
fn a_class_declared_in_a_file_body_conditional_exists_only_in_its_branch() {
    same_as_native(
        &[("gqlc/top.rb", TOP)],
        "gqlc/top",
        "p Gqlc.top\n",
        &[(&[("GQLC_TOP", "1")], ":top\n"), (&[], "nil\n")],
    );
}

/// A class declared in a conditional and reopened unconditionally by
/// another file. Natively the reopening defines it whether or not the
/// branch runs; the IR merges both bodies into one class, which the
/// emitted tree writes inside the branch. Not modeled: named at the
/// conditional declaration, with the reopening file.
const REOPENED: &[(&str, &str)] = &[
    (
        "gqlc/q.rb",
        "module Gqlc\n  module Q\n    if ENV[\"GQLC_LEGACY\"].nil?\n      class V\n        def a = 1\n      end\n    end\n\n    def self.f = 1\n  end\nend\n",
    ),
    ("gqlc/q_ext.rb", "require \"gqlc/q\"\n\nmodule Gqlc\n  module Q\n    class V\n      def b = 2\n    end\n  end\nend\n"),
];

#[test]
fn a_conditional_declaration_reopened_in_another_file_is_named() {
    let script = "p Gqlc::Q::V.instance_methods(false).sort\n";
    assert_eq!(native(REOPENED, &format!("require \"gqlc/q_ext\"\n{script}"), &[]), "[:a, :b]\n");
    assert_eq!(native(REOPENED, &format!("require \"gqlc/q_ext\"\n{script}"), &[("GQLC_LEGACY", "1")]), "[:b]\n");

    let root = std::env::temp_dir().join(format!("rh_conditional_reopened_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let gem: Vec<(String, &str)> = REOPENED.iter().map(|(path, text)| (format!("vendor/gems/gqlc-1.0.0/lib/{path}"), *text)).collect();
    for (path, text) in [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("config/application.rb", "require \"gqlc/q_ext\"\n"),
        ("roundhouse.yml", ""),
        ("Gemfile.lock", "PATH\n  remote: vendor/gems/gqlc-1.0.0\n  specs:\n    gqlc (1.0.0)\n\nDEPENDENCIES\n  gqlc!\n"),
    ]
    .into_iter()
    .chain(gem.iter().map(|(path, text)| (path.as_str(), *text)))
    {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let row = text.lines().find(|l| l.contains("q.rb:4:7:")).unwrap_or_else(|| panic!("{text}"));
    assert!(row.contains("error[unsupported]"), "{row}");
    assert!(
        row.contains("`Gqlc::Q::V` is declared in a load-time conditional here and unconditionally in vendor/gems/gqlc-1.0.0/lib/gqlc/q_ext.rb"),
        "{row}"
    );
}
