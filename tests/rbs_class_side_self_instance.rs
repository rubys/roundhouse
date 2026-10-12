//! A class-side `def self.make; new; end` returns the receiving class's
//! instance: `Child.make` is a Child. Analysis keeps `SelfInstance` on
//! that signature so dispatch can substitute the receiver, and the RBS
//! sidecar spells it `instance`, which in a singleton method means the
//! same thing. It used to reach emit as `self_instance_type` and print
//! `RoundhouseUnsupportedSelfInstance`.

use roundhouse::App;
use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

const SOURCE: &str = r#"class Widget
  def self.make
    new
  end

  def self.pair
    [new, new]
  end

  def name
    "w"
  end
end

class Child < Widget
  def label
    "c"
  end
end

class Shelf
  def self.first_label
    Child.make.label
  end
end
"#;

#[test]
fn a_class_side_new_is_the_rbs_instance_type() {
    let classes = ingest_library_classes(SOURCE.as_bytes(), "widget.rb").expect("ingest");
    let mut app = App::new();
    app.library_classes.extend(classes);
    Analyzer::new(&app).analyze(&mut app);
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| emit_library(&app));
    let rbs = files
        .iter()
        .find(|f| f.path.ends_with("sig/app/models/widget.rbs"))
        .map(|f| f.content.clone())
        .expect("widget.rbs");
    assert!(rbs.contains("def self.make: () -> instance"), "{rbs}");
    assert!(rbs.contains("def self.pair: () -> Array[instance]"), "{rbs}");
    assert!(!rbs.contains("RoundhouseUnsupported"), "{rbs}");
    assert!(diags.is_empty(), "{diags:#?}");
    // As in Ruby, `Child.make` is a Child: `label` is Child's alone.
    let shelf = files
        .iter()
        .find(|f| f.path.ends_with("sig/app/models/shelf.rbs"))
        .map(|f| f.content.clone())
        .expect("shelf.rbs");
    assert!(shelf.contains("def self.first_label: () -> String"), "{shelf}");

    let path = std::env::temp_dir().join(format!("rh_widget_{}.rbs", std::process::id()));
    std::fs::write(&path, &rbs).unwrap();
    let parsed = std::process::Command::new("ruby")
        .args(["-rrbs", "-e", "RBS::Parser.parse_signature(File.read(ARGV[0]))"])
        .arg(&path)
        .output()
        .expect("ruby with the rbs gem");
    assert!(parsed.status.success(), "{rbs}\n{}", String::from_utf8_lossy(&parsed.stderr));
}
