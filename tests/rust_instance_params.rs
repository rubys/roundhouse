//! Synthesized `attr_accessor` writers and the `ActiveModel::Model`
//! constructor carry known signatures. Emit must render those, not `()`.
//!
//! Compile only the generated constructor and writer fragments this
//! change owns; this is not a full-class or full-application build gate.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

fn emit_class(source: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [(
        PathBuf::from("lib/rails_ext/widget.rb"),
        source.as_bytes().to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("_class.rs"))
        .expect("class emit")
        .content
}

fn method_slice<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("pub fn {name}("))
        .unwrap_or_else(|| panic!("missing `{name}` in:\n{source}"));
    let rest = &source[start..];
    let end = rest.find("\n    pub fn ").unwrap_or(rest.len());
    &rest[..end]
}

fn rustc(source: &str, label: &str) {
    let dir = std::env::temp_dir().join(format!("rh-rust-instance-params-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let path = dir.join("probe.rs");
    std::fs::write(&path, source).expect("write probe");
    let deps = std::env::current_exe().expect("test executable");
    let deps = deps.parent().expect("deps directory");
    let serde_json = std::fs::read_dir(deps)
        .expect("deps")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libserde_json-") && name.ends_with(".rlib"))
        })
        .expect("built serde_json rlib");
    let compile = Command::new("rustc")
        .args(["--edition", "2021", "--extern"])
        .arg(format!("serde_json={}", serde_json.display()))
        .arg("-L")
        .arg(deps)
        .arg("-o")
        .arg(dir.join("probe"))
        .arg(&path)
        .output()
        .expect("rustc");
    assert!(
        compile.status.success(),
        "{label} must compile:\n{}{}\n--- source ---\n{source}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(dir.join("probe")).output().expect("execute probe");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
}

#[test]
fn synthesized_writer_and_constructor_keep_known_signatures() {
    // ActionText::Attachment::OpengraphEmbed shape: attr_accessor plus
    // the synthesized ActiveModel::Model constructor. `label` is an
    // unrelated getter whose inferred String signature must survive.
    let source = r#"
class Widget
  include ActiveModel::Model
  attr_accessor :href, :url

  def label
    "widget"
  end
end
"#;
    let emitted = emit_class(source);
    let setter = method_slice(&emitted, "set_href");
    assert!(
        setter.contains("pub fn set_href(&mut self, value: serde_json::Value)"),
        "writer param is the untyped value the generator declared, not ():\n{setter}\n---\n{emitted}"
    );
    let url = method_slice(&emitted, "set_url");
    assert!(
        url.contains("value: serde_json::Value"),
        "sibling writer uses the same known signature:\n{url}"
    );
    let constructor = method_slice(&emitted, "new");
    assert!(
        constructor.contains(
            "pub fn new(attrs: std::collections::HashMap<String, serde_json::Value>) -> Self"
        ),
        "constructor attrs bag is the symbol-keyed hash the lowerer declared, not ():\n{constructor}\n---\n{emitted}"
    );
    assert!(
        constructor.contains(".get(") && constructor.contains("serde_json::Value::Null"),
        "typed hash reads are a total lookup, not a panicking index:\n{constructor}"
    );
    assert!(
        constructor.contains("let mut href: serde_json::Value"),
        "untyped hash values land in a Value field, not ():\n{constructor}"
    );
    let label = method_slice(&emitted, "label");
    assert!(
        label.contains("-> String"),
        "unrelated explicit getter stays signed and is not deleted to make the class compile:\n{label}"
    );
    // Generated readers stay in the emit. Their `Option<Value>` return
    // against a `Value` field is a pre-existing getter path, not the
    // constructor or writer this change owns, so the compile claim is
    // the two fragments — not a class with those readers stripped.
    assert!(
        emitted.contains("pub fn href(&self)") && emitted.contains("pub fn url(&self)"),
        "generated readers must remain in the emit:\n{emitted}"
    );

    rustc(
        &format!(
            "#![allow(dead_code, unused_mut)]\nstruct Widget {{ href: serde_json::Value, url: serde_json::Value }}\nimpl Widget {{\n{constructor}\n{setter}\n{url}\n}}\n{}",
            r#"
fn main() {
    let mut widget = Widget::new(std::collections::HashMap::from([
        ("href".into(), serde_json::json!("left")),
        ("url".into(), serde_json::json!("right")),
    ]));
    assert_eq!(widget.href, serde_json::json!("left"));
    assert_eq!(widget.url, serde_json::json!("right"));
    widget.set_href(serde_json::json!(42));
    widget.set_url(serde_json::Value::Null);
    assert_eq!(widget.href, serde_json::json!(42));
    assert_eq!(widget.url, serde_json::Value::Null);
    let empty = Widget::new(std::collections::HashMap::new());
    assert_eq!(empty.href, serde_json::Value::Null);
    assert_eq!(empty.url, serde_json::Value::Null);
}
"#
        ),
        "constructor-and-writers",
    );
}
