//! An `x.class.foo` call site is not param evidence for an instance `def foo`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

fn params_of(files: &[(&str, &str)], class: &str, method: &str) -> Vec<Ty> {
    let mut tree: HashMap<PathBuf, Vec<u8>> =
        files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\nend\n".to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let owner = app.library_classes.iter().find(|c| c.name.0.as_str() == class).expect("class");
    let def = owner.methods.iter().find(|m| m.name.as_str() == method).expect("method");
    let Some(Ty::Fn { params, .. }) = &def.signature else { panic!("typed: {:?}", def.signature) };
    params.iter().map(|p| p.ty.clone()).collect()
}

fn params_of_get(client: &str) -> Vec<Ty> {
    params_of(
        &[
            ("app/services/client.rb", client),
            ("app/services/caller.rb", "class Caller\n  def run\n    Client.new.get(\"x\", { a: 1 })\n  end\nend\n"),
        ],
        "Client",
        "get",
    )
}

/// The caller's `{ a: 1 }` joined with the `{}` default: no `{ query: … }` arm.
fn caller_options_hash() -> Ty {
    let unknown = || Ty::Var { var: roundhouse::ident::TyVar(0) };
    Ty::Hash {
        key: Box::new(Ty::Union { variants: vec![Ty::Sym, unknown()] }),
        value: Box::new(Ty::Union { variants: vec![Ty::Int, unknown()] }),
    }
}

/// HTTParty gives the class a `get(url, options)`; `self.class.get` inside
/// the instance `get` is that call, not a recursive one. Read as the
/// instance method's own site, `params` took `{ query: params }` and
/// wrapped itself one level deeper every round (chatwoot's
/// `Crm::Leadsquared::Api::BaseClient`).
#[test]
fn a_class_side_call_does_not_feed_the_instance_method_of_the_same_name() {
    let params = params_of_get(
        "class Client\n  include HTTParty\n\n  def get(path, params = {})\n    options = { query: params }\n    self.class.get(path, options)\n  end\nend\n",
    );
    assert_eq!(params[1], caller_options_hash());
}

/// Same shape when the class also defines `def self.get`: the shared
/// params key must still not take `self.class.get`'s args as instance
/// evidence.
#[test]
fn both_sides_class_call_still_does_not_feed_the_instance_method() {
    let params = params_of_get(
        "class Client\n  def self.get(url, options = {})\n    [url, options]\n  end\n\n  def get(path, params = {})\n    options = { query: params }\n    self.class.get(path, options)\n  end\nend\n",
    );
    assert_eq!(params[1], caller_options_hash());
}

/// A mailer's class-side call is how its instance method runs. Under
/// `Devise::Mailer` (mastodon's `UserMailer`) no class-side forwarder is
/// registered, so the call is the instance method's only evidence.
#[test]
fn a_mailer_class_side_call_still_types_the_instance_method() {
    let params = params_of(
        &[
            ("app/mailers/user_mailer.rb", "class UserMailer < Devise::Mailer\n  def welcome(name)\n    @name = name\n  end\nend\n"),
            ("app/services/greeter.rb", "class Greeter\n  def run\n    UserMailer.welcome(\"Ann\")\n  end\nend\n"),
        ],
        "UserMailer",
        "welcome",
    );
    assert_eq!(params, vec![Ty::Str]);
}

/// An `extend self` module is called on itself; that is its methods' evidence.
#[test]
fn an_extend_self_module_call_still_types_the_method() {
    let params = params_of(
        &[
            ("app/lib/global_path.rb", "module GlobalPath\n  def cdn_path(p)\n    p\n  end\n\n  extend self\nend\n"),
            ("app/services/linker.rb", "class Linker\n  def run\n    GlobalPath.cdn_path(\"/a\")\n  end\nend\n"),
        ],
        "GlobalPath",
        "cdn_path",
    );
    assert_eq!(params, vec![Ty::Str]);
}
