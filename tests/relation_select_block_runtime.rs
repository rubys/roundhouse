//! PostgreSQL: relation select block runtime, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_block_select_on_a_relation_filters_its_records_on_cruby() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_relation_select_block\n")
        .emit(BuildTarget::Ruby);
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            "require_relative 'runtime/active_record'\n\
             require_relative 'runtime/active_record_relation_ext'\n\
             r = ActiveRecord::Relation.allocate\n\
             r.instance_variable_set(:@records, [1, 2, 3, 4])\n\
             p r.select { |x| x.even? }\n\
             begin\n\
               r.select(:id) { |x| x }\n\
             rescue ArgumentError => e\n\
               puts e.message\n\
             end\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "[2, 4]\n'select' with block doesn't take arguments.\n", "{stderr}");
}
