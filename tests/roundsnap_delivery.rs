//! Exercise the CLI delivery path in subprocesses, including real emitted
//! model/controller tests. No unsafe process-global environment mutation.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch_dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("rh-roundsnap-{name}-{}", std::process::id()));
    if p.exists() {
        std::fs::remove_dir_all(&p).unwrap();
    }
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn emit(fixture: &Path, out: &Path, flag: &str, keep: bool, target: &str) {
    let result = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target])
        .arg(fixture)
        .arg("-o")
        .arg(out)
        .current_dir(std::env::temp_dir())
        .env_remove("ROUNDSNAP")
        .env_remove("ROUNDHOUSE_RUBY_ISEQ")
        .env_remove("ROUNDSNAP_KEEP_SOURCE")
        .env_remove("ROUNDHOUSE_ISEQ_KEEP_SOURCE")
        .env(flag, "1")
        .env("ROUNDSNAP_KEEP_SOURCE", if keep { "1" } else { "0" })
        .output()
        .expect("spawn compiler");
    assert!(
        result.status.success(),
        "emit failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn ruby(out: &Path, args: &[&str]) {
    let result = Command::new("ruby")
        .args(args)
        .current_dir(out)
        .env_remove("BUNDLE_GEMFILE")
        .env_remove("RUBYOPT")
        .output()
        .expect("spawn MRI");
    assert!(
        result.status.success(),
        "MRI failed\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&result.stdout));
}

#[test]
fn tiny_blog_roundsnap_artifact_shape_and_target_isolation() {
    let fixture = std::fs::canonicalize("fixtures/tiny-blog").unwrap();
    let out = scratch_dir("tiny");
    // The legacy alias works, and re-emitting a source tree removes only
    // units actually compiled. User/config/seeds/tool files survive.
    emit(&fixture, &out, "UNUSED_ROUNDSNAP_FLAG", false, "ruby");
    assert!(out.join("app/models.rb").is_file());
    let main = std::fs::read(out.join("main.rb")).unwrap();
    std::fs::write(out.join("user.rb"), "USER_FILE = true\n").unwrap();
    emit(&fixture, &out, "ROUNDHOUSE_RUBY_ISEQ", false, "ruby");
    assert!(out.join("manifest.json").is_file());
    assert!(out.join("vendor/roundsnap/LICENSE").is_file());
    assert!(!out.join("app/models.rb").exists());
    assert!(!out.join("units.json").exists());
    assert!(out.join("user.rb").is_file());
    assert_eq!(main, std::fs::read(out.join("main.rb")).unwrap());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["units"].get("boot").is_none());
    assert!(
        manifest["units"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v["mapped"] == true)
    );
    // tiny-blog is a shape fixture without ApplicationRecord, not a boot
    // conformance fixture. Actual boot and execution live below on real-blog.

    let jruby = scratch_dir("jruby");
    // A reused output may contain a failed prior ISeq input. Neither JRuby
    // with the flag nor plain CRuby may compile or delete that stale input.
    std::fs::write(jruby.join("units.json"), "invalid stale input").unwrap();
    emit(&fixture, &jruby, "ROUNDSNAP", false, "jruby");
    assert!(!jruby.join("manifest.json").exists());
    assert!(!jruby.join("vendor/roundsnap").exists());
    assert!(jruby.join("app/models.rb").is_file());
    emit(&fixture, &jruby, "UNUSED_ROUNDSNAP_FLAG", false, "ruby");
    assert!(!jruby.join("manifest.json").exists());
    assert_eq!(
        "invalid stale input",
        std::fs::read_to_string(jruby.join("units.json")).unwrap()
    );
    std::fs::remove_dir_all(out).unwrap();
    std::fs::remove_dir_all(jruby).unwrap();
}

#[test]
fn real_blog_roundsnap_compiles_and_runs_emitted_tests() {
    let fixture = std::fs::canonicalize(roundhouse::fixtures::real_blog()).unwrap();
    let out = scratch_dir("real-blog");
    emit(&fixture, &out, "ROUNDSNAP", false, "ruby");
    assert!(!out.join("app/models/article.rb").exists());
    assert!(!out.join("runtime/active_record/relation.rb").exists());
    assert!(
        std::fs::read_to_string(out.join("main.rb"))
            .unwrap()
            .contains("require \"stringio\"")
    );
    ruby(
        &out,
        &[
            "-e",
            "require_relative 'boot'; require_relative 'runtime/active_record/relation'; raise unless defined?(Set); puts 'REAL_BLOG_ROUNDSNAP_BOOT_OK'",
        ],
    );
    // Same app suites as the normal Ruby toolchain gate, not a boot-only
    // smoke. Run files separately: MRI's other positional args are ARGV.
    for test in [
        "test/models/article_test.rb",
        "test/models/comment_test.rb",
        "test/controllers/articles_controller_test.rb",
        "test/controllers/comments_controller_test.rb",
    ] {
        ruby(&out, &["-Itest", "-I.", test]);
    }
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn failed_finalize_preserves_last_compiled_generation() {
    let out = scratch_dir("failed-finalize");
    let fixture = std::fs::canonicalize(roundhouse::fixtures::real_blog()).unwrap();
    emit(&fixture, &out, "ROUNDSNAP", false, "ruby");
    let before = std::fs::read(out.join("manifest.json")).unwrap();
    std::fs::write(
        out.join("units.json"),
        r#"[{"key":"bad","source":"def broken(","file":"bad.rb"}]"#,
    )
    .unwrap();
    assert!(roundhouse::project::finalize_roundsnap(&out).is_err());
    assert_eq!(before, std::fs::read(out.join("manifest.json")).unwrap());
    ruby(
        &out,
        &[
            "-e",
            "require_relative 'boot'; puts 'OLD_GENERATION_BOOT_OK'",
        ],
    );
    std::fs::remove_dir_all(out).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        std::fs::copy(from, to).unwrap();
    }
}

#[test]
fn real_blog_model_and_erb_map_to_their_source_lines() {
    let scratch = scratch_dir("mapping");
    let source = scratch.join("real-blog");
    let fixture = roundhouse::fixtures::real_blog();
    // Only ingest inputs; no Rails tmp/storage/bundle trees.
    for dir in ["app", "config", "db"] {
        copy_tree(&fixture.join(dir), &source.join(dir));
    }
    std::fs::copy(fixture.join("Gemfile"), source.join("Gemfile")).unwrap();
    let article_path = source.join("app/models/article.rb");
    let article = std::fs::read_to_string(&article_path).unwrap();
    std::fs::write(
        &article_path,
        article.replacen(
            "class Article < ApplicationRecord\n",
            r#"class Article < ApplicationRecord
  def roundsnap_probe
    raise 'model-probe'
  end
  def csv_probe
    CSV.generate(headers: ["id", "title"], write_headers: true) do |csv|
      csv << [1, "x"]
      csv << [2, "y,z"]
    end
  end
"#,
            1,
        ),
    )
    .unwrap();
    std::fs::write(
        source.join("app/views/articles/roundsnap_probe.html.erb"),
        "<p>probe</p>\n<%= Article.new.roundsnap_probe %>\n",
    )
    .unwrap();
    let out = scratch.join("emit");
    emit(&source, &out, "ROUNDSNAP", true, "ruby");
    assert!(
        std::fs::read_to_string(out.join("Gemfile"))
            .unwrap()
            .contains("gem \"csv\"")
    );
    // Keeping sources is a supported debugging mode. Compiler input digest
    // must match the actual file; source markers have already been finished.
    ruby(
        &out,
        &[
            "-rjson",
            "-rdigest",
            "-e",
            r#"
require_relative 'boot'
raise 'CSV boot require lost' unless Article.new.csv_probe == "id,title\n1,x\n2,\"y,z\"\n"
loader = Roundsnap::Loader.current
loader.manifest.fetch('units').each do |key, entry|
  source = File.binread(key + '.rb')
  raise 'compiled source differs' unless Digest::SHA256.hexdigest(source) == entry.fetch('digest')
  source.each_line do |line|
    raise 'unfinished marker' if line.lstrip.start_with?('#<SPINEL_SOURCE>') && !line.start_with?('#<SPINEL_SOURCE>')
  end
end
[[-> { Article.new.roundsnap_probe }, 'model-probe', 'real-blog/app/models/article.rb:3:'],
 [-> { Views::Articles.roundsnap_probe }, 'model-probe', 'real-blog/app/views/articles/roundsnap_probe.html.erb:2:']].each do |call, message, location|
  begin
    call.call
  rescue RuntimeError => error
    raise unless error.message == message
    original = error.backtrace.dup
    mapped = loader.format_backtrace(original)
    raise "wrong source location: #{mapped.join('\n')}" unless mapped.any? { |frame| frame.start_with?(location) }
    raise 'native frame changed' unless error.backtrace == original
    next
  end
  raise 'probe did not raise'
end
puts 'REAL_SOURCE_MODEL_AND_ERB_OK'
"#,
        ],
    );
    std::fs::remove_dir_all(scratch).unwrap();
}
