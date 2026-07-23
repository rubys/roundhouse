//! Route-helper path params are typed PER PARAM: `<x>_id` names model
//! `<x>` directly, so a nested route under a slug parent (`story_id`
//! in `/stories/:story_id/suggestions`, Story#to_param → short_id)
//! types that param `Str` even though the owning route's own resource
//! is id-shaped. Before this, the helper's strict-target signature
//! said Int and every call site passing `story.short_id` was a C type
//! error (8+ sites on the lobsters spinel lane, 2026-07-23).

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_routes_to_library_functions;
use roundhouse::ty::Ty;

#[test]
fn nested_param_under_slug_parent_types_str() {
    let files = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :stories do |t|\n    t.string :short_id\n  end\n  create_table :suggestions do |t|\n    t.integer :story_id\n  end\nend\n",
        ),
        (
            "app/models/story.rb",
            "class Story < ApplicationRecord\n  def to_param\n    short_id\n  end\nend\n",
        ),
        (
            "app/models/suggestion.rb",
            "class Suggestion < ApplicationRecord\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :stories do\n    resources :suggestions\n  end\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let helpers = lower_routes_to_library_functions(&app);

    let sig_params = |name: &str| -> Vec<roundhouse::ty::Param> {
        let sig = helpers
            .iter()
            .find(|f| f.name.as_str() == name)
            .unwrap_or_else(|| panic!("helper {name} not generated"))
            .signature
            .clone()
            .expect("helper signature");
        let Ty::Fn { params, .. } = sig else {
            panic!("helper signature is not Ty::Fn: {sig:?}")
        };
        params
    };

    // Nested collection helper: its `story_id` param must be Str (the
    // parent's to_param is a slug), not Int.
    let params = sig_params("story_suggestions_path");
    assert_eq!(params[0].name.as_str(), "story_id");
    assert!(
        matches!(params[0].ty, Ty::Str),
        "story_id under a slug parent must type Str, got {:?}",
        params[0].ty
    );

    // The suggestion's own member id keeps Int (Suggestion has no
    // to_param override).
    let params = sig_params("story_suggestion_path");
    let id_ty = params
        .iter()
        .find(|p| p.name.as_str() == "id")
        .map(|p| p.ty.clone())
        .expect("id param");
    assert!(
        matches!(id_ty, Ty::Int),
        "suggestion's own id stays Int, got {id_ty:?}"
    );
}
