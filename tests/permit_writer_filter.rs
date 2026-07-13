//! Permit-writer filter (`model_to_library::writable_permit_fields`) —
//! a controller's permit list is wider than the model's writer surface,
//! and the synthesized `update`/`from_params` must only assign fields a
//! writer actually backs. Lobsters permits lookup keys (`tag[tag_name]`,
//! `category[category_name]`) with no writer; assigning them made the
//! emitted model undefined-method-error under spinel AOT (and would
//! NoMethodError under CRuby had a form ever submitted the key).
//!
//! The writable set unions: table columns, `belongs_to` writers,
//! `attr_accessor`/`attr_writer` virtuals, `typed_store` attrs,
//! `has_secure_password`'s plaintext pair, and user-defined
//! `def <field>=`. Everything else drops (with a `lower_residue`
//! warning at emit time).

use roundhouse::ident::Symbol;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_models_with_registry_and_params;

fn lobsters_like_app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :tags do |t|\n    t.string :tag\n    t.string :description\n    t.integer :category_id\n  end\nend\n",
        ),
        (
            "app/models/tag.rb",
            r#"class Tag < ApplicationRecord
  belongs_to :category
  attr_accessor :some_virtual

  def category_name=(category)
    @category_id = 1
  end
end
"#,
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest tree")
}

#[test]
fn update_and_from_params_skip_writerless_permitted_fields() {
    let app = lobsters_like_app();
    // The permit spec a TagsController would declare: `tag_name` is a
    // lookup key with NO writer; every other name has one (column,
    // user-defined `def category_name=`, attr_accessor virtual).
    let params_specs: std::collections::BTreeMap<Symbol, Vec<Symbol>> = [(
        Symbol::from("tag"),
        vec![
            Symbol::from("tag_name"),
            Symbol::from("category_name"),
            Symbol::from("some_virtual"),
            Symbol::from("tag"),
            Symbol::from("description"),
        ],
    )]
    .into_iter()
    .collect();

    let (lcs, _registry) =
        lower_models_with_registry_and_params(&app.models, &app.schema, vec![], &params_specs);
    let tag = lcs.iter().find(|lc| lc.name.0.as_str() == "Tag").expect("Tag lowered");

    let body_of = |name: &str| {
        let m = tag
            .methods
            .iter()
            .find(|m| m.name.as_str() == name)
            .unwrap_or_else(|| panic!("`{name}` not synthesized"));
        format!("{:?}", m.body)
    };

    for method in ["update", "from_params"] {
        let body = body_of(method);
        assert!(
            !body.contains("\"tag_name\""),
            "`{method}` must skip the writerless permitted field `tag_name`: {body}"
        );
        for kept in ["\"category_name\"", "\"some_virtual\"", "\"description\""] {
            assert!(
                body.contains(kept),
                "`{method}` must keep the writable permitted field {kept}: {body}"
            );
        }
    }
}
