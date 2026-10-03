use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use roundhouse::app::App;
use roundhouse::ingest::{IngestError, ingest_app, ingest_app_from_tree};

fn unique_tmp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "roundhouse_{name}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn test_source(class_name: &str) -> String {
    format!(
        "class {class_name} < ActiveSupport::TestCase\n  test \"runs\" do\n    assert_equal 1, 1\n  end\nend\n"
    )
}

fn ruby_file(path: &str, class_name: &str) -> (PathBuf, String) {
    (PathBuf::from(path), test_source(class_name))
}

fn text_file(path: &str, text: &str) -> (PathBuf, String) {
    (PathBuf::from(path), text.to_string())
}

fn ingest_files(files: impl IntoIterator<Item = (PathBuf, String)>) -> Result<App, IngestError> {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, text)| (path, text.into_bytes()))
        .collect();
    ingest_app_from_tree(tree)
}

fn module_names(app: &App) -> Vec<String> {
    let mut names: Vec<_> = app
        .test_modules
        .iter()
        .map(|module| module.name.0.as_str().to_string())
        .collect();
    names.sort();
    names
}

fn assert_modules(app: &App, expected: &[&str]) {
    let actual = module_names(app);
    let mut expected: Vec<_> = expected.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(actual, expected);
}

fn default_and_custom_tests() -> Vec<(PathBuf, String)> {
    vec![
        ruby_file("test/models/model_default_test.rb", "ModelDefaultTest"),
        ruby_file(
            "test/controllers/controller_default_test.rb",
            "ControllerDefaultTest",
        ),
        ruby_file("test/helpers/helper_default_test.rb", "HelperDefaultTest"),
        ruby_file(
            "test/channels/channel_default_test.rb",
            "ChannelDefaultTest",
        ),
        ruby_file("test/lib/lib_default_test.rb", "LibDefaultTest"),
        ruby_file("test/unit/unit_custom_test.rb", "UnitCustomTest"),
        ruby_file("test/utils/utils_custom_test.rb", "UtilsCustomTest"),
    ]
}

const DEFAULT_MODULES: &[&str] = &[
    "ModelDefaultTest",
    "ControllerDefaultTest",
    "HelperDefaultTest",
    "ChannelDefaultTest",
    "LibDefaultTest",
];

const ALL_STANDARD_MODULES: &[&str] = &[
    "ModelDefaultTest",
    "ControllerDefaultTest",
    "HelperDefaultTest",
    "ChannelDefaultTest",
    "LibDefaultTest",
    "UnitCustomTest",
    "UtilsCustomTest",
    "QualitySpecsTest",
];

#[test]
fn rails_test_roots_include_test_lib_by_default() {
    let app = ingest_files(default_and_custom_tests()).expect("ingest tree");
    assert_modules(&app, DEFAULT_MODULES);
}

#[test]
fn configured_roots_add_app_relative_test_folders() {
    let mut files = default_and_custom_tests();
    files.push(ruby_file(
        "quality/specs/nested/probe.rb",
        "QualitySpecsTest",
    ));
    files.push(text_file(
        "roundhouse.yml",
        "test_paths:\n  - test/unit\n  - test/utils\n  - quality/specs\n",
    ));

    let app = ingest_files(files).expect("ingest tree");
    assert_modules(&app, ALL_STANDARD_MODULES);
}

#[test]
fn empty_configuration_keeps_defaults_and_adds_no_custom_roots() {
    for config in ["", "{}\n", "test_paths: []\n"] {
        let mut files = default_and_custom_tests();
        files.push(text_file("roundhouse.yml", config));
        let app = ingest_files(files).expect("ingest tree");
        assert_modules(&app, DEFAULT_MODULES);
    }
}

#[test]
fn overlapping_and_repeated_roots_do_not_duplicate_modules() {
    let nested_source = format!(
        "{}{}",
        test_source("NestedFirstTest"),
        test_source("NestedSecondTest")
    );
    let app = ingest_files([
        ruby_file("test/models/model_default_test.rb", "ModelDefaultTest"),
        (PathBuf::from("test/unit/nested/probe.rb"), nested_source),
        text_file(
            "roundhouse.yml",
            "test_paths:\n  - test/unit\n  - test/unit\n  - ./test/unit/\n  - test/unit/nested\n  - test/models\n",
        ),
    ])
    .expect("ingest tree");

    assert_modules(
        &app,
        &["ModelDefaultTest", "NestedFirstTest", "NestedSecondTest"],
    );
}

#[test]
fn missing_directories_add_no_test_roots() {
    let mut files = default_and_custom_tests();
    files.push(ruby_file("test/not_a_directory.rb", "NotADirectoryTest"));
    files.push(text_file(
        "roundhouse.yml",
        "test_paths:\n  - test/unit/missing\n",
    ));

    let app = ingest_files(files).expect("ingest tree");
    assert_modules(&app, DEFAULT_MODULES);
}

#[test]
fn configured_file_path_is_a_configuration_error() {
    let error = ingest_files([
        ruby_file("test/unit.rb", "UnitTest"),
        text_file("roundhouse.yml", "test_paths: [test/unit.rb]\n"),
    ])
    .expect_err("a configured file must not be treated as a directory");

    let IngestError::Parse { file, message } = error else {
        panic!("expected a config parse error, got {error:?}");
    };
    assert_eq!(file, "roundhouse.yml");
    assert!(message.contains("directories"), "{message}");
}

#[cfg(unix)]
#[test]
fn configured_symlink_root_is_rejected() {
    use std::os::unix::fs::symlink;

    let root = unique_tmp_dir("test_paths_symlink_root");
    let app_root = root.join("app");
    let outside = root.join("outside");
    std::fs::create_dir_all(app_root.join("test")).expect("create app test directory");
    std::fs::create_dir_all(&outside).expect("create outside directory");
    std::fs::write(
        outside.join("outside_test.rb"),
        test_source("OutsideRootTest"),
    )
    .expect("write outside test");
    symlink(&outside, app_root.join("test/external")).expect("create external directory link");
    std::fs::write(
        app_root.join("roundhouse.yml"),
        "test_paths: [test/external]\n",
    )
    .expect("write config");

    let error = ingest_app(&app_root).expect_err("external symlink root must fail");
    let IngestError::Parse { file, message } = error else {
        panic!("expected a config parse error, got {error:?}");
    };
    assert_eq!(file, app_root.join("roundhouse.yml").display().to_string());
    assert!(message.contains("symbolic link"), "{message}");
    std::fs::remove_dir_all(root).expect("remove temp app");
}

#[cfg(unix)]
#[test]
fn recursive_discovery_skips_symlinks_outside_the_app() {
    use std::os::unix::fs::symlink;

    let root = unique_tmp_dir("test_paths_recursive_symlinks");
    let app_root = root.join("app");
    let test_root = app_root.join("quality/specs");
    let outside_dir = root.join("outside_dir");
    let outside_file = root.join("outside_file.rb");
    std::fs::create_dir_all(&test_root).expect("create test root");
    std::fs::create_dir_all(&outside_dir).expect("create outside directory");
    std::fs::write(test_root.join("local_test.rb"), test_source("LocalTest"))
        .expect("write local test");
    std::fs::write(
        outside_dir.join("outside_dir_test.rb"),
        test_source("OutsideDirTest"),
    )
    .expect("write outside directory test");
    std::fs::write(&outside_file, test_source("OutsideFileTest")).expect("write outside file test");
    symlink(&outside_dir, test_root.join("external_dir")).expect("create external directory link");
    symlink(&outside_file, test_root.join("external_file_test.rb"))
        .expect("create external file link");
    std::fs::write(
        app_root.join("roundhouse.yml"),
        "test_paths: [quality/specs]\n",
    )
    .expect("write config");

    let app = ingest_app(&app_root).expect("ingest app");
    assert_modules(&app, &["LocalTest"]);
    std::fs::remove_dir_all(root).expect("remove temp app");
}

#[test]
fn invalid_configuration_stops_ingestion_with_the_config_path() {
    let invalid_configs = [
        ("test_paths: [\n", None),
        ("test_paths: test/unit\n", None),
        ("unknown_key: true\n", None),
        ("test_paths: [\"\"]\n", Some("\"\"")),
        ("test_paths: [\".\"]\n", Some("\".\"")),
        ("test_paths: [\"../tests\"]\n", Some("\"../tests\"")),
        ("test_paths: [\"/tmp/tests\"]\n", Some("\"/tmp/tests\"")),
    ];

    for (config, rejected_path) in invalid_configs {
        let error = ingest_files([
            ruby_file("test/models/model_default_test.rb", "ModelDefaultTest"),
            text_file("roundhouse.yml", config),
        ])
        .expect_err("invalid configuration must fail ingestion");
        match error {
            IngestError::Parse { file, message } => {
                assert_eq!(file, "roundhouse.yml");
                if let Some(path) = rejected_path {
                    assert_eq!(
                        message,
                        format!(
                            "test_paths entries must be non-empty app-relative directories without '..': {path}"
                        )
                    );
                }
            }
            other => panic!("expected a config parse error, got {other:?}"),
        }
    }
}

fn test_source_paths(app: &App, root: Option<&Path>) -> Vec<String> {
    let mut paths: Vec<_> = app
        .sources
        .iter()
        .filter_map(|source| {
            let path = Path::new(&source.path);
            let relative = match root {
                Some(root) => path.strip_prefix(root).ok()?,
                None => path,
            };
            (relative
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("rb"))
            .then(|| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    paths.sort();
    paths
}

#[test]
fn filesystem_and_map_vfs_select_the_same_configured_tests() {
    let root = unique_tmp_dir("test_paths_parity");
    let files = vec![
        ruby_file("test/models/default_test.rb", "DefaultTest"),
        ruby_file("quality/specs/nested/custom_test.rb", "CustomTest"),
        text_file(
            "roundhouse.yml",
            "test_paths: [quality/specs, ./quality/specs]\n",
        ),
    ];

    for (relative, content) in &files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("file parent")).expect("create directory");
        std::fs::write(path, content).expect("write test app file");
    }

    let fs_app = ingest_app(&root).expect("ingest filesystem app");
    let map_app = ingest_files(files).expect("ingest map app");
    assert_eq!(module_names(&fs_app), module_names(&map_app));
    assert_eq!(
        test_source_paths(&fs_app, Some(&root)),
        test_source_paths(&map_app, None)
    );
    assert_eq!(
        test_source_paths(&map_app, None),
        vec![
            "quality/specs/nested/custom_test.rb".to_string(),
            "test/models/default_test.rb".to_string(),
        ]
    );

    std::fs::remove_dir_all(root).expect("remove temp app");
}
