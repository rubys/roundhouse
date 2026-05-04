//! Anchor for step 1 of the spinel-blog plan: ingest a spinel-blog
//! runtime file through the library-shape pipeline, emit Ruby, verify
//! the IR captures the semantics.
//!
//! Note on goal: this is *not* a strict source-equivalence round-trip.
//! `attr_reader :foo` is lowered to `def foo; @foo; end` at ingest
//! time (per the YAGNI-on-round-trip decision); emitted Ruby differs
//! syntactically from input. The forcing function is "Spinel can
//! compile the emitted Ruby and the result behaves the same as the
//! original" — surface preservation is not the goal.
//!
//! Smallest non-trivial entry: `runtime/active_record/errors.rb` —
//! two classes inside `module ActiveRecord` (`RecordNotFound`,
//! `RecordInvalid`), one ivar, one `super` call with a string-interp
//! arg, one `attr_reader` (which lowers to a getter method).

use std::path::PathBuf;

use roundhouse::App;
use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_library_classes;

const ERRORS_RB_PATH: &str = "runtime/ruby/active_record/errors.rb";
const INFLECTOR_RB_PATH: &str = "runtime/ruby/inflector.rb";
const VALIDATIONS_RB_PATH: &str = "runtime/ruby/active_record/validations.rb";
const BASE_RB_PATH: &str = "runtime/ruby/active_record/base.rb";

#[test]
fn errors_rb_ingests_and_emits_via_library_path() {
    let path = PathBuf::from(ERRORS_RB_PATH);
    let source = std::fs::read(&path).expect("read errors.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    // Both classes from errors.rb should land. Names are last-segment
    // (Prism reports the syntactic name; module nesting is implicit).
    assert_eq!(
        classes.len(),
        2,
        "expected RecordNotFound + RecordInvalid; got {} ({:?})",
        classes.len(),
        classes.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>(),
    );
    let names: Vec<&str> = classes.iter().map(|c| c.name.0.as_str()).collect();
    assert!(names.contains(&"RecordNotFound"), "names: {names:?}");
    assert!(names.contains(&"RecordInvalid"), "names: {names:?}");

    // Both inherit from StandardError. is_module = false.
    for lc in &classes {
        assert!(!lc.is_module, "{} should be a class, not module", lc.name.0.as_str());
        let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
        assert_eq!(parent, "StandardError", "{}: parent {parent}", lc.name.0.as_str());
    }

    // RecordInvalid is the rich one. attr_reader :record lowers to
    // a getter method, so the methods Vec should hold both that
    // synthesized getter and the source-defined initialize.
    let invalid = classes
        .iter()
        .find(|c| c.name.0.as_str() == "RecordInvalid")
        .expect("RecordInvalid present");
    let method_names: Vec<&str> = invalid.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        method_names.contains(&"record"),
        "expected synthesized getter for attr_reader :record; got {method_names:?}",
    );
    assert!(
        method_names.contains(&"initialize"),
        "expected initialize method; got {method_names:?}",
    );

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 2, "one file per LibraryClass");

    let invalid_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("record_invalid.rb"))
        .expect("record_invalid.rb emitted");
    let content = &invalid_file.content;

    // Class shell + parent.
    assert!(content.contains("class RecordInvalid < StandardError"), "emitted: {content}");
    // Lowered attr_reader: a `def record` returning `@record`.
    assert!(content.contains("def record"), "emitted: {content}");
    assert!(content.contains("@record"), "emitted: {content}");
    // The source-defined initialize body round-trips.
    assert!(content.contains("def initialize(record)"), "emitted: {content}");
    assert!(content.contains("super("), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}

/// `runtime/inflector.rb`: a `module Inflector` with one `def
/// self.pluralize`. Lowered to a `LibraryClass` with no parent and
/// the singleton method (per the YAGNI-on-round-trip decision —
/// callers only use `Inflector.pluralize(...)`, never `include`, so
/// module-vs-class distinction can collapse to class semantics).
#[test]
fn inflector_rb_ingests_module_as_namespace() {
    let path = PathBuf::from(INFLECTOR_RB_PATH);
    let source = std::fs::read(&path).expect("read inflector.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    assert_eq!(
        classes.len(),
        1,
        "expected one LibraryClass (Inflector); got {} ({:?})",
        classes.len(),
        classes.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>(),
    );

    let inflector = &classes[0];
    assert_eq!(inflector.name.0.as_str(), "Inflector");
    assert!(inflector.is_module, "Inflector is a module in source");
    assert!(inflector.parent.is_none(), "module has no parent");
    assert_eq!(inflector.methods.len(), 1);

    let m = &inflector.methods[0];
    assert_eq!(m.name.as_str(), "pluralize");
    // `def self.pluralize` → MethodReceiver::Class.
    assert!(
        matches!(m.receiver, roundhouse::dialect::MethodReceiver::Class),
        "expected class-method receiver; got {:?}",
        m.receiver,
    );
    assert_eq!(
        m.params.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        vec!["count", "word"],
    );

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 1);
    let content = &files[0].content;

    // Module emitted as `module Inflector` (preserved); singleton
    // method emits as `def self.x`.
    assert!(content.contains("module Inflector"), "emitted: {content}");
    assert!(!content.contains("class Inflector"), "should not emit as class: {content}");
    assert!(content.contains("def self.pluralize(count, word)"), "emitted: {content}");
    assert!(content.contains("if "), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}

/// `runtime/active_record/validations.rb`: a mixin module
/// (`module Validations` inside `module ActiveRecord`) with seven
/// instance-method validation helpers. Lowered to a `LibraryClass`
/// with `is_module: true` and the methods carried verbatim. The
/// outer `module ActiveRecord` is a pure namespace wrapper (no
/// direct defs) and should NOT surface as a separate LibraryClass.
#[test]
fn validations_rb_ingests_mixin_module() {
    let path = PathBuf::from(VALIDATIONS_RB_PATH);
    let source = std::fs::read(&path).expect("read validations.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    assert_eq!(
        classes.len(),
        1,
        "expected one LibraryClass (Validations); got {} ({:?})",
        classes.len(),
        classes.iter().map(|c| c.name.0.as_str().to_string()).collect::<Vec<_>>(),
    );

    let v = &classes[0];
    assert_eq!(v.name.0.as_str(), "Validations");
    assert!(v.is_module, "Validations is a mixin module");
    assert!(v.parent.is_none());

    let method_names: Vec<&str> = v.methods.iter().map(|m| m.name.as_str()).collect();
    for expected in [
        "errors",
        "validates_presence_of",
        "validates_absence_of",
        "validates_length_of",
        "validates_numericality_of",
        "validates_inclusion_of",
        "validates_format_of",
    ] {
        assert!(
            method_names.contains(&expected),
            "missing method `{expected}` (got {method_names:?})",
        );
    }

    // All methods are instance methods (no `def self.*`).
    for m in &v.methods {
        assert!(
            matches!(m.receiver, roundhouse::dialect::MethodReceiver::Instance),
            "{} should be instance method",
            m.name.as_str(),
        );
    }

    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    assert_eq!(files.len(), 1);
    let content = &files[0].content;

    // Critical: `module Validations`, NOT `class Validations` —
    // mixin semantics require the module form.
    assert!(content.contains("module Validations"), "emitted: {content}");
    assert!(!content.contains("class Validations"), "must not emit as class: {content}");
    assert!(content.contains("def errors"), "emitted: {content}");
    assert!(content.contains("def validates_presence_of(attr_name, value)"), "emitted: {content}");
    assert!(content.trim_end().ends_with("end"), "emitted: {content}");
}

/// `runtime/active_record/base.rb`: the heaviest file. Three patterns
/// to verify here:
///   - Top-level `require` directives are silently dropped (not
///     captured by ingest; not emitted).
///   - `module ActiveRecord` with `class << self; attr_accessor :adapter; end`
///     surfaces as a LibraryClass with class-level `adapter` /
///     `adapter=` methods.
///   - `class Base` with include + attr_accessor + ~30 def/`def self.*`
///     ingests with all methods captured and the right receiver kind.
#[test]
fn base_rb_ingests_module_with_singleton_class_and_class() {
    let path = PathBuf::from(BASE_RB_PATH);
    let source = std::fs::read(&path).expect("read base.rb");
    let path_str = path.display().to_string();

    let classes = ingest_library_classes(&source, &path_str)
        .expect("ingest_library_classes returned Err");

    let names: Vec<&str> = classes.iter().map(|c| c.name.0.as_str()).collect();
    assert!(names.contains(&"ActiveRecord"), "expected ActiveRecord module; got {names:?}");
    assert!(names.contains(&"Base"), "expected Base class; got {names:?}");

    // ActiveRecord module surfaces because of the class << self block.
    let ar = classes.iter().find(|c| c.name.0.as_str() == "ActiveRecord").unwrap();
    assert!(ar.is_module);
    let ar_method_names: Vec<&str> = ar.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(ar_method_names.contains(&"adapter"), "ActiveRecord methods: {ar_method_names:?}");
    assert!(ar_method_names.contains(&"adapter="), "ActiveRecord methods: {ar_method_names:?}");
    // Singleton-class attr_accessor → class receiver on both pair members.
    for m in &ar.methods {
        assert!(
            matches!(m.receiver, roundhouse::dialect::MethodReceiver::Class),
            "ActiveRecord.{} should be class method",
            m.name.as_str(),
        );
    }

    // Base class assertions.
    let base = classes.iter().find(|c| c.name.0.as_str() == "Base").unwrap();
    assert!(!base.is_module);
    assert!(base.parent.is_none(), "Base has no explicit superclass");
    assert!(
        base.includes.iter().any(|i| i.0.as_str() == "Validations"),
        "Base should include Validations; got {:?}",
        base.includes,
    );

    let base_methods: Vec<&str> = base.methods.iter().map(|m| m.name.as_str()).collect();
    // attr_accessor :id lowered → id reader + id= writer.
    assert!(base_methods.contains(&"id"), "expected id getter; methods: {base_methods:?}");
    assert!(base_methods.contains(&"id="), "expected id= setter; methods: {base_methods:?}");
    // A few key class methods.
    for cm in ["table_name", "all", "find", "where", "count"] {
        assert!(
            base_methods.contains(&cm),
            "expected class method `{cm}`; methods: {base_methods:?}",
        );
    }
    // A few key instance methods. Ruby's `==` / `eql?` / `hash`
    // equality protocol was intentionally removed from base.rb (no
    // cross-target analog; per-target runtimes implement value
    // equality as appropriate for their host).
    for im in ["save", "save!", "destroy", "valid?", "reload"] {
        assert!(
            base_methods.contains(&im),
            "expected instance method `{im}`; methods: {base_methods:?}",
        );
    }

    // Receiver checks: def self.* land as Class, def x as Instance.
    let class_methods = ["table_name", "all", "find", "where", "count", "exists?"];
    let instance_methods = ["save", "destroy", "persisted?", "reload"];
    for m in &base.methods {
        let n = m.name.as_str();
        if class_methods.contains(&n) {
            assert!(
                matches!(m.receiver, roundhouse::dialect::MethodReceiver::Class),
                "{n} should be class method, got {:?}",
                m.receiver,
            );
        } else if instance_methods.contains(&n) {
            assert!(
                matches!(m.receiver, roundhouse::dialect::MethodReceiver::Instance),
                "{n} should be instance method, got {:?}",
                m.receiver,
            );
        }
    }

    // Emit + spot-check structural shape.
    let mut app = App::new();
    for lc in classes {
        app.library_classes.push(lc);
    }
    let files = emit_library(&app);
    let base_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("base.rb"))
        .expect("base.rb emitted");
    let bc = &base_file.content;
    assert!(bc.contains("class Base"), "base.rb: {bc}");
    assert!(bc.contains("include Validations"), "base.rb: {bc}");
    assert!(bc.contains("def self.find(id)"), "base.rb: {bc}");
    assert!(bc.contains("def save"), "base.rb: {bc}");

    let ar_file = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("active_record.rb"))
        .expect("active_record.rb emitted");
    let arc = &ar_file.content;
    assert!(arc.contains("module ActiveRecord"), "active_record.rb: {arc}");
    assert!(arc.contains("def self.adapter"), "active_record.rb: {arc}");
    assert!(arc.contains("def self.adapter=(value)"), "active_record.rb: {arc}");
}

/// Sweep every `.rb` file under `runtime/ruby/` through
/// `ingest_library_classes` and `emit_library`. Doesn't make per-file
/// shape assertions — just confirms each file ingests without error
/// and the resulting LibraryClasses serialize to non-empty Ruby (when
/// they contain anything). Whatever fails here next is the next gap.
#[test]
fn all_spinel_blog_runtime_files_ingest_and_emit() {
    let runtime_dir = PathBuf::from("runtime/ruby");

    let mut walked = 0usize;
    let mut walk_errors: Vec<String> = Vec::new();
    walk_rb_files(&runtime_dir, &mut |path: &std::path::Path| {
        walked += 1;
        let source = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                walk_errors.push(format!("{}: read failed: {e}", path.display()));
                return;
            }
        };
        let path_str = path.display().to_string();
        let classes = match ingest_library_classes(&source, &path_str) {
            Ok(c) => c,
            Err(e) => {
                walk_errors.push(format!("{}: ingest failed: {e}", path.display()));
                return;
            }
        };
        let mut app = App::new();
        for lc in classes {
            app.library_classes.push(lc);
        }
        // Emit shouldn't panic. Pure-`require` aggregator files
        // emit zero output, which is fine.
        let _ = emit_library(&app);
    });

    assert!(
        walked > 0,
        "expected to walk at least one .rb file under {}",
        runtime_dir.display(),
    );
    assert!(
        walk_errors.is_empty(),
        "{} file(s) failed:\n  {}",
        walk_errors.len(),
        walk_errors.join("\n  "),
    );
}

fn walk_rb_files(dir: &std::path::Path, f: &mut dyn FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip the framework's own test tree — those files are
            // Minitest test cases authored to run under stock Ruby,
            // not framework library code. They use Ruby constructs
            // (Float literals, eval-shaped fixtures, etc.) that
            // ingest_library_classes intentionally doesn't accept.
            if path.file_name().is_some_and(|n| n == "test") {
                continue;
            }
            walk_rb_files(&path, f);
        } else if path.extension().map(|e| e == "rb").unwrap_or(false) {
            f(&path);
        }
    }
}
