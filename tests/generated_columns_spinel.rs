//! Native Spinel counterpart to the generated-column SQLite contract.
//! The full DDL, callback, cache, and reload behavior runs in the compiled
//! application, not only through diagnostics or string assertions.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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
#[ignore = "requires the Spinel toolchain"]
fn native_spinel_models_read_and_persist_generated_columns() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        include_str!("support/generated_columns_contract.rb")
    );
    let run = app().run_spinel(&script);
    run.assert_passes();
    assert!(
        run.stdout
            .contains("generated column create/update/reload contract passed")
    );
}
