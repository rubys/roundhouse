//! Spinel toolchain integration test — compiles the emitted real-blog
//! tests via the spinel AOT compiler and runs the resulting native
//! binaries. Mirrors `ruby_toolchain.rs`: same emit, same 4 test
//! suites, swapped runner.
//!
//! Two differences from the Ruby toolchain test:
//!   1. `runtime/db.rb` is the FFI-backed shim (`runtime/spinel/db.rb`,
//!      module Db over libsqlite3) rather than the gem-backed sibling.
//!   2. The runner is the scaffold Makefile's `spinel-test` target,
//!      which compiles each `test/<dir>/<stem>.rb` via `$(SPINEL)` and
//!      executes the resulting binary. `$(SPINEL)` defaults to `spinel`
//!      on PATH — set the `SPINEL` env var to override.
//!
//! Marked `#[ignore]` — CI-only. Invoke:
//!
//!     cargo test --test spinel_toolchain -- --ignored --nocapture
//!
//! Prerequisites for local runs: `spinel` on PATH (or `SPINEL=...`),
//! and `libsqlite3.so` discoverable at link time (`libsqlite3-dev` on
//! Debian/Ubuntu; macOS ships it).
//!
//! Suites validated: same 4 as ruby_toolchain — article + comment
//! model tests, articles + comments controller tests. Wider coverage
//! (article_broadcasts, views suite) tracked in
//! `project_lowered_ir_gaps_for_runnability`.

use std::path::{Path, PathBuf};
use std::process::Command;


use roundhouse::ingest::ingest_app;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/time_change_usec.rs"]
mod time_change_usec_contract;
#[path = "spinel_toolchain/time_change_usec.rs"]
mod time_change_usec;
#[path = "support/string_truncate.rs"]
mod string_truncate;
#[path = "spinel_toolchain/string_truncate.rs"]
mod string_truncate_native;
#[path = "support/array_excluding.rs"]
mod array_excluding;
#[path = "spinel_toolchain/array_excluding.rs"]
mod array_excluding_native;
#[path = "spinel_toolchain/lobsters_zero.rs"]
mod lobsters_zero;
#[path = "support/class_attribute.rs"]
mod class_attribute;
#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/dry_initializer.rs"]
mod dry_initializer;
#[path = "support/data_factory.rs"]
mod data_factory;
#[path = "support/relation_fork.rs"]
mod relation_fork_contract;
#[path = "spinel_toolchain/relation_fork.rs"]
mod relation_fork;
#[path = "support/dry_struct.rs"]
mod dry_struct;
#[path = "support/dry_validation.rs"]
mod dry_validation;
#[path = "support/dry_monads.rs"]
mod dry_monads;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;
#[path = "support/cable_allowed_origins.rs"]
mod cable_allowed_origins_contract;
#[path = "spinel_toolchain/cable_allowed_origins.rs"]
mod cable_allowed_origins;
#[path = "support/public_file_server_headers.rs"]
mod public_file_server_headers_contract;
#[path = "spinel_toolchain/public_file_server_headers.rs"]
mod public_file_server_headers;
#[path = "support/cable_actions.rs"]
mod cable_actions_contract;
#[path = "spinel_toolchain/cable_actions.rs"]
mod cable_actions;
#[path = "support/cable_broadcast_json.rs"]
mod cable_broadcast_json_contract;
#[path = "support/cable_transmit.rs"]
mod cable_transmit_contract;
#[path = "spinel_toolchain/cable_transmit.rs"]
mod cable_transmit;
#[path = "spinel_toolchain/cable_broadcast_json.rs"]
mod cable_broadcast_json;
#[path = "support/index_with.rs"]
mod index_with_contract;
#[path = "spinel_toolchain/index_with.rs"]
mod index_with;
#[path = "support/send_data_filename.rs"]
mod send_data_filename_contract;
#[path = "spinel_toolchain/send_data_filename.rs"]
mod send_data_filename;
#[path = "support/vary_accept.rs"]
mod vary_accept_contract;
#[path = "spinel_toolchain/vary_accept.rs"]
mod vary_accept;
#[path = "support/erb_template.rs"]
mod erb_template_contract;
#[path = "support/bare_permit.rs"]
mod bare_permit_contract;
#[path = "support/data_paths.rs"]
mod data_paths_contract;
#[path = "support/eager_load_filter.rs"]
mod eager_load_filter_contract;
#[path = "spinel_toolchain/erb_template.rs"]
mod erb_template;
#[path = "support/anonymous_keywords.rs"]
mod anonymous_keywords;
#[path = "support/lambda_signatures.rs"]
mod lambda_signatures_contract;
#[path = "spinel_toolchain/lambda_signatures.rs"]
mod lambda_signatures;
#[path = "support/native_http.rs"]
mod native_http;
#[path = "spinel_toolchain/action_controller_head.rs"]
mod action_controller_head;
#[path = "support/conditional_get.rs"]
mod conditional_get_contract;
#[path = "spinel_toolchain/conditional_get.rs"]
mod conditional_get;
#[path = "spinel_toolchain/response_headers.rs"]
mod response_headers;
#[path = "spinel_toolchain/action_dispatch_request_put.rs"]
mod action_dispatch_request_put;
#[path = "spinel_toolchain/strong_params.rs"]
mod strong_params;
#[path = "spinel_toolchain/params_wrapper.rs"]
mod params_wrapper;
#[path = "spinel_toolchain/json_request_values.rs"]
mod json_request_values;
#[path = "spinel_toolchain/multipart_file_parts.rs"]
mod multipart_file_parts;
#[path = "support/default_format_dispatch.rs"]
mod default_format_dispatch;
#[path = "spinel_toolchain/route_default_format.rs"]
mod route_default_format;
#[path = "support/json_params.rs"]
mod json_params_contract;
#[path = "spinel_toolchain/json_params.rs"]
mod json_params;
#[path = "support/io_process_constants.rs"]
mod io_process_constants_contract;
#[path = "spinel_toolchain/io_process_constants.rs"]
mod io_process_constants;
#[path = "support/backticks.rs"]
mod backticks_contract;
#[path = "spinel_toolchain/backticks.rs"]
mod backticks;
#[path = "support/class_side_default.rs"]
mod class_side_default_contract;
#[path = "spinel_toolchain/class_side_default.rs"]
mod class_side_default;
#[path = "spinel_toolchain/action_text_sanitize.rs"]
mod action_text_sanitize;
#[path = "support/campfire_caches.rs"]
mod campfire_caches_contract;
#[path = "spinel_toolchain/campfire_caches.rs"]
mod campfire_caches;
#[path = "spinel_toolchain/ordinalize.rs"]
mod ordinalize;
#[path = "spinel_toolchain/params_helper_no_permit.rs"]
mod params_helper_no_permit;
#[path = "spinel_toolchain/rails_parity.rs"]
mod rails_parity;
#[path = "spinel_toolchain/api_parity_round3.rs"]
mod api_parity_round3;
#[path = "spinel_toolchain/api_parity_round4.rs"]
mod api_parity_round4;
#[path = "spinel_toolchain/server_binding.rs"]
mod server_binding;
#[path = "spinel_toolchain/unix_listener.rs"]
mod unix_listener;
#[path = "support/request_id_runtime.rs"]
mod request_id_runtime_contract;
#[path = "spinel_toolchain/request_id_runtime.rs"]
mod request_id_runtime;

/// Compile and execute the shared custom Data factory contract with Spinel;
/// successful Ruby emission alone does not establish native target support.
#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn custom_data_factories_run_natively() {
    let run = emit_and_run::real_blog()
        .write("app/services/factory_examples.rb", data_factory::CUSTOM_DECLARATIONS)
        .run_spinel(data_factory::CUSTOM_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("custom Data factory contract passed"));
}
#[path = "support/stdlib_rnp.rs"]
mod stdlib_rnp_contract;
#[path = "spinel_toolchain/stdlib_rnp.rs"]
mod stdlib_rnp;
#[path = "support/representable_decorator.rs"]
mod representable_decorator;
#[path = "spinel_toolchain/representable.rs"]
mod representable;
#[path = "support/for_loops.rs"]
mod for_loops_contract;
#[path = "spinel_toolchain/for_loops.rs"]
mod for_loops;
#[path = "support/block_macros.rs"]
mod block_macros_contract;
#[path = "spinel_toolchain/block_macros.rs"]
mod block_macros;
#[path = "support/stdlib_scan_zlib.rs"]
mod stdlib_scan_zlib_contract;
#[path = "spinel_toolchain/stdlib_scan_zlib.rs"]
mod stdlib_scan_zlib;
#[path = "support/stdlib_rexml_include.rs"]
mod stdlib_rexml_include_contract;
#[path = "spinel_toolchain/stdlib_rexml_include.rs"]
mod stdlib_rexml_include;
#[path = "support/mutable_literals.rs"]
mod mutable_literals_contract;
#[path = "spinel_toolchain/mutable_literals.rs"]
mod mutable_literals;
#[path = "support/auto_source_gem.rs"]
mod auto_source_gem_contract;
#[path = "spinel_toolchain/auto_source_gem.rs"]
mod auto_source_gem;
#[path = "support/active_model_serializer.rs"]
mod active_model_serializer_app;
#[path = "spinel_toolchain/active_model_serializer.rs"]
mod active_model_serializer;

/// The native counterpart of the generic emitted-Ruby regression test.
#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn anonymous_keyword_forwarding_runs_natively() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/keyword_forwarder.rb",
            anonymous_keywords::SOURCE,
        )
        .run_spinel(anonymous_keywords::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("anonymous keyword forwarding contract passed"));
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/keyword_forwarder.rb"))
        .expect("emitted keyword forwarding class");
    assert!(emitted.contains("class KeywordForwarder"), "{emitted}");
    assert!(emitted.contains("request(kind: :get, path: path, **)"), "{emitted}");
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn active_support_many_block_and_squish_bang_run_natively() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/active_support_probe.rb",
            r#"class ActiveSupportProbe
  def self.many_matches
    seen = []
    zero = [1, 2, 3].many? { |n| n > 9 }
    one = [1, 2, 3].many? { |n| n == 2 }
    multiple = [1, 2, 3, 4].many? { |n| seen << n; n > 1 }
    [zero, one, multiple, seen.length]
  end

  def self.squish_bang
    text = "\u00a0foo\u2003bar\u2028".dup
    text.squish!
  end
end
"#,
        )
        .run_spinel(
            r#"raise "many? block zero/one/multiple or short circuit failed" unless ActiveSupportProbe.many_matches == [false, false, true, 3]
raise "squish! did not match Unicode whitespace semantics" unless ActiveSupportProbe.squish_bang == "foo bar"
puts "ActiveSupport core extensions native passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("ActiveSupport core extensions native passed"));
}

#[path = "support/engine_mount.rs"]
mod engine_mount;

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn literal_isolated_engine_mount_dispatches_on_native_http() {
    engine_mount::spinel_http_witness();
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn finite_concern_class_configuration_runs_natively() {
    for (overlay, assertions) in [
        (class_configuration::overlay(), class_configuration::ASSERTIONS),
        (class_configuration::empty_overlay(), class_configuration::EMPTY_ASSERTIONS),
    ] {
        let run = overlay.run_spinel(assertions);
        run.assert_passes();
        assert!(run.stdout.contains("finite class configuration contract passed"));
    }
}

/// The native half of `emit_and_run::dry_initializer_classes_run_lowered`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn dry_initializer_classes_run_lowered_natively() {
    let run = dry_initializer::overlay().run_spinel(dry_initializer::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("dry-initializer contract passed"));
}

/// The native half of `emit_and_run::dry_struct_classes_run_lowered`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn dry_struct_classes_run_lowered_natively() {
    let run = dry_struct::overlay()
        .run_spinel(&format!("{}\n{}", dry_struct::ASSERTIONS, dry_struct::STAMP_ASSERTIONS));
    run.assert_passes();
    assert!(run.stdout.contains("dry-struct contract passed"));
    assert!(run.stdout.contains("dry-struct stamp contract passed"));
}

/// The native half of `emit_and_run::concern_class_attribute_macros_run_at_class_load`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn concern_class_attribute_macros_run_at_class_load_natively() {
    let run = class_attribute::overlay().run_spinel(class_attribute::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute contract passed"));
}

/// The native half of `emit_and_run::concern_class_attribute_set_to_nil_is_not_unset`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn concern_class_attribute_set_to_nil_is_not_unset_natively() {
    let run = class_attribute::nil_overlay().run_spinel(class_attribute::NIL_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute nil contract passed"));
}

/// The native half of `emit_and_run::rails_root_join_takes_any_number_of_parts`:
/// the fixtures never call `join` with more than one part, so no other
/// Spinel lane compiles the variadic `Rails::AppPath#join`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn rails_root_join_takes_any_number_of_parts_natively() {
    let run = rails_root_join::overlay().run_spinel(rails_root_join::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root.join contract passed"));
}

/// The native half of `rails_health_check::the_rails_health_check_answers_up`:
/// `/up` routes to the synthesized `Rails::HealthController`, which
/// compiles and answers the green page. `main.rb` boots the server
/// under AOT, so its `instantiate_controller` arm is checked as text.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn the_rails_health_check_answers_up_natively() {
    let run = emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\n",
        )
        .run_spinel(r#"
routed = false
RouteTable.table.each do |route|
  routed = true if route.verb == "GET" && route.pattern == "/up" && route.controller == :rails__health && route.action == :show
end
raise "no GET /up route to rails_health#show" unless routed
c = Rails::HealthController.new
c.process_action(:show)
raise "status #{c.status}" unless c.status == 200
raise "body #{c.body}" unless c.body == "<!DOCTYPE html><html><body style=\"background-color: green\"></body></html>"
puts "rails health contract passed"
"#);
    run.assert_passes();
    assert!(run.stdout.contains("rails health contract passed"));
    let main = std::fs::read_to_string(run.emitted.join("main.rb")).expect("main.rb");
    assert!(main.contains("when :rails__health then Rails::HealthController.new"), "{main}");
}

fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-spinel-{tag}"))
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).expect("mkdir");
        for entry in std::fs::read_dir(src).expect("readdir") {
            let entry = entry.expect("entry");
            copy_tree(&entry.path(), &dst.join(entry.file_name()));
        }
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::copy(src, dst).expect("copy file");
    }
}

/// Build the scratch project: the REAL spinel base file set, exactly as
/// `project::spinel_files` assembles it before `spin_shape`.
///
/// This used to hand-copy an enumerated list of runtime files on top of
/// the scaffold. Its own comment called that "a FOURTH registration
/// point for a new runtime file" and predicted the failure mode — "a
/// miss shows up as `cannot load such file` from spinel rather than
/// from anything the unit tests reach" — which is exactly how it broke
/// once `test/test_helper.rb` started requiring `main.rb` (whose chain
/// is complete). Deleted in favour of the set that ships.
fn generate_project(fixture: &Path, scratch: &Path) {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch).expect("clean scratch");
    }
    std::fs::create_dir_all(scratch).expect("create scratch");

    let mut app = ingest_app(fixture).expect("ingest");
    // The shipped tree is built from the post-analyze lowered IR, as `roundhouse --target spinel` builds it
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::project::spinel_base_files(&app, fixture).expect("spinel base files");
    roundhouse::project::write_to_dir(&files, scratch).expect("write spinel tree");

    // The framework runtime's OWN tests (broadcasts/cgi_io + the
    // integration/views/models/tools subdirs) are a harness concern, not
    // something an app archive ships — overlay them so this job keeps
    // covering them alongside the app's emitted suite.
    copy_tree(Path::new("runtime/spinel/test"), &scratch.join("test"));

    // …but not that tree's `test_helper.rb`: the shipped tree already
    // carries the per-app rendered one (`render_test_helper`), and the
    // source copy is the blog-shaped stand-in it exists to replace.
    for (path, content) in &files {
        if path == "test/test_helper.rb" {
            std::fs::write(scratch.join("test/test_helper.rb"), content)
                .expect("restore rendered test_helper");
        }
    }
}

#[test]
#[ignore]
fn real_blog_spinel_tests_pass() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

    let output = Command::new("make")
        .arg("spinel-test")
        .current_dir(&scratch)
        .output()
        .expect("spawn make spinel-test");

    assert!(
        output.status.success(),
        "make spinel-test failed\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Exercise the shipped CLI/spin package, not the pre-spin_shape test overlay.
#[test]
#[ignore = "requires spin and spinel on PATH plus SQLite/jemalloc development libraries"]
fn identical_cli_emission_reuses_native_build_but_changed_ruby_rebuilds() {
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn snapshot(root: &Path, dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(snapshot(root, &path));
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (
                        fs::read(&path).unwrap(),
                        fs::metadata(&path).unwrap().modified().unwrap(),
                    ),
                );
            }
        }
        files
    }

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = scratch_dir(&format!("repeat-{}-{unique}", std::process::id()));
    let fixture = scratch.join("source");
    let dest = scratch.join("output");
    copy_tree(roundhouse::fixtures::real_blog(), &fixture);
    fs::write(fixture.join("public/freshness.bin"), [0xff, 0x00, 0x80]).unwrap();
    let emit = || {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--target", "spinel"])
            .arg(&fixture)
            .arg("-o")
            .arg(&dest)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "strict emit failed: {stderr}");
        stderr
    };
    let build = || {
        let output = Command::new("spin")
            .args(["build", "blog"])
            .current_dir(&dest)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "spin build failed:\n{stdout}\n{stderr}"
        );
        eprintln!("{stdout}{stderr}");
        (stdout, stderr)
    };

    let first_emit = emit();
    let generated = snapshot(&dest, &dest);
    assert!(generated.contains_key(Path::new("spin.toml")));
    assert!(
        generated
            .keys()
            .any(|p| p.extension().is_some_and(|e| e == "rbs"))
    );
    assert_eq!(
        generated[Path::new("public/freshness.bin")].0,
        [0xff, 0x00, 0x80]
    );
    assert!(
        first_emit.contains(&format!("emitted {} files", generated.len())),
        "{first_emit}"
    );
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog\n"), "{stdout}");
    assert!(
        stderr.contains("bin/blog.rb -> "),
        "compiler invocation missing: {stderr}"
    );
    let executable = dest.join("build/bin/blog");
    let built_at = fs::metadata(&executable).unwrap().modified().unwrap();

    // Spinel compares integer seconds with a strict >. Waiting makes the
    // old unconditional writer reliably invalidate the already built binary.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(emit(), first_emit);
    let repeated = snapshot(&dest, &dest);
    for (path, expected) in &generated {
        assert_eq!(&repeated[path], expected, "{}", path.display());
    }
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog (up to date)"), "{stdout}");
    assert!(
        !stderr.contains("bin/blog.rb -> "),
        "unexpected compiler invocation: {stderr}"
    );
    assert_eq!(
        fs::metadata(&executable).unwrap().modified().unwrap(),
        built_at
    );

    std::thread::sleep(Duration::from_secs(2));
    let source = fixture.join("app/controllers/articles_controller.rb");
    let before = fs::read_to_string(&source).unwrap();
    let after = before.replace(
        "Article was successfully created.",
        "Article was successfully changed.",
    );
    assert_ne!(
        before, after,
        "fixture no longer contains the mutation literal"
    );
    fs::write(source, after).unwrap();
    emit();
    let ruby = Path::new("app/controllers/articles_controller.rb");
    let changed = fs::read(dest.join(ruby)).unwrap();
    assert_ne!(
        changed, generated[ruby].0,
        "source change must reach generated Ruby"
    );
    assert!(String::from_utf8_lossy(&changed).contains("Article was successfully changed."));
    assert_eq!(
        fs::metadata(dest.join("spin.toml"))
            .unwrap()
            .modified()
            .unwrap(),
        generated[Path::new("spin.toml")].1
    );
    let (stdout, stderr) = build();
    assert!(stdout.contains("build blog\n"), "{stdout}");
    assert!(
        stderr.contains("bin/blog.rb -> "),
        "changed Ruby did not invoke compiler: {stderr}"
    );
    assert!(fs::metadata(&executable).unwrap().modified().unwrap() > built_at);
    fs::remove_dir_all(scratch).unwrap();
}

/// The native half of `emit_and_run::dry_validation_contracts_run_lowered`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn dry_validation_contracts_run_lowered_natively() {
    let run = dry_validation::overlay().run_spinel(&dry_validation::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("dry-validation contract passed"));
}

/// The native half of `emit_and_run::dry_monads_results_run_lowered`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn dry_monads_results_run_lowered_natively() {
    let run = dry_monads::overlay().run_spinel(&dry_monads::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("dry-monads contract passed"));
}

/// The native half of `emit_and_run::oj_under_mimic_json_runs_as_json`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn oj_under_mimic_json_runs_as_json_natively() {
    let run = emit_and_run::real_blog()
        .write("config/initializers/oj.rb", "# frozen_string_literal: true\n\nOj.mimic_JSON\nOj.optimize_rails\n")
        .write("lib/payload.rb", r##"class Payload
  def self.generated(value)
    Oj.generate(value)
  end

  def self.generated_or(value)
    Oj.generate(value)
  rescue JSON::GeneratorError
    :not_finite
  end

  def self.decode(text)
    Oj.load(text)
  end

  def self.decode_symbols(text)
    Oj.load(text, symbol_keys: true)
  end

  def self.guarded(text)
    Oj.load(text)
  rescue Oj::ParseError
    :oj_rescued
  rescue JSON::ParserError
    :json_rescued
  end

  def self.guarded_symbols(text)
    Oj.load(text, symbol_keys: true)
  rescue JSON::ParserError
    :bad
  end
end
"##)
        .run_spinel(r##"raise "generate" unless Payload.generated({a: :b, h: "<&>", n: [1.5, nil, true]}) == %({"a":"b","h":"<&>","n":[1.5,null,true]})
raise "generate infinity" unless Payload.generated_or({x: Float::INFINITY}) == :not_finite
raise "generate nan" unless Payload.generated_or({x: Float::NAN}) == :not_finite
raise "load" unless Payload.decode(%({"a":[1,2.5,null,true],"b":{"c":"d"}})) == {"a" => [1, 2.5, nil, true], "b" => {"c" => "d"}}
raise "load nil" unless Payload.decode(nil).nil?
raise "load integer" unless Payload.decode(5) == 5
raise "load symbols" unless Payload.decode_symbols(%({"a":{"b":1}})) == {a: {b: 1}}
raise "parse error" unless Payload.guarded("{x") == :json_rescued
raise "load array" unless Payload.decode([1]) == [1]
raise "load array nil" unless Payload.guarded([nil]) == :json_rescued
raise "load array symbol" unless Payload.guarded([:a]) == :json_rescued
raise "load array hash" unless Payload.guarded([{"a" => 1}]) == :json_rescued
raise "load array symbols" unless Payload.decode_symbols(%([{"a":1}])) == [{a: 1}]
raise "load bad array symbols" unless Payload.guarded_symbols([{"a" => 1}]) == :bad
raise "load rational" unless Payload.guarded(Rational(1, 2)) == :json_rescued
raise "load infinity" unless Payload.decode(Float::INFINITY).infinite?
raise "load nan" unless Payload.decode(Float::NAN).nan?
raise "load hash" unless Payload.guarded({"a" => 1}) == :json_rescued
raise "load symbol" unless Payload.guarded(:x) == :json_rescued
puts "oj passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("oj passed"));
}

/// The native half of `emit_and_run::named_capture_matches_assign_their_locals`.
#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn named_capture_matches_assign_their_locals_natively() {
    let run = emit_and_run::real_blog()
        .write("lib/matcher.rb", r##"class Matcher
  def self.map_name(html)
    /usemap=\s*['"]#(?<map_name>\S+?)['"]/m =~ html
    map_name
  end

  def self.pair(text)
    if /(?<a>\d+)-(?<b>\d+)/ =~ text
      [a, b]
    else
      [a, b, :none]
    end
  end

  def self.index(text)
    /(?<w>x)/ =~ text
  end
end
"##)
        .run_spinel(r##"raise "map" unless Matcher.map_name(%(<img usemap="#m1">)) == "m1"
raise "map none" unless Matcher.map_name("none").nil?
raise "pair" unless Matcher.pair("1-2") == ["1", "2"]
raise "pair none" unless Matcher.pair("x") == [nil, nil, :none]
raise "index" unless Matcher.index("abx") == 2
raise "index none" unless Matcher.index("q").nil?
puts "named captures passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("named captures passed"));
}

#[path = "spinel_toolchain/db_binary_literal.rs"]
mod db_binary_literal;
