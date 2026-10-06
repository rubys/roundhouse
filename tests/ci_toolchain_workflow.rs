use std::fs;

/// Inspect expanded steps, including YAML aliases: every MRI setup and
/// prepared-oracle key must follow the workflow selector. JRuby is separate.
#[test]
fn mri_jobs_and_oracle_caches_use_the_central_ruby_line() {
    let source = fs::read_to_string(".github/workflows/ci.yml").unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source).unwrap();
    let minimum = fs::read_to_string(".ruby-version").unwrap();
    assert_eq!(workflow["env"]["MRI_RUBY"].as_str(), Some(minimum.trim()));
    let (mut mri, mut jruby, mut oracles) = (0, 0, 0);
    for (name, job) in workflow["jobs"].as_mapping().unwrap() {
        assert!(job["env"].get("MRI_RUBY").is_none(), "{name:?} shadows MRI");
        for step in job["steps"].as_sequence().unwrap() {
            assert!(
                step["env"].get("MRI_RUBY").is_none(),
                "{name:?} shadows MRI"
            );
            let uses = step["uses"].as_str().unwrap_or("");
            if uses.starts_with("ruby/setup-ruby@") {
                match step["with"]["ruby-version"].as_str() {
                    Some("${{ env.MRI_RUBY }}") => mri += 1,
                    Some("jruby-10.0") => jruby += 1,
                    version => panic!("{name:?} bypasses the MRI selector: {version:?}"),
                }
            }
            let key = step["with"]["key"].as_str().unwrap_or("");
            if key.starts_with("campfire-oracle-") {
                oracles += 1;
                assert!(key.contains("ruby${{ env.MRI_RUBY }}"), "{name:?}: {key}");
            }
        }
    }
    assert!(mri > 0 && jruby > 0 && oracles > 0);
}

/// Active Node work uses one current major. A leftover Node 20 pin
/// recompiles `better-sqlite3` (no ABI 115 prebuild) and a mixed
/// `setup-node` major splits the cache and install contract.
#[test]
fn active_node_jobs_pin_node_24_with_setup_node_v7() {
    let source = fs::read_to_string(".github/workflows/ci.yml").unwrap();
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source).unwrap();
    let mut expanded = 0;
    for (name, job) in workflow["jobs"].as_mapping().unwrap() {
        for step in job["steps"].as_sequence().unwrap() {
            let uses = step["uses"].as_str().unwrap_or("");
            if !uses.starts_with("actions/setup-node@") {
                continue;
            }
            expanded += 1;
            assert_eq!(
                uses, "actions/setup-node@v7",
                "{name:?} must use the current setup-node major"
            );
            assert_eq!(
                step["with"]["node-version"].as_str(),
                Some("24"),
                "{name:?} must install Node 24, not an older ABI"
            );
        }
    }
    assert!(
        expanded > 0,
        "must inspect actual Node setup steps, including expanded YAML anchors"
    );
    let files = roundhouse::emit::typescript::emit(&roundhouse::App::new());
    let package = files
        .iter()
        .find(|file| file.path == std::path::Path::new("package.json"))
        .expect("typescript emit writes package.json");
    let package: serde_json::Value = serde_json::from_str(&package.content).unwrap();
    assert_eq!(
        package["devDependencies"]["@types/node"].as_str(),
        Some("^24"),
        "emitted Node types must follow the runtime pin"
    );
}

#[test]
fn uv_cache_keys_use_the_generated_dependency_source_before_emit() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for job in ["compare", "compare-extra", "smoke"] {
        let steps = workflow["jobs"][job]["steps"].as_sequence().unwrap();
        let uv = steps
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("astral-sh/setup-uv@")
            })
            .expect("Python lanes install uv before generating a project");
        let glob = uv["with"]["cache-dependency-glob"].as_str().unwrap();
        assert_eq!(glob, "src/emit/python/pyproject.rs", "{job}");
        assert!(std::path::Path::new(glob).is_file(), "{job}: {glob}");
        assert!(uv["with"].get("enable-cache").is_none());
        assert!(uv["with"].get("ignore-nothing-to-cache").is_none());
    }
}

/// Swift compare-extra is the hosted outlier (~7–8 billable minutes). The
/// job still installs Swift 6.1 and runs the same framework-tests +
/// `scripts/compare swift` check; it must cache SwiftPM checkouts so the
/// Hummingbird/NIO clone is not paid on every run. Smoke uses the same
/// cache key because its README `swift build` fetches the same pins.
#[test]
fn swift_compare_and_smoke_cache_swiftpm_checkouts() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    let mut keys = Vec::new();
    for job in ["compare", "compare-extra", "smoke"] {
        let steps = workflow["jobs"][job]["steps"].as_sequence().unwrap();
        let setup = steps
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("swift-actions/setup-swift@")
            })
            .unwrap_or_else(|| panic!("{job} installs Swift"));
        assert_eq!(
            setup["with"]["swift-version"].as_str(),
            Some("6.1"),
            "{job}"
        );
        let cache = steps
            .iter()
            .find(|step| step["name"].as_str() == Some("Cache SwiftPM checkouts"))
            .unwrap_or_else(|| panic!("{job} caches SwiftPM"));
        assert_eq!(cache["if"].as_str(), Some("matrix.target == 'swift'"), "{job}");
        assert_eq!(
            cache["uses"].as_str(),
            Some("actions/cache@v6"),
            "{job}"
        );
        let path = cache["with"]["path"].as_str().unwrap();
        assert!(path.contains("~/.cache/org.swift.swiftpm"), "{job}: {path}");
        let key = cache["with"]["key"].as_str().unwrap();
        assert!(
            key.contains("hashFiles('src/emit/swift/package.rs')"),
            "{job}: {key}"
        );
        keys.push(key.to_string());
    }
    assert!(keys.windows(2).all(|pair| pair[0] == pair[1]));
}

/// Release WMO of the emitted app was ~150s of compare-extra (swift).
/// Debug `swift build` is the same boot-and-DOM-diff check (and matches
/// the published README / `swift_toolchain` compile gate).
#[test]
fn compare_script_builds_swift_debug_not_release() {
    let script = fs::read_to_string("scripts/compare").unwrap();
    assert!(
        script.contains("swift build -Xswiftc -gnone"),
        "compare must use a debug SPM build for the Swift server"
    );
    assert!(
        !script.contains("--disable-index-store"),
        "Linux `swift test`/`swift build` fatalError if the index store path is missing"
    );
    assert!(
        !script.contains("swift build -c release"),
        "release codegen is for scripts/bench, not the DOM-diff job"
    );
    assert!(
        script.contains("./.build/debug/App"),
        "{script}"
    );
    assert!(
        !script.contains("./.build/release/App"),
        "compare must boot the debug binary it just built"
    );
    assert!(
        script.contains("tools/compare/target/release/roundhouse-compare"),
        "non-Swift compare jobs keep the previous release comparator"
    );
}

#[test]
fn compare_rust_cache_does_not_add_extra_directories() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&fs::read_to_string(".github/workflows/ci.yml").unwrap()).unwrap();
    for job in ["compare", "compare-extra"] {
        let rust_cache = workflow["jobs"][job]["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Swatinem/rust-cache@")
            })
            .unwrap_or_else(|| panic!("{job} rust-cache"));
        assert!(
            rust_cache.get("with").is_none(),
            "{job} rust-cache must stay the default workspace cache; extra directories change the key for every compare target"
        );
    }
}
