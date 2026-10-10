//! The Spinel side of two emit changes that Ruby and Spinel share: a
//! `**options` call argument the shared keyword lowering made
//! positional is written back as a splat (`ERASED_KEYWORD_SPLAT`), and
//! `__dir__` / `__FILE__` anchor on the emitted file
//! (`SOURCE_FILE_PATH`, written as `File.expand_path(rel, __dir__)`).
//! The program must compile and answer the same as the CRuby runs in
//! `tests/ruby_keyword_splat_passthrough.rs` and `tests/source_file_path.rs`.
//! Needs a Spinel compiler, so `#[ignore]`d like its sibling suites;
//! CI's `spinel-framework` job runs it with `--ignored`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LIBRARY: &str = r##"class KeywordConnection
  def query(sql, limit:, offset: 0)
    "#{sql} #{limit} #{offset}"
  end

  def relay(options)
    query("relay", **options)
  end

  def relay_dynamic(meth, *args, **kwargs)
    send(meth, *args, **kwargs)
  end

  def anchored
    File.expand_path("x", __dir__).end_with?("/x")
  end
end
"##;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_keyword_splat_and_a_dir_anchor_compile_and_run_on_spinel() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/lib/keyword_connection.rb", LIBRARY)
        .run_spinel(
            "c = KeywordConnection.new\nputs c.relay({ limit: 3 })\nputs c.relay_dynamic(:query, \"dyn\", limit: 4)\nputs c.anchored\n",
        );
    run.assert_passes();
    assert_eq!(run.stdout, "relay 3 0\ndyn 4 0\ntrue\n");
}

/// Runs without a compiler: the Spinel tree carries the same two
/// rewrites the Ruby tree does, so a Spinel regression shows up here
/// even where the toolchain is absent.
#[test]
fn the_spinel_tree_writes_the_splat_and_the_dir_anchor() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write("app/lib/keyword_connection.rb", LIBRARY)
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let text = std::fs::read_to_string(emitted.join("app/models/keyword_connection.rb")).expect("emitted file");
    assert!(text.contains("send(meth, *args, **kwargs)"), "{text}");
    assert!(text.contains("File.expand_path(\"x\", File.expand_path(\"../../app/lib\", __dir__))"), "{text}");
}
