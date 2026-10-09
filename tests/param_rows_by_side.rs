//! A class method and an instance method of one name are two `def`s, and
//! each gets the parameter types of the calls that reach it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::MethodReceiver;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn params_of(files: &[(&str, &str)], class: &str, method: &str, side: MethodReceiver) -> Vec<Ty> {
    let mut tree: HashMap<PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let owner = app.library_classes.iter().find(|c| c.name.0.as_str() == class).expect("class");
    let def = owner
        .methods
        .iter()
        .find(|m| m.name.as_str() == method && m.receiver == side)
        .expect("method");
    let Some(Ty::Fn { params, .. }) = &def.signature else { panic!("typed: {:?}", def.signature) };
    params.iter().map(|p| p.ty.clone()).collect()
}

/// The lab's `name_collision`: `self.class.get(path, options)` is the
/// class-side `get`'s evidence. Sharing one row, the class `get` took the
/// instance `get`'s `Hash[String, Integer]` for `options`.
const CLIENT: &str = "class ApiClient
  def self.get(url, options)
    [url, options]
  end

  def get(path, params = {})
    options = { query: params }
    self.class.get(path, options)
  end
end
";

const CALLER: &str = "class Caller\n  def run\n    ApiClient.new.get(\"items\", { \"page\" => 1 })\n  end\nend\n";

#[test]
fn a_dot_class_call_types_the_class_method_of_that_name() {
    let params = params_of(
        &[("app/services/api_client.rb", CLIENT), ("app/services/caller.rb", CALLER)],
        "ApiClient",
        "get",
        MethodReceiver::Class,
    );
    assert_eq!(params[0], Ty::Str);
    assert!(
        matches!(&params[1], Ty::Hash { key, .. } if **key == Ty::Sym),
        "options is the `{{ query: … }}` hash: {:?}",
        params[1]
    );
}

#[test]
fn the_instance_method_keeps_its_own_callers_types() {
    let params = params_of(
        &[("app/services/api_client.rb", CLIENT), ("app/services/caller.rb", CALLER)],
        "ApiClient",
        "get",
        MethodReceiver::Instance,
    );
    let unknown = || Ty::Var { var: roundhouse::ident::TyVar(0) };
    assert_eq!(params[0], Ty::Str);
    assert_eq!(
        params[1],
        Ty::Hash {
            key: Box::new(Ty::Union { variants: vec![Ty::Str, unknown()] }),
            value: Box::new(Ty::Union { variants: vec![Ty::Int, unknown()] }),
        }
    );
}

/// A constant receiver reaches the class side when both sides define the
/// name, and an instance receiver the instance side.
#[test]
fn each_side_types_from_the_calls_that_reach_it() {
    let files = [
        ("app/services/counter.rb", "class Counter\n  def self.bump(n)\n    n\n  end\n\n  def bump(label)\n    label\n  end\nend\n"),
        ("app/services/caller.rb", "class Caller\n  def run\n    Counter.bump(1)\n    Counter.new.bump(\"a\")\n  end\nend\n"),
    ];
    assert_eq!(params_of(&files, "Counter", "bump", MethodReceiver::Class), vec![Ty::Int]);
    assert_eq!(params_of(&files, "Counter", "bump", MethodReceiver::Instance), vec![Ty::Str]);
}

/// Keywords bind by the class method's own shape. One row for both
/// `def`s held two shapes, so the keywords were left unplaced.
#[test]
fn keywords_bind_by_the_class_methods_own_shape() {
    let files = [
        (
            "app/services/stamp.rb",
            "class Stamp\n  def self.touch!(id, at:)\n    [id, at]\n  end\n\n  def touch!(time)\n    self.class.touch!(1, at: time)\n  end\nend\n",
        ),
        ("app/services/caller.rb", "class Caller\n  def run\n    Stamp.new.touch!(\"noon\")\n  end\nend\n"),
    ];
    assert_eq!(params_of(&files, "Stamp", "touch!", MethodReceiver::Class), vec![Ty::Int, Ty::Str]);
    assert_eq!(params_of(&files, "Stamp", "touch!", MethodReceiver::Instance), vec![Ty::Str]);
}

#[test]
fn a_class_method_called_only_on_the_class_is_unaffected() {
    let params = params_of(
        &[
            ("app/services/greeter.rb", "class Greeter\n  def self.hello(name)\n    name\n  end\nend\n"),
            ("app/services/caller.rb", "class Caller\n  def run\n    Greeter.hello(\"Ann\")\n  end\nend\n"),
        ],
        "Greeter",
        "hello",
        MethodReceiver::Class,
    );
    assert_eq!(params, vec![Ty::Str]);
}

/// Ruby finds a class method up the class-side chain: `Child.fetch`
/// reaches `Grand.fetch` past `Base`'s instance `fetch`, and
/// `Child.new.fetch` reaches `Base#fetch`.
#[test]
fn an_inherited_def_is_found_on_the_calls_side() {
    let files = [
        ("app/services/grand.rb", "class Grand\n  def self.fetch(id)\n    id\n  end\nend\n"),
        ("app/services/base.rb", "class Base < Grand\n  def fetch(key)\n    key\n  end\nend\n"),
        ("app/services/child.rb", "class Child < Base\nend\n"),
        ("app/services/caller.rb", "class Caller\n  def run\n    Child.fetch(1)\n    Child.new.fetch(\"k\")\n  end\nend\n"),
    ];
    assert_eq!(params_of(&files, "Grand", "fetch", MethodReceiver::Class), vec![Ty::Int]);
    assert_eq!(params_of(&files, "Base", "fetch", MethodReceiver::Instance), vec![Ty::Str]);
}

#[test]
fn an_instance_call_passes_a_class_method_on_the_way_up() {
    let files = [
        ("app/services/grand.rb", "class Grand\n  def fetch(key)\n    key\n  end\nend\n"),
        ("app/services/base.rb", "class Base < Grand\n  def self.fetch(id)\n    id\n  end\nend\n"),
        ("app/services/child.rb", "class Child < Base\nend\n"),
        ("app/services/caller.rb", "class Caller\n  def run\n    Child.fetch(1)\n    Child.new.fetch(\"k\")\n  end\nend\n"),
    ];
    assert_eq!(params_of(&files, "Base", "fetch", MethodReceiver::Class), vec![Ty::Int]);
    assert_eq!(params_of(&files, "Grand", "fetch", MethodReceiver::Instance), vec![Ty::Str]);
}
