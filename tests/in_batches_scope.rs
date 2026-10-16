//! A scope whose body is `in_batches` answers the model's relation.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ident::ClassId;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

#[test]
fn an_in_batches_scope_types_as_the_relation() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\" do |t|\n    t.string \"title\"\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/post.rb"), b"class Post < ApplicationRecord\n  scope :batched, -> { in_batches }\nend\n".to_vec());
    tree.insert(PathBuf::from("app/services/probe.rb"), b"class Probe\n  def run\n    Post.batched\n  end\nend\n".to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let probe = app.library_classes.iter().find(|c| c.name.0.as_str() == "Probe").expect("class");
    let def = probe.methods.iter().find(|m| m.name.as_str() == "run").expect("method");
    let Some(Ty::Fn { ret, .. }) = &def.signature else { panic!("typed: {:?}", def.signature) };
    assert_eq!(**ret, Ty::Relation { of: ClassId(roundhouse::ident::Symbol::from("Post")) });
}
