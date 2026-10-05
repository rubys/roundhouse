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
