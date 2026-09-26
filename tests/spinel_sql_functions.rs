//! SQL functions an app registers in an initializer, on every tree.
//!
//! lobsters' `config/initializers/sqlite_functions.rb` patches `regexp`,
//! `if` and a `stddev` aggregate into the SQLite adapter. CRuby installs
//! them through the sqlite3 gem; spinel has no gem, so its tree gets an
//! FFI install (`project::spinel_sql_functions_file`) — trampolines per
//! function, the `fn` context, and a `SqlFunctions.install(dbh)` on
//! every pooled connection. JRuby has no install yet (ledgered in
//! docs/pipeline/runtime.md). With both functions registered,
//! FlaggedCommenters' façade stands aside and the real class serves.
//!
//! Emitted from the real-blog fixture with the initializer and the
//! FlaggedCommenters shape added — `target_files` reads a fixture.

use roundhouse::ingest::ingest_app;
use roundhouse::project::{target_files, BuildTarget};

const INITIALIZER: &str = r#"
ActiveSupport.on_load(:active_record) do
  ActiveRecord::ConnectionAdapters::SQLite3Adapter.class_eval do
    alias_method :orig_initialize, :initialize

    def initialize(connection, logger = nil, pool = nil)
      orig_initialize(connection, logger, pool)

      raw_connection.create_function("if", 3) do |fn, c, t, f|
        fn.result = (c == 1) ? t : f
      end

      raw_connection.create_aggregate("stddev", 1) do
        step do |fn, value|
          next if value.nil?

          fn[:n] ||= 0
          fn[:n] += 1
        end

        finalize do |fn|
          fn.result = fn[:n].nil? ? nil : fn[:n].to_f
        end
      end
    end
  end
end
"#;

fn files_for(target: BuildTarget, initializer: bool) -> Vec<(String, String)> {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let mut app = ingest_app(&fixture).expect("ingest real-blog");
    if initializer {
        let extra = roundhouse::ingest::ingest_app_from_tree(
            [(
                std::path::PathBuf::from("config/initializers/sqlite_functions.rb"),
                INITIALIZER.as_bytes().to_vec(),
            )]
            .into_iter()
            .collect(),
        )
        .expect("ingest initializer");
        app.sql_functions = extra.sql_functions;
    }
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, &fixture, target).expect("target files")
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> Option<&'a str> {
    files.iter().find(|(p, _)| p == path).map(|(_, c)| c.as_str())
}

#[test]
fn spinel_installs_them_through_ffi_on_every_connection() {
    let files = files_for(BuildTarget::Spinel, true);
    let src = file(&files, "runtime/sql_functions.rb").expect("spinel tree carries the install");
    assert!(src.contains("ffi_callback :rh_sqlfn"), "FFI registration:\n{src}");
    assert!(src.contains(r#"SqlFunctionsFFI.rh_sql_scalar(db, "if", 3, method(:rh_sqlfn_if))"#), "{src}");
    assert!(
        src.contains(r#"SqlFunctionsFFI.rh_sql_agg(db, "stddev", 1, method(:rh_sqlfn_stddev_step), method(:rh_sqlfn_stddev_final))"#),
        "{src}"
    );
    // The app's bodies as written: `fn[:n] ||= 0` / `fn[:n] += 1` on the
    // context class (spinel serves both since 606acc03, matz/spinel#5054).
    assert!(src.contains("fn[:n] ||= 0") && src.contains("fn[:n] += 1"), "{src}");
    let db = file(&files, "runtime/db.rb").expect("db.rb");
    assert!(db.starts_with("require_relative \"sql_functions\"\n"), "db.rb requires the install");
    assert!(db.contains("SqlFunctions.install(dbh)"), "installed per connection");
}

#[test]
fn cruby_uses_the_gem_and_jruby_has_none() {
    let ruby = files_for(BuildTarget::Ruby, true);
    let src = file(&ruby, "runtime/sql_functions.rb").expect("CRuby install");
    assert!(src.contains("db.create_function(\"if\", 3)"), "the sqlite3 gem's API:\n{src}");
    assert!(!src.contains("ffi_"), "no FFI on CRuby:\n{src}");
    let jruby = target_files_jruby();
    assert!(file(&jruby, "runtime/sql_functions.rb").is_none(), "JRuby installs none yet");
}

fn target_files_jruby() -> Vec<(String, String)> {
    files_for(BuildTarget::Jruby, true)
}

#[test]
fn an_app_without_them_is_untouched() {
    let files = files_for(BuildTarget::Spinel, false);
    assert!(file(&files, "runtime/sql_functions.rb").is_none());
    let db = file(&files, "runtime/db.rb").expect("db.rb");
    assert!(!db.contains("SqlFunctions"), "the blog's db.rb has no install");
}
