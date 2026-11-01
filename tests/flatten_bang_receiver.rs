//! `xs.flatten!` replaces the array's contents, so later reads of `xs`
//! answer the flattened element. The binding used to keep the element it
//! was bound with: a parameter whose flattened value is stored and passed
//! back to it then gained an Array level every round, and the production
//! fixpoint ran to its cap. `flatten(depth)` and `flatten!(depth)` unwrap
//! `depth` levels, and `flatten!` answers nil when nothing was nested.

use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::analyze::{Analyzer, FixpointRounds, LoopEnd};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

/// `prepare` flattens its rest parameter one level and stores it, and
/// `replay` passes what was stored back as one argument.
const REGISTRY: &str = r#"class Registry
  def initialize
    @sets = {}
  end

  def prepare(name, *fields)
    fields.flatten!(1)
    @sets[name] = fields
    fields.size
  end

  def replay(name)
    prepare(name, @sets[name])
  end

  def fields_of(name)
    @sets[name]
  end
end
"#;

const REGISTRY_CALLER: &str = r#"class RegistryCaller
  def run
    Registry.new.prepare("set", "a", "b")
  end
end
"#;

const PACKER: &str = r#"class Packer
  def flattened
    xs = [["a"], "b"]
    xs.flatten!
    xs
  end

  def flattened_one_level
    xs = [[["a"]], "b"]
    xs.flatten!(1)
    xs
  end

  def flattened_no_level
    xs = [["a"], "b"]
    xs.flatten!(0)
    xs
  end

  def flattened_negative_level
    xs = [[["a"]], "b"]
    xs.flatten!(-1)
    xs
  end

  def flattened_nil_level
    xs = [[["a"]], "b"]
    xs.flatten!(nil)
    xs
  end

  def flattened_in_a_branch(go)
    xs = [["a"], "b"]
    xs.flatten! if go
    xs
  end

  def flattened_in_a_block
    xs = [[["a"]]]
    2.times { xs.flatten!(1) }
    xs
  end

  def flattened_no_level_in_a_block
    xs = [[["a"]], "b"]
    2.times { xs.flatten!(0) }
    xs
  end

  def flattened_everything_in_a_block
    xs = [[["a"]], "b"]
    2.times { xs.flatten! }
    xs
  end

  def flattened_ivar
    @xs = [["a"], "b"]
    @xs.flatten!
    @xs
  end

  def flatten_bang_answer
    xs = [["a"], "b"]
    xs.flatten!(1)
  end

  def flatten_one_level
    [[["a"]]].flatten(1)
  end

  def flatten_every_level
    [[["a"]]].flatten
  end

  def flatten_nil_level
    [[["a"]]].flatten(nil)
  end

  def flatten_run_time_level(depth)
    [[["a"]]].flatten(depth)
  end
end
"#;

struct Analyzed {
    rounds: FixpointRounds,
    returns: HashMap<(String, String), Ty>,
    params: HashMap<(String, String), Vec<Ty>>,
}

fn analyze(files: &[(&str, &str)]) -> Analyzed {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    for (path, src) in files {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let mut returns = HashMap::new();
    let mut params = HashMap::new();
    for (id, info) in analyzer.class_registry() {
        for (method, ty) in &info.instance_methods {
            let key = (id.0.as_str().to_string(), method.as_str().to_string());
            let ret = match ty {
                Ty::Fn { ret, .. } => (**ret).clone(),
                other => other.clone(),
            };
            returns.insert(key.clone(), ret);
            params.insert(key, analyzer.inferred_param_types(id, method).unwrap_or_default().to_vec());
        }
    }
    Analyzed { rounds: analyzer.fixpoint_rounds(), returns, params }
}

fn ret(analyzed: &Analyzed, class: &str, method: &str) -> Ty {
    analyzed
        .returns
        .get(&(class.to_string(), method.to_string()))
        .unwrap_or_else(|| panic!("{class}#{method} not registered"))
        .clone()
}

fn arr(elem: Ty) -> Ty {
    Ty::Array { elem: std::sync::Arc::new(elem) }
}

fn union(arms: &[Ty]) -> Ty {
    Ty::Union { variants: arms.to_vec().into() }
}

/// Equal up to the order of union arms, at any depth.
fn same(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Union { variants: x }, Ty::Union { variants: y }) => {
            x.len() == y.len() && x.iter().all(|t| y.iter().any(|u| same(t, u)))
        }
        (Ty::Array { elem: x }, Ty::Array { elem: y }) => same(x, y),
        _ => a == b,
    }
}

fn assert_same(got: &Ty, expected: &Ty) {
    assert!(same(got, expected), "expected {expected:?}, got {got:?}");
}

/// How many Arrays deep `ty` nests.
fn array_depth(ty: &Ty) -> usize {
    match ty {
        Ty::Array { elem } => 1 + array_depth(elem),
        Ty::Union { variants } => variants.iter().map(array_depth).max().unwrap_or(0),
        _ => 0,
    }
}

#[test]
fn a_parameter_fed_its_own_flattened_value_settles() {
    let analyzed = analyze(&[
        ("app/models/registry.rb", REGISTRY),
        ("app/models/registry_caller.rb", REGISTRY_CALLER),
    ]);
    let rounds = analyzed.rounds;
    assert!(
        matches!(rounds.production, LoopEnd::Settled(_))
            && matches!(rounds.views_and_tests, LoopEnd::Settled(_))
            && rounds.absorb != LoopEnd::RanToCap,
        "the fixpoint stopped on a round cap: {rounds:?}"
    );
    let field = &analyzed.params[&("Registry".to_string(), "prepare".to_string())][1];
    assert_eq!(array_depth(field), 1, "a field of prepare: {field:?}");
    let stored = ret(&analyzed, "Registry", "fields_of");
    assert_eq!(array_depth(&stored), 1, "fields_of: {stored:?}");
}

#[test]
fn a_statement_level_flatten_bang_rebinds_its_receiver() {
    let analyzed = analyze(&[("app/models/packer.rb", PACKER)]);
    assert_eq!(ret(&analyzed, "Packer", "flattened"), arr(Ty::Str));
    assert_same(&ret(&analyzed, "Packer", "flattened_one_level"), &arr(union(&[Ty::Str, arr(Ty::Str)])));
    assert_same(&ret(&analyzed, "Packer", "flattened_no_level"), &arr(union(&[Ty::Str, arr(Ty::Str)])));
    assert_eq!(ret(&analyzed, "Packer", "flattened_negative_level"), arr(Ty::Str));
    assert_eq!(ret(&analyzed, "Packer", "flattened_nil_level"), arr(Ty::Str));
    assert_eq!(ret(&analyzed, "Packer", "flattened_ivar"), arr(Ty::Str));
}

#[test]
fn a_flatten_bang_that_may_not_run_keeps_the_nested_element() {
    let analyzed = analyze(&[("app/models/packer.rb", PACKER)]);
    assert_same(&ret(&analyzed, "Packer", "flattened_in_a_branch"), &arr(union(&[Ty::Str, arr(Ty::Str)])));
    assert_same(
        &ret(&analyzed, "Packer", "flattened_in_a_block"),
        &arr(union(&[Ty::Str, arr(union(&[Ty::Str, arr(Ty::Str)]))])),
    );
    assert_same(
        &ret(&analyzed, "Packer", "flattened_no_level_in_a_block"),
        &arr(union(&[Ty::Str, arr(arr(Ty::Str))])),
    );
    assert_same(
        &ret(&analyzed, "Packer", "flattened_everything_in_a_block"),
        &arr(union(&[Ty::Str, arr(arr(Ty::Str))])),
    );
}

#[test]
fn flatten_unwraps_as_many_levels_as_its_depth() {
    let analyzed = analyze(&[("app/models/packer.rb", PACKER)]);
    assert_same(&ret(&analyzed, "Packer", "flatten_bang_answer"), &union(&[arr(Ty::Str), Ty::Nil]));
    assert_eq!(ret(&analyzed, "Packer", "flatten_one_level"), arr(arr(Ty::Str)));
    assert_eq!(ret(&analyzed, "Packer", "flatten_every_level"), arr(Ty::Str));
    assert_eq!(ret(&analyzed, "Packer", "flatten_nil_level"), arr(Ty::Str));
    assert_same(
        &ret(&analyzed, "Packer", "flatten_run_time_level"),
        &arr(union(&[Ty::Str, arr(union(&[Ty::Str, arr(Ty::Str)]))])),
    );
}

/// A clean `check` on the registry is a claim that it runs.
#[test]
fn the_registry_runs_once_emitted() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/registry.rb", REGISTRY)
        .write("app/models/registry_caller.rb", REGISTRY_CALLER)
        .run_ruby(
            r#"
raise "prepare: wrong size" unless RegistryCaller.new.run == 2
registry = Registry.new
raise "prepare: nested argument" unless registry.prepare("set", ["a", ["b"]], "c") == 3
raise "stored: #{registry.fields_of("set").inspect}" unless registry.fields_of("set") == ["a", ["b"], "c"]
raise "replay: wrong size" unless registry.replay("set") == 3
raise "replayed: #{registry.fields_of("set").inspect}" unless registry.fields_of("set") == ["a", ["b"], "c"]
puts "registry ran"
"#,
        )
        .assert_passes();
}
