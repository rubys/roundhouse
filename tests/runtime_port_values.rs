//! PostgreSQL: runtime port values, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::process::Command;

const METADATA_ACCESSORS: &str = r#"module MetadataAccessors
  FALSE_VALUES = ActiveModel::Type::Boolean::FALSE_VALUES

  def self.falsy?(value)
    FALSE_VALUES.include?(value)
  end
end
"#;

const THREAD_LOCAL_TTL: &str = r#"module ActiveSupport
  module Cache
    module ThreadLocalTtl
      EXPLICIT_EXPIRATION_KEYS = [
        :expires_in,
        :expires_at,
        *ActiveSupport::Cache::OPTION_ALIASES.fetch(:expires_in),
      ].freeze
    end
  end
end
"#;

const PROFILING_METADATA: &str = r#"module ProfilingMetadata
  def self.metadata
    { "rails_version" => Rails::VERSION::STRING }
  end
end
"#;

/// Run after the three files load, natively and emitted.
const OBSERVE: &str = r#"p MetadataAccessors::FALSE_VALUES.sort_by(&:inspect), MetadataAccessors::FALSE_VALUES.class, MetadataAccessors::FALSE_VALUES.frozen?
p ["off", "on", 0, 1, false, nil, :F, "FALSE"].map { |v| MetadataAccessors.falsy?(v) }
p ActiveSupport::Cache::ThreadLocalTtl::EXPLICIT_EXPIRATION_KEYS
p ActiveSupport::Cache::OPTION_ALIASES, ActiveSupport::Cache::OPTION_ALIASES.frozen?, ActiveSupport::Cache::OPTION_ALIASES[:expires_in].frozen?
p ProfilingMetadata.metadata, Rails::VERSION::STRING.frozen?
"#;

const EXPECTED: &str = r#"["0", "F", "FALSE", "OFF", "f", "false", "off", 0, :"0", :F, :FALSE, :OFF, :f, :false, :off, false]
Set
true
[true, false, true, false, true, false, true, true]
"#;

fn app(postgresql: bool) -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("lib/metadata_accessors.rb", METADATA_ACCESSORS)
        .write("lib/active_support/cache/thread_local_ttl.rb", THREAD_LOCAL_TTL)
        .write("lib/profiling_metadata.rb", PROFILING_METADATA);
    if postgresql {
        app = app
            .write("roundhouse.yml", "test_paths:\n  - test\n")
            .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_runtime_port_values\n");
    } else {
        app = app.write("roundhouse.yml", "test_paths:\n  - test\n");
    }
    app
}

fn emitted_file<'a>(dir: &std::path::Path, files: &'a [String], leaf: &str) -> &'a String {
    files.iter().find(|f| f.ends_with(leaf)).unwrap_or_else(|| panic!("no emitted {leaf} in {}: {files:?}", dir.display()))
}

fn native() -> String {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_runtime_port_values_native_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in [
        ("metadata_accessors.rb", METADATA_ACCESSORS),
        ("thread_local_ttl.rb", THREAD_LOCAL_TTL),
        ("profiling_metadata.rb", PROFILING_METADATA),
    ] {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activesupport\", \"activemodel\", \"railties\")\n\
         require \"active_support\"\nrequire \"active_support/cache\"\nrequire \"active_model\"\nrequire \"active_model/type\"\n\
         require \"rails/gem_version\"\n\
         require_relative \"metadata_accessors\"\nrequire_relative \"thread_local_ttl\"\nrequire_relative \"profiling_metadata\"\n\
         {OBSERVE}"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .arg("native.rb")
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

#[test]
#[ignore = "fork spec: postgresql"]
fn port_values_load_on_the_postgresql_tree_and_answer_like_rails() {
    let (dir, errors) = app(true).emit(BuildTarget::Ruby);
    // `check` names the PostgreSQL adapter as having no runtime of its
    // own in target-independent analysis; the CRuby tree carries one.
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    for port in ["runtime/active_model_type.rb", "runtime/active_support_cache.rb", "runtime/rails_railtie.rb"] {
        assert!(dir.join(port).exists(), "{port}");
    }
    let mut files = Vec::new();
    let mut emitted_files = Vec::new();
    for leaf in ["metadata_accessors.rb", "thread_local_ttl.rb", "profiling_metadata.rb"] {
        let found: Vec<String> = walk(&dir).into_iter().filter(|f| f.ends_with(leaf) && !f.starts_with("runtime/")).collect();
        emitted_files.extend(found);
    }
    for leaf in ["metadata_accessors.rb", "thread_local_ttl.rb", "profiling_metadata.rb"] {
        let file = emitted_file(&dir, &emitted_files, leaf).clone();
        let text = std::fs::read_to_string(dir.join(&file)).unwrap();
        assert!(!text.contains("roundhouse:"), "{file}: {text}");
        files.push(file);
    }

    // The emitted files load beside the three ports, as the booted tree
    // loads them.
    let script = format!(
        "require_relative 'runtime/active_support_cache'\nrequire_relative 'runtime/active_model_type'\nrequire_relative 'runtime/rails_railtie'\n{}\n{OBSERVE}",
        files.iter().map(|f| format!("require_relative {:?}", f.trim_end_matches(".rb"))).collect::<Vec<_>>().join("\n")
    );
    let out = emit_and_run::ruby().current_dir(&*dir).args(["-W:no-deprecated", "-e", &script]).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.starts_with(EXPECTED), "{stdout}");
    assert!(stdout.contains("[:expires_in, :expires_at, :expire_in, :expired_in]"), "{stdout}");
    assert!(stdout.contains("{\"rails_version\" => \"8.2.0.alpha\"}\ntrue\n"), "{stdout}");
    assert_eq!(stdout, native(), "emitted on the ports vs native Rails");
}

/// The SQLite CRuby tree loads the activemodel and activesupport ports
/// (runtime/gem_facades.rb) and runs those reads, but only carries
/// rails_railtie.rb with the rest of runtime/spinel/: nothing requires it,
/// so `Rails::VERSION` is a NameError there and the read is refused. The
/// Spinel tree loads none of the three. Each refusal names the file.
#[test]
#[ignore = "fork spec: postgresql"]
fn trees_that_do_not_load_a_port_refuse_its_value_reads() {
    let (dir, errors) = app(false).emit(BuildTarget::Ruby);
    assert!(dir.join("runtime/rails_railtie.rb").exists(), "the SQLite tree carries the file");
    assert!(
        errors.iter().any(|e| e.contains(
            "Rails::VERSION::STRING is assigned by the Ruby-family runtime (runtime/rails_railtie.rb), which ruby carries but does not load"
        )),
        "{errors:#?}"
    );
    for name in ["FALSE_VALUES", "OPTION_ALIASES"] {
        assert!(!errors.iter().any(|e| e.contains(name)), "{name}: {errors:#?}");
    }
    let script = format!(
        "require File.expand_path('main', Dir.pwd)\nrequire_relative 'app/models/metadata_accessors'\n\
         p MetadataAccessors.falsy?('off'), ActiveSupport::Cache::ThreadLocalTtl::EXPLICIT_EXPIRATION_KEYS\n\
         p defined?(Rails::VERSION)\n"
    );
    let out = emit_and_run::ruby().current_dir(&*dir).args(["-W0", "-e", &script]).env("BLOG_DB", ":memory:").output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "true\n[:expires_in, :expires_at, :expire_in, :expired_in]\nnil\n");

    let (dir, errors) = app(false).emit(BuildTarget::Spinel);
    for (name, file) in [
        ("ActiveModel::Type::Boolean::FALSE_VALUES", "runtime/active_model_type.rb"),
        ("ActiveSupport::Cache::OPTION_ALIASES", "runtime/active_support_cache.rb"),
        ("Rails::VERSION::STRING", "runtime/rails_railtie.rb"),
    ] {
        let state = if dir.join(file).exists() { "carries but does not load" } else { "does not emit" };
        let expected = format!("{name} is assigned by the Ruby-family runtime ({file}), which spinel {state}");
        assert!(errors.iter().any(|e| e.contains(&expected)), "spinel {name}: {errors:#?}");
    }
}

#[test]
#[ignore = "fork spec: postgresql"]
fn uri_gid_is_the_globalid_ports_constant() {
    const READER: &str = "module GidReader\n  def self.model_name(gid)\n    URI::GID.parse(gid).model_name\n  rescue URI::GID::MissingModelIdError\n    nil\n  end\nend\n";
    let app = |postgresql: bool| {
        let app = emit_and_run::empty_app().write("lib/gid_reader.rb", READER);
        if postgresql {
            app.write("roundhouse.yml", "test_paths:\n  - test\n")
                .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_runtime_port_values\n")
        } else {
            app.write("roundhouse.yml", "test_paths:\n  - test\n")
        }
    };
    let (dir, errors) = app(true).emit(BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let reader = std::fs::read_to_string(dir.join("app/models/gid_reader.rb")).expect("gid_reader.rb");
    assert!(!reader.contains("roundhouse:"), "{reader}");
    assert!(reader.contains("URI::GID.parse(gid).model_name") && reader.contains("rescue URI::GID::MissingModelIdError"), "{reader}");
    // The port needs ActiveSupport's `blank?`, which boot loads first.
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-W0",
            "-e",
            "require_relative 'runtime/active_support_core_ext'\nrequire_relative 'runtime/global_id'\nrequire_relative 'app/models/gid_reader'\n\
             p GidReader.model_name('gid://app/User/1'), GidReader.model_name('gid://app/User')\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "\"User\"\nnil\n");

    // Spinel takes no PostgreSQL app; the SQLite app's Spinel tree.
    let (_dir, errors) = app(false).emit(BuildTarget::Spinel);
    assert!(
        errors.iter().any(|e| e.contains("URI::GID is declared by the globalid gem, which the spinel tree does not load")),
        "{errors:#?}"
    );
    assert!(!errors.iter().any(|e| e.contains("namespace ownership") || e.contains("uri gem")), "{errors:#?}");
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    fn go(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                go(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    go(dir, dir, &mut out);
    out.sort();
    out
}
