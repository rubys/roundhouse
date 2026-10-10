//! Class/module constants are emitted in their Rust file's owner scope.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn library_class_and_module_constants_are_emitted_in_owner_scope() {
    let sources = [
        (
            "app/models/campfire_class_constants.rb",
            "class CampfireClassConstants\n  RETRY_LIMIT = 3\n  COOKIE_NAME = \"campfire\"\nend\n",
        ),
        (
            "app/models/campfire_module_constants.rb",
            "module CampfireModuleConstants\n  PAGE_SIZE = 25\n  def self.page_size\n    PAGE_SIZE\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = sources
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest sources");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);

    let emitted = rust::emit(&app);
    let class = emitted
        .iter()
        .find(|file| file.path.ends_with("campfire_class_constants_class.rs"))
        .expect("class output")
        .content
        .as_str();
    let module = emitted
        .iter()
        .find(|file| file.path.ends_with("campfire_module_constants_class.rs"))
        .expect("module output")
        .content
        .as_str();

    assert!(
        class.contains("pub const RETRY_LIMIT: i64 = 3_i64;"),
        "{class}"
    );
    assert!(
        class.contains("pub const COOKIE_NAME: &str = \"campfire\";"),
        "{class}"
    );
    assert!(
        module.contains("pub const PAGE_SIZE: i64 = 25_i64;"),
        "{module}"
    );
    assert!(
        module.matches("PAGE_SIZE").count() >= 2,
        "module method should read its emitted owner constant:\n{module}"
    );
    assert!(module.find("pub const PAGE_SIZE").unwrap() < module.find("pub struct").unwrap());
}

#[test]
fn array_constant_iteration_binds_its_mutex_guard_for_the_loop() {
    let sources = [(
        "app/models/campfire_array_constants.rb",
        "module CampfireArrayConstants\n  NAMES = [\"alpha\"]\n  def self.find_name\n    NAMES.each { |name| return name if name == \"alpha\" }\n    \"\"\n  end\nend\n",
    )];
    let tree: HashMap<PathBuf, Vec<u8>> = sources
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest sources");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);

    let emitted = rust::emit(&app);
    let source = emitted
        .iter()
        .find(|file| file.path.ends_with("campfire_array_constants_class.rs"))
        .expect("module output")
        .content
        .as_str();

    assert!(
        source.contains(
            "let __rh_const_items = NAMES.lock().unwrap(); for name in __rh_const_items.iter()"
        ),
        "array constant iteration must borrow through the emitted mutex guard:\n{source}"
    );
    assert!(
        source.contains("return (name.clone()).to_string()"),
        "a Ruby non-local return inside each must remain a Rust method return:\n{source}"
    );
}
