//! A targeted smoke consumes the same bytes as full publication, without
//! requiring website assets, WASM or unselected archives.

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn selected_cli_archives_match_full_site_bytes_and_clear_stale_targets() {
    roundhouse::stack::run(|| {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("selected-archives-{}-{unique}", std::process::id()));
        let selected = root.join("selected");
        let full = root.join("full");
        fs::create_dir_all(selected.join("browse")).unwrap();
        fs::write(selected.join("browse/go.tgz"), "stale, unselected bytes").unwrap();
        fs::write(selected.join("keep.txt"), "unrelated output").unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["--archives", "ruby,rust"])
            .arg(roundhouse::fixtures::real_blog())
            .arg("-o")
            .arg(&selected)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut names: Vec<_> = fs::read_dir(selected.join("browse"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "ruby.json",
                "ruby.tgz",
                "ruby.zip",
                "rust.json",
                "rust.tgz",
                "rust.zip"
            ]
        );
        assert_eq!(
            fs::read_to_string(selected.join("keep.txt")).unwrap(),
            "unrelated output"
        );
        assert!(!selected.join("index.html").exists());
        assert!(!selected.join("lib").exists());

        roundhouse::project::build_site(roundhouse::fixtures::real_blog(), &full).unwrap();
        assert!(full.join("index.html").is_file());
        assert!(full.join("browse/index.html").is_file());
        assert!(full.join("browse/app.js").is_file());
        for name in names {
            assert_eq!(
                fs::read(selected.join("browse").join(&name)).unwrap(),
                fs::read(full.join("browse").join(&name)).unwrap(),
                "archive drift: {name}"
            );
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(selected.join("browse/rust.json")).unwrap()).unwrap();
        let files = manifest["files"].as_array().unwrap();
        for required in ["README.md", "e2e/package.json", "e2e/playwright.config.js"] {
            assert!(
                files.iter().any(|f| f["path"].as_str() == Some(required)),
                "missing archive contract: {required}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn archive_cli_rejects_ambiguous_modes_and_invalid_lists() {
    for args in [
        vec!["--archives"],
        vec!["--archives", ""],
        vec!["--archives", "rust,unknown"],
        vec!["--archives", "roda"],
        vec!["--archives", "rust", "--site"],
        vec!["--archives", "rust", "--target", "go"],
        vec!["--archives", "rust", "--allow-unsupported"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(&args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("roundhouse:"));
    }
}
