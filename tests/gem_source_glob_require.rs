//! A gem's glob require is the requires of its files, in directory order.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/globgem-1.2.0\n  specs:\n    globgem (1.2.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  globgem!\n";
const LIB: &str = "vendor/gems/globgem-1.2.0/lib";

/// The gem's lib files, relative to `lib/`.
fn gem_files() -> Vec<(String, String)> {
    let mut files = vec![
        (
            "globgem.rb".to_string(),
            "require_relative \"globgem/version\"\n\n$globgem_log = []\nDir[File.join(__dir__, \"globgem/**/*.rb\")].each { |file| require_relative file }\n\nmodule Globgem\n  def self.loaded = $globgem_log\nend\n".to_string(),
        ),
        ("globgem/version.rb".to_string(), "module Globgem\n  VERSION = \"1.2.0\"\nend\n".to_string()),
        // buyer-risk-ml's keys.rb requires a Rails feature the runtime
        // ports answer; the gem is still compiled, not refused.
        (
            "globgem/keys.rb".to_string(),
            "require \"active_support/core_ext/object/blank\"\n\nmodule Globgem\n  module Keys\n    def self.key(value)\n      value.blank? ? \"none\" : value.to_s\n    end\n  end\nend\n\n$globgem_log << \"keys\"\n".to_string(),
        ),
        ("globgem/data.json".to_string(), "{}\n".to_string()),
    ];
    for path in ["a", "a/y", "ab", "b", "b/z"] {
        let mark: String = path.replace('/', "").to_uppercase();
        files.push((
            format!("globgem/{path}.rb"),
            format!("module Globgem\n  module Mark{mark}\n  end\nend\n\n$globgem_log << \"{path}\"\n"),
        ));
    }
    files
}

/// The log native Ruby writes: the logging files in `Dir[]`'s order.
fn native_order() -> Vec<String> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let lib = std::env::temp_dir().join(format!(
        "rh_gem_source_glob_require_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    for (path, content) in gem_files() {
        let full = lib.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
    let out = emit_and_run::ruby()
        .args(["-e", "puts Dir[File.join(ARGV[0], 'globgem/**/*.rb')].map { |f| f.delete_prefix(ARGV[0] + '/globgem/').delete_suffix('.rb') }"])
        .arg(&lib)
        .output()
        .expect("ruby");
    let _ = std::fs::remove_dir_all(&lib);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| *l != "version").map(str::to_string).collect()
}

fn app() -> emit_and_run::Overlay {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"globgem\", \"1.2.0\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write(
            "app/models/glob_reader.rb",
            "module GlobReader\n  LOADED = Globgem.loaded.dup.freeze\n  KEYS = [Globgem::Keys.key(nil), Globgem::Keys.key(\"x\")].freeze\n  MARKS = [Globgem::MarkAY, Globgem::MarkBZ].map(&:name).freeze\nend\n",
        );
    for (path, content) in gem_files() {
        app = app.write(&format!("{LIB}/{path}"), &content);
    }
    app
}

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_sources_glob_require_is_the_requires_of_its_files_in_dir_order() {
    let native = native_order();
    assert_eq!(native, ["a/y", "a", "ab", "b/z", "b", "keys"], "Ruby's Dir[] order");
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("gem_requires_rails")), "{errors:#?}");
    let entry = std::fs::read_to_string(dir.join(format!("app/models/roundhouse_files/{LIB}/globgem.rb"))).expect("entry program");
    assert!(!entry.contains("Dir["), "{entry}");
    let required: Vec<&str> = entry
        .lines()
        .filter_map(|l| l.strip_prefix(&format!("RoundhouseAutoload.require_gem_file(\"models/roundhouse_files/{LIB}/globgem/")))
        .map(|l| l.trim_end_matches("\")"))
        .collect();
    assert_eq!(required, native, "{entry}");

    let script = "module StrictW\n  def warn(msg, category: nil, **kw)\n    raise \"StrictWarning: #{msg}\" if msg.include?(\"already initialized constant\")\n    super\n  end\nend\n\
         Warning.singleton_class.prepend(StrictW)\n\
         require_relative 'boot'\n\
         p Globgem.loaded\n\
         p GlobReader::LOADED\n\
         p GlobReader::KEYS\n\
         p GlobReader::MARKS\n\
         p require(File.expand_path('app/models/glob_reader.rb'))\n\
         p require('globgem')\n";
    let out = emit_and_run::ruby().current_dir(&*dir).args(["-e", script]).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let log = format!("[{}]", native.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>().join(", "));
    assert_eq!(
        stdout,
        format!("{log}\n{log}\n[\"none\", \"x\"]\n[\"Globgem::MarkAY\", \"Globgem::MarkBZ\"]\nfalse\nfalse\n"),
        "{stderr}"
    );
    assert!(!stderr.contains("file_statement_not_run") && !stderr.contains("file_require_not_carried"), "{stderr}");
}
