//! `save!` answers the RECORD, not a bool.
//!
//! The runtime is unambiguous: `runtime/ruby/active_record/base.rb`
//! raises `RecordInvalid` on failure and returns `self`, its sidecar
//! declares `save!: () -> Base`, and the method catalog types it
//! `ReturnKind::SelfType`. The per-model `ClassInfo` seeded by the model
//! lowerer disagreed and said `Bool`.
//!
//! What that costs: every method whose tail is `record.save!` carried a
//! `-> bool` signature its own body contradicts. Nothing under CRuby
//! notices — lobsters' `Domain#ban_by_user_for_reason!`,
//! `#unban_by_user_for_reason!` and `SavedStory.save_story_for_user` all
//! end in a `save!` and all ran fine — but the emitted RBS is a false
//! statement about the program, and once spinel began judging return
//! seeds (matz/spinel#4005) it refused the AOT build outright.

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_models_with_registry_and_params;
use roundhouse::ty::Ty;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :domains do |t|\n    t.string :domain\n    t.datetime :banned_at\n  end\nend\n",
        ),
        ("app/models/domain.rb", "class Domain < ApplicationRecord\nend\n"),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn the_per_model_registry_types_save_bang_as_the_model() {
    let app = app();
    let (_lcs, registry) = lower_models_with_registry_and_params(
        &app.models,
        &app.schema,
        vec![],
        &Default::default(),
    );
    let info = registry
        .get(&ClassId(Symbol::from("Domain")))
        .expect("Domain seeded");

    let ret = |name: &str| match info.instance_methods.get(&Symbol::from(name)) {
        Some(Ty::Fn { ret, .. }) => (**ret).clone(),
        other => panic!("`{name}` is not a function type: {other:?}"),
    };

    // `save` is the boolean half of the pair and stays boolean.
    assert_eq!(ret("save"), Ty::Bool, "`save` answers whether it saved");
    assert_eq!(
        ret("save!"),
        Ty::Class { id: ClassId(Symbol::from("Domain")), args: vec![] },
        "`save!` answers the record — it raises rather than returning false",
    );
}
