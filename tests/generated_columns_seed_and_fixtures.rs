//! Generated-column values are excluded from static seed INSERTs, and
//! hand-written YAML fixture values are rejected at their source location.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::path::Path;
use std::{collections::HashMap, path::PathBuf};

use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            include_str!("support/generated_columns_schema.rb"),
        )
        .write(
            "app/models/person.rb",
            include_str!("support/generated_columns_person.rb"),
        )
        .write(
            "app/models/virtual_person.rb",
            include_str!("support/generated_columns_virtual_person.rb"),
        )
        .write(
            "app/models/constant_person.rb",
            include_str!("support/generated_columns_constant_person.rb"),
        )
}

#[test]
fn static_seed_insert_keeps_source_values_and_omits_explicit_generated_key() {
    let (emitted, app, errors) = app()
        .write(
            "db/seeds.rb",
            r#"Person.create!(first_name: "Seed", last_name: "Row", display_name: "forged-seed")
"#,
        )
        .emit_with_app(BuildTarget::Ruby);
    assert!(
        errors.is_empty(),
        "seed should be supported without errors: {errors:?}"
    );

    let seed = std::fs::read_to_string(emitted.join("db/seed.sql")).expect("generated db/seed.sql");
    let insert = seed
        .split(';')
        .find(|statement| statement.contains("INSERT INTO") && statement.contains("people"))
        .unwrap_or_else(|| panic!("seed insert missing: {seed}"));
    let columns = insert.split("VALUES").next().unwrap_or(insert);
    assert!(
        columns.contains("first_name") && columns.contains("last_name"),
        "{insert}"
    );
    assert!(
        !columns.contains("display_name"),
        "generated value must not be writable:\n{insert}"
    );
    assert!(
        insert.contains("Seed") && insert.contains("Row"),
        "source values were lost:\n{insert}"
    );
    assert!(
        !insert.contains("forged-seed"),
        "an explicit generated value must not leak into the seed INSERT:\n{insert}"
    );
    drop(app);
}

#[test]
fn yaml_fixture_generated_value_is_a_located_unsupported_boundary() {
    let mut tree = HashMap::new();
    for (path, source) in [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "db/schema.rb",
            include_str!("support/generated_columns_schema.rb"),
        ),
        (
            "app/models/person.rb",
            include_str!("support/generated_columns_person.rb"),
        ),
        (
            "app/models/virtual_person.rb",
            include_str!("support/generated_columns_virtual_person.rb"),
        ),
        (
            "app/models/constant_person.rb",
            include_str!("support/generated_columns_constant_person.rb"),
        ),
        (
            "test/fixtures/people.yml",
            "one:\n  first_name: Ada\n  last_name: Lovelace\n  display_name: forged-fixture\n",
        ),
    ] {
        tree.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app =
        roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest minimal fixture app");
    roundhouse::session::analyze_and_lower(&mut app);

    let error = roundhouse::project::target_files(&app, Path::new("."), BuildTarget::Ruby)
        .expect_err("fixture target generation must reject a generated field");
    assert!(
        error.contains("test/fixtures/people.yml"),
        "fixture source path: {error}"
    );
    assert!(error.contains("record `one`"), "fixture record: {error}");
    assert!(
        error.contains("people.display_name"),
        "generated column: {error}"
    );
}
