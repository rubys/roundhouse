use std::{collections::HashMap, fs, path::PathBuf, process::Command};

use roundhouse::runtime_src::parse_methods_with_rbs;
use roundhouse::{analyze::Analyzer, emit::rust, ingest::ingest_app_from_tree};

#[test]
fn rails_runtime_is_typed_and_included_in_rust_runtime() {
    let ruby = include_str!("../runtime/ruby/rails.rb");
    let rbs = include_str!("../runtime/ruby/rails.rbs");
    let methods = parse_methods_with_rbs(ruby, rbs).expect("Rails Ruby/RBS surface is typed");
    assert!(
        methods
            .iter()
            .any(|method| method.name.as_str() == "public_path")
    );

    let mut app = ingest_app_from_tree(HashMap::<PathBuf, Vec<u8>>::new())
        .expect("empty synthetic app ingests");
    Analyzer::new(&app).analyze(&mut app);
    let files = rust::emit(&app);
    let rails = files
        .iter()
        .find(|file| file.path.to_string_lossy() == "src/rails.rs")
        .expect("Rails runtime unit is emitted for Rust");
    assert!(
        rails.content.contains("pub struct Rails"),
        "{}",
        rails.content
    );
    assert!(
        rails.content.contains("pub fn public_path"),
        "{}",
        rails.content
    );

    let scratch =
        std::env::temp_dir().join(format!("roundhouse-rails-runtime-{}", std::process::id()));
    let src = scratch.join("src");
    fs::create_dir_all(&src).expect("create isolated Rust compile dir");
    fs::write(src.join("rails.rs"), &rails.content).expect("write generated Rails runtime");
    fs::write(
        src.join("main.rs"),
        r#"mod rails;
fn main() {
    assert_eq!(
        rails::Rails::root()
            .join(vec!["config".to_string(), "routes.rb".to_string()])
            .to_s(),
        "./config/routes.rb"
    );
    assert_eq!(
        rails::Rails::public_path()
            .join(vec!["avatars".to_string()])
            .to_path(),
        "public/avatars"
    );
}
"#,
    )
    .expect("write minimal Rust behavior probe");
    let check = Command::new("rustc")
        .arg("--edition=2021")
        .arg(src.join("main.rs"))
        .arg("-o")
        .arg(scratch.join("rails_runtime"))
        .output()
        .expect("rustc is installed");
    assert!(
        check.status.success(),
        "generated Rails Rust path behavior does not compile:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let run = Command::new(scratch.join("rails_runtime"))
        .output()
        .expect("run generated Rails Rust behavior probe");
    let _ = fs::remove_dir_all(&scratch);
    assert!(
        run.status.success(),
        "generated Rails Rust path behavior failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn app_class_can_call_the_supported_rails_path_surface() {
    let tree = [
        (
            PathBuf::from("app/models/rails_path_probe.rb"),
            b"class RailsPathProbe\n  def root_is_relative\n    Rails.root.to_path == \".\"\n  end\nend\n"
                .to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("synthetic app ingests");
    Analyzer::new(&app).analyze(&mut app);
    let files = rust::emit(&app);
    let model = files
        .iter()
        .find(|file| file.path.to_string_lossy() == "src/app_classes/rails_path_probe_class.rs")
        .expect("Rails path probe is emitted");
    let rails = files
        .iter()
        .find(|file| file.path.to_string_lossy() == "src/rails.rs")
        .expect("Rails runtime is emitted");
    assert!(
        model.content.contains("use crate::rails::Rails;") && model.content.contains("Rails::root"),
        "generated application class does not call the emitted Rails runtime:\n{}",
        model.content
    );

    let scratch =
        std::env::temp_dir().join(format!("roundhouse-rails-app-path-{}", std::process::id()));
    let src = scratch.join("src");
    fs::create_dir_all(&src).expect("create generated application probe directory");
    fs::write(src.join("rails.rs"), &rails.content).expect("write generated Rails runtime");
    let app_classes = src.join("app_classes");
    fs::create_dir_all(&app_classes).expect("create generated application class directory");
    fs::write(
        app_classes.join("rails_path_probe_class.rs"),
        &model.content,
    )
    .expect("write generated application class");
    fs::write(
        src.join("main.rs"),
        r#"mod rails;
mod user_agent {}
mod view_helpers { pub struct ViewHelpers; }
mod http {
    pub trait RubyToS { fn ruby_to_s(&self) -> String; }
    impl RubyToS for String { fn ruby_to_s(&self) -> String { self.clone() } }
}
mod app_classes {
    pub use crate::rails::Rails;
    #[path = "rails_path_probe_class.rs"]
    pub mod rails_path_probe_class;
    pub use rails_path_probe_class::RailsPathProbe;
}
fn main() {
    assert_eq!(
        app_classes::RailsPathProbe::default().root_is_relative(),
        true
    );
}
"#,
    )
    .expect("write generated Rust application harness");
    let check = Command::new("rustc")
        .arg("--edition=2021")
        .arg(src.join("main.rs"))
        .arg("-o")
        .arg(scratch.join("rails_app_path"))
        .output()
        .expect("rustc is installed");
    assert!(
        check.status.success(),
        "generated app call to Rails.root does not compile:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let run = Command::new(scratch.join("rails_app_path"))
        .output()
        .expect("run generated Rust application harness");
    let _ = fs::remove_dir_all(&scratch);
    assert!(
        run.status.success(),
        "generated app Rails.root behavior failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn rails_path_and_environment_runtime_behave_as_documented() {
    let script = r#"
      require_relative "runtime/ruby/rails"
      abort unless Rails.env.development?
      Rails.env_name = "production"
      abort unless Rails.env.production?
      abort unless Rails.root.join("config", "routes.rb").to_s == "./config/routes.rb"
      abort unless Rails.public_path.join("avatars").to_path == "public/avatars"
    "#;
    let output = Command::new("ruby")
        .args(["-e", script])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Ruby runtime is installed");
    assert!(
        output.status.success(),
        "Rails runtime behavior failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
