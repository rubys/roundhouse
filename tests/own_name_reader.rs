//! An instance whose class defines its own `name` is answered by that
//! method, not by the `Module#name` fallback. The first typing pass runs
//! before any return is harvested, so `name` there fell through to the
//! Module introspection fallback and typed as String. From the next round
//! on the reader answers, but the stray String could circulate in a ring
//! of methods that rewrite each other's types every round, and the
//! fixpoint then ran to its cap. A class with no `name` of its own keeps
//! the String fallback.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{Analyzer, FixpointRounds, LoopEnd};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

/// A settings store: `[]=` writes what `Setting#value_for` returns, and
/// `[]` passes what was stored back to it. The default value comes from an
/// options Hash that also holds the namespace's `name`.
const SETTINGS: &[(&str, &str)] = &[
    (
        "app/models/namespace.rb",
        r#"class Namespace
  attr_reader :name

  def initialize(name)
    @name = name.to_sym
  end

  def setting(options = {})
    options[:namespace] = name
    Setting.new(options)
  end
end
"#,
    ),
    (
        "app/models/setting.rb",
        r#"class Setting
  def initialize(options = {})
    @default_value = options[:default]
  end

  def value_for(original_value)
    original_value.nil? ? @default_value : original_value
  end
end
"#,
    ),
    (
        "app/models/settings.rb",
        r#"class Settings
  DEFINITION = Namespace.new(:web).setting(default: true)

  def initialize(original_hash)
    @original_hash = original_hash
  end

  def [](key)
    DEFINITION.value_for(@original_hash[key])
  end

  def []=(key, value)
    @original_hash[key] = DEFINITION.value_for(value)
  end
end
"#,
    ),
];

/// `pick` answers the label of a `Tag`, a Symbol, or what was stored.
const RING: &[(&str, &str)] = &[
    (
        "app/models/tag.rb",
        r#"class Tag
  attr_reader :name

  def initialize(name)
    @name = name.to_sym
  end

  def label
    name
  end
end
"#,
    ),
    (
        "app/models/ring.rb",
        r#"class Ring
  def initialize
    @store = {}
  end

  def pick(key, original)
    original.nil? ? Tag.new("x").label : original
  end

  def write(key, value)
    @store[key] = pick(key, value)
  end

  def read(key)
    pick(key, @store[key])
  end
end
"#,
    ),
    (
        "app/models/user.rb",
        r#"class User
  def remember
    ring = Ring.new
    ring.write(:k, :v)
    ring.read(:k)
  end
end
"#,
    ),
];

/// A controller helper `name` that answers a Symbol. The action writes it
/// into a ring through `label` and reads it back.
const CONTROLLER: &[(&str, &str)] = &[
    (
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::Base\nend\n",
    ),
    (
        "app/controllers/tags_controller.rb",
        r#"class TagsController < ApplicationController
  def show
    ring = Ring.new
    ring.write(:k, label)
    @value = ring.read(:k)
  end

  private

  def label
    name
  end

  def name
    :tags
  end
end
"#,
    ),
    (
        "app/models/ring.rb",
        r#"class Ring
  def initialize
    @store = {}
  end

  def pick(key, original)
    original.nil? ? :none : original
  end

  def write(key, value)
    @store[key] = pick(key, value)
  end

  def read(key)
    pick(key, @store[key])
  end
end
"#,
    ),
];

/// No `name` of its own. The class-side send is `Module#name`. Ruby raises
/// `NameError` for the instance send; the analyzer keeps its String
/// fallback there, which this pins.
const PLAIN: &[(&str, &str)] = &[(
    "app/models/plain.rb",
    r#"class Plain
  def self.title
    name
  end

  def label
    name
  end
end
"#,
)];

/// A literal send dispatches on its receiver's side for every method, not
/// only `name`: an instance's `send(:kind)` reaches the instance's `kind`,
/// and a class method's reaches the class method.
const SIDES: &[(&str, &str)] = &[(
    "app/models/widget.rb",
    r#"class Widget
  def self.kind
    1
  end

  def kind
    :k
  end

  def self.own_kind
    send(:kind)
  end

  def self.instance_kind
    new.send(:kind)
  end

  def sent_kind
    public_send(:kind)
  end
end
"#,
)];

struct Analyzed {
    rounds: FixpointRounds,
    returns: HashMap<(String, String, bool), Ty>,
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
    for (id, info) in analyzer.class_registry() {
        for (class_side, table) in [(false, &info.instance_methods), (true, &info.class_methods)] {
            for (method, ty) in table {
                returns.insert((id.0.as_str().to_string(), method.as_str().to_string(), class_side), ty.clone());
            }
        }
    }
    Analyzed { rounds: analyzer.fixpoint_rounds(), returns }
}

fn ret(analyzed: &Analyzed, class: &str, method: &str, class_side: bool) -> Ty {
    let key = (class.to_string(), method.to_string(), class_side);
    match analyzed.returns.get(&key) {
        Some(Ty::Fn { ret, .. }) => (**ret).clone(),
        Some(ty) => ty.clone(),
        None => panic!("{class}{}{method} not registered", if class_side { "." } else { "#" }),
    }
}

fn arms(ty: &Ty) -> Vec<Ty> {
    match ty {
        Ty::Union { variants } => variants.to_vec(),
        other => vec![other.clone()],
    }
}

#[test]
fn a_ring_through_its_own_name_reader_settles() {
    let analyzed = analyze(SETTINGS);
    let rounds = analyzed.rounds;
    assert!(
        matches!(rounds.production, LoopEnd::Settled(_))
            && matches!(rounds.views_and_tests, LoopEnd::Settled(_))
            && rounds.absorb != LoopEnd::RanToCap,
        "the fixpoint stopped on a round cap: {rounds:?}"
    );
    let value = ret(&analyzed, "Setting", "value_for", false);
    assert!(!arms(&value).contains(&Ty::Str), "value_for: {value:?}");
}

#[test]
fn a_ring_keeps_no_string_the_reader_never_returns() {
    let analyzed = analyze(RING);
    assert_eq!(ret(&analyzed, "Tag", "label", false), Ty::Sym);
    assert_eq!(ret(&analyzed, "Ring", "read", false), Ty::Sym);
}

#[test]
fn a_ring_through_send_keeps_no_string_the_reader_never_returns() {
    let tag = RING[0].1.replace("    name\n", "    send(:name)\n");
    assert_ne!(tag, RING[0].1);
    let analyzed = analyze(&[(RING[0].0, tag.as_str()), RING[1], RING[2]]);
    assert_eq!(ret(&analyzed, "Ring", "read", false), Ty::Sym);
}

#[test]
fn a_controller_ring_keeps_no_string_its_name_helper_never_returns() {
    let analyzed = analyze(CONTROLLER);
    assert_eq!(ret(&analyzed, "TagsController", "label", false), Ty::Sym);
    assert_eq!(ret(&analyzed, "Ring", "read", false), Ty::Sym);
}

#[test]
fn a_class_without_its_own_name_keeps_the_string_fallback() {
    let analyzed = analyze(PLAIN);
    assert_eq!(ret(&analyzed, "Plain", "label", false), Ty::Str);
    assert_eq!(ret(&analyzed, "Plain", "title", true), Ty::Str);
}

#[test]
fn a_literal_send_keeps_its_receivers_side() {
    let analyzed = analyze(SIDES);
    assert_eq!(ret(&analyzed, "Widget", "sent_kind", false), Ty::Sym);
    assert_eq!(ret(&analyzed, "Widget", "instance_kind", true), Ty::Sym);
    assert_eq!(ret(&analyzed, "Widget", "own_kind", true), Ty::Int);
}
