//! A method whose result reaches its own input must not grow its type
//! every fixpoint round (#518).
//!
//! A return that nests its own previous copy is cut by the harvest
//! (`harvest_return`). These shapes escape that check: a cycle of methods
//! nests the other method's previous return, and a result merged back into
//! the method's own parameter never reappears intact. With an Array element
//! and a Hash value each one doubled per round, and analysis ran out of
//! memory. Every type carried to the next round is bounded instead.
//!
//! Analysis runs on a worker thread, watched for time and resident memory,
//! so that a regression fails in seconds instead of exhausting the machine.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::analyze::{diagnose, Analyzer, Severity};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const TIMEOUT: Duration = Duration::from_secs(60);

/// Each app settles in tens of MiB; without the bound they pass this in
/// seconds.
const MEMORY_LIMIT_MIB: u64 = 1024;

/// The bounds `analyze::fixpoint_bound` applies.
const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 512;

const TWO_METHOD_CYCLE: &str = r#"
class TreesController < ApplicationController
  def index
    @tree = walk_0({ "a" => [1, { "b" => "x" }] })
  end

  private

  def walk_0(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk_1(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk_1(v) }
    else
      value
    end
  end

  def walk_1(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk_0(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk_0(v) }
    else
      value
    end
  end
end
"#;

const MERGED_BACK: &str = r#"
class TreesController < ApplicationController
  def index
    inner = canonical({ "b" => [1, "x"] })
    @tree = canonical({ "a" => inner.merge("d" => 2), "c" => [inner.merge("e" => nil)] })
  end

  private

  def canonical(value)
    case value
    when Hash then value.sort.to_h { |k, v| [k.to_s, canonical(v)] }
    when Array then value.map { |v| canonical(v) }
    else value
    end
  end
end
"#;

const SELF_RECURSIVE: &str = r#"
class TreesController < ApplicationController
  def index
    @tree = walk({ "a" => [1, { "b" => "x" }] })
  end

  private

  def walk(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk(v) }
    else
      value
    end
  end
end
"#;

const CLASS_METHOD_CYCLE: &str = r#"
class Walker
  def self.walk(value)
    if value.is_a?(Hash)
      value.transform_values { |v| step(v) }
    elsif value.is_a?(Array)
      value.map { |v| step(v) }
    else
      value
    end
  end

  def self.step(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk(v) }
    else
      value
    end
  end
end
"#;

const WALKER_CONTROLLER: &str = r#"
class TreesController < ApplicationController
  def index
    @tree = Walker.walk({ "a" => [1, { "b" => "x" }] })
  end
end
"#;

/// Container depth and node count, measured as the bound measures them.
fn measure(ty: &Ty) -> (usize, usize) {
    let (kids, container): (Vec<&Ty>, bool) = match ty {
        Ty::Array { elem } => (vec![elem], true),
        Ty::Hash { key, value } => (vec![key, value], true),
        Ty::Tuple { elems } => (elems.iter().collect(), !elems.is_empty()),
        Ty::Record { row } => (row.fields.values().collect(), !row.fields.is_empty()),
        Ty::Union { variants } => (variants.iter().collect(), false),
        Ty::Class { args, .. } => (args.iter().collect(), !args.is_empty()),
        Ty::Fn { params, block, ret, .. } => (
            params.iter().map(|p| &p.ty).chain(block.as_deref()).chain(std::iter::once(&**ret)).collect(),
            true,
        ),
        _ => (Vec::new(), false),
    };
    let (mut depth, mut nodes) = (0, 1);
    for kid in kids {
        let (d, n) = measure(kid);
        depth = depth.max(d);
        nodes += n;
    }
    (depth + usize::from(container), nodes)
}

fn mentions_container(ty: &Ty) -> bool {
    match ty {
        Ty::Array { .. } | Ty::Hash { .. } => true,
        Ty::Union { variants } => variants.iter().any(mentions_container),
        _ => false,
    }
}

struct Settled {
    returns: Vec<(String, Ty)>,
    params: Vec<(String, Ty)>,
    errors: Vec<String>,
}

/// Analyze an app holding `files` (beside an empty schema, an
/// `ApplicationController`, a root route and an `index` view rendering
/// `@tree`), and read back the settled types of `methods`, keyed
/// `(class, method, class_side)`.
fn analyze(files: &[(&str, &str)], methods: &[(&str, &str, bool)]) -> Settled {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let base = [
        ("db/schema.rb", "ActiveRecord::Schema[8.0].define(version: 0) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  root \"trees#index\"\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/views/trees/index.html.erb", "<%= @tree %>\n"),
    ];
    for (path, src) in base.iter().chain(files) {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    let methods: Vec<(String, String, bool)> =
        methods.iter().map(|(c, m, s)| (c.to_string(), m.to_string(), *s)).collect();
    watched(move || {
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        let registry = analyzer.class_registry();
        let mut returns = Vec::new();
        let mut params = Vec::new();
        for (class, method, class_side) in &methods {
            let id = ClassId(Symbol::from(class.as_str()));
            let name = Symbol::from(method.as_str());
            let info = registry.get(&id).unwrap_or_else(|| panic!("{class} not registered"));
            let table = if *class_side { &info.class_methods } else { &info.instance_methods };
            let ret = table.get(&name).unwrap_or_else(|| panic!("{class}#{method} not registered"));
            returns.push((format!("{class}#{method}"), ret.clone()));
            for (i, ty) in analyzer.inferred_param_types(&id, &name).unwrap_or_default().iter().enumerate() {
                params.push((format!("{class}#{method}[{i}]"), ty.clone()));
            }
        }
        let errors = diagnose(&app)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect();
        Settled { returns, params, errors }
    })
}

/// Run `work` on a worker thread with a main-thread-sized stack, and stop
/// the binary if it outlives [`TIMEOUT`] or the process passes
/// [`MEMORY_LIMIT_MIB`].
fn watched<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new().stack_size(64 << 20);
    worker
        .spawn(move || {
            let _ = tx.send(work());
        })
        .expect("spawn analysis");
    let started = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(done) => return done,
            Err(RecvTimeoutError::Disconnected) => panic!("analysis panicked"),
            Err(RecvTimeoutError::Timeout) => {}
        }
        let resident = resident_mib();
        if started.elapsed() > TIMEOUT || resident.is_some_and(|mib| mib > MEMORY_LIMIT_MIB) {
            // A panic would leave the worker allocating while the other
            // tests run, so stop the whole binary. Written past the test
            // harness's output capture, which exiting would discard.
            let _ = writeln!(
                std::io::stderr(),
                "analysis did not settle after {:?} ({resident:?} MiB resident): a carried type is growing every round",
                started.elapsed()
            );
            std::process::exit(1);
        }
    }
}

/// This process's resident set, where `/proc` reports it.
fn resident_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

fn assert_bounded(settled: &Settled) {
    assert!(settled.errors.is_empty(), "errors: {:#?}", settled.errors);
    for (name, ty) in settled.returns.iter().chain(&settled.params) {
        let (depth, nodes) = measure(ty);
        assert!(
            depth <= MAX_DEPTH && nodes <= MAX_NODES,
            "{name} carries depth {depth}, {nodes} nodes"
        );
    }
    for (name, ty) in &settled.returns {
        assert!(mentions_container(ty), "{name} lost its Hash/Array spine: {ty:?}");
    }
}

#[test]
fn a_two_method_cycle_settles_bounded() {
    let settled = analyze(
        &[("app/controllers/trees_controller.rb", TWO_METHOD_CYCLE)],
        &[("TreesController", "walk_0", false), ("TreesController", "walk_1", false)],
    );
    assert_bounded(&settled);
}

#[test]
fn a_class_method_cycle_settles_bounded() {
    let settled = analyze(
        &[("app/models/walker.rb", CLASS_METHOD_CYCLE), ("app/controllers/trees_controller.rb", WALKER_CONTROLLER)],
        &[("Walker", "walk", true), ("Walker", "step", true)],
    );
    assert_bounded(&settled);
}

#[test]
fn a_result_merged_back_into_its_own_parameter_settles_bounded() {
    let settled = analyze(
        &[("app/controllers/trees_controller.rb", MERGED_BACK)],
        &[("TreesController", "canonical", false)],
    );
    assert_bounded(&settled);
    assert!(!settled.params.is_empty(), "canonical's parameter was never unified");
}

/// The harvest cuts this method's direct self-nesting, but its return
/// still gained a level every round and ran both loops to their caps.
#[test]
fn a_self_recursive_walk_settles_bounded() {
    let settled = analyze(
        &[("app/controllers/trees_controller.rb", SELF_RECURSIVE)],
        &[("TreesController", "walk", false)],
    );
    assert_bounded(&settled);
}

/// The call sites live in the class, so analysis sees the same feedback
/// as the controllers above.
const TREE_WALK: &str = r#"
class TreeWalk
  def walked
    walk_0({ "a" => [1, { "b" => "x" }] })
  end

  def canonicalized
    inner = canonical({ "b" => [1, "x"] })
    canonical({ "a" => inner.merge("d" => 2), "c" => [inner.merge("e" => nil)] })
  end

  def walk_0(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk_1(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk_1(v) }
    else
      value
    end
  end

  def walk_1(value)
    if value.is_a?(Hash)
      value.transform_values { |v| walk_0(v) }
    elsif value.is_a?(Array)
      value.map { |v| walk_0(v) }
    else
      value
    end
  end

  def canonical(value)
    case value
    when Hash then value.sort.to_h { |k, v| [k.to_s, canonical(v)] }
    when Array then value.map { |v| canonical(v) }
    else value
    end
  end
end
"#;

/// A clean `check` on these shapes is a claim that they run: the bounded
/// types must not cost the emitted program its values.
#[test]
fn the_cycle_and_the_merged_back_result_run_once_emitted() {
    let run = watched(|| {
        emit_and_run::empty_app()
            .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
            .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
            .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
            .write("app/models/tree_walk.rb", TREE_WALK)
            .run_ruby(
                r#"
walk = TreeWalk.new
walked = walk.walked
raise "cycle lost values: #{walked.inspect}" unless walked == { "a" => [1, { "b" => "x" }] }
canonical = walk.canonicalized
expected = { "a" => { "b" => [1, "x"], "d" => 2 }, "c" => [{ "b" => [1, "x"], "e" => nil }] }
raise "merge feedback lost values: #{canonical.inspect}" unless canonical == expected
raise "canonical kept a Symbol key" unless walk.canonical({ b: { c: 1 } }) == { "b" => { "c" => 1 } }
puts "recursive walks ran"
"#,
            )
    });
    run.assert_passes();
}
