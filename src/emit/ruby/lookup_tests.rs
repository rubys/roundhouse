//! Regression tests for the source-derived whole-program lookup indexes.
use super::*;
use std::cell::Cell;
use crate::lower::model_to_library::schema::{self, lookup_test_support as demand};

thread_local! {
    pub(super) static UNCACHED: Cell<bool> = const { Cell::new(false) };
    pub(super) static READER_BUILDS: Cell<usize> = const { Cell::new(0) };
    pub(super) static READER_SCAN_VISITS: Cell<usize> = const { Cell::new(0) };
    pub(super) static READER_CLASS_VISITS: Cell<usize> = const { Cell::new(0) };
    pub(super) static NAME_BUILDS: Cell<usize> = const { Cell::new(0) };
    pub(super) static NAME_CLASS_VISITS: Cell<usize> = const { Cell::new(0) };
    pub(super) static CONSTANT_PATH_BUILDS: Cell<usize> = const { Cell::new(0) };
    pub(super) static BARE_MODULE_BUILDS: Cell<usize> = const { Cell::new(0) };
}

fn uncached<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool, bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            UNCACHED.set(self.0);
            demand::UNCACHED.set(self.1);
        }
    }
    let _restore = Restore(UNCACHED.replace(true), demand::UNCACHED.replace(true));
    f()
}

fn reset() {
    READER_BUILDS.set(0);
    READER_SCAN_VISITS.set(0);
    READER_CLASS_VISITS.set(0);
    NAME_BUILDS.set(0);
    NAME_CLASS_VISITS.set(0);
    BARE_MODULE_BUILDS.set(0);
    CONSTANT_PATH_BUILDS.set(0);
    demand::BUILDS.set(0);
    demand::MODEL_VISITS.set(0);
}

fn classes(src: &str) -> Vec<LibraryClass> {
    crate::ingest::ingest_library_classes(src.as_bytes(), "synthetic.rb").unwrap()
}

fn tree(files: &[(&str, &str)]) -> App {
    crate::ingest::ingest_app_from_tree(files.iter().map(|(path, src)| {
        (PathBuf::from(path), src.as_bytes().to_vec())
    }).collect()).unwrap()
}

fn models(n: usize, demand_body: bool) -> App {
    let src = if demand_body {
        "class Widget < ApplicationRecord\n def check; label_changed?; end\nend\n"
    } else {
        "class Widget < ApplicationRecord\n def check; nil; end\nend\n"
    };
    let mut app = tree(&[
        ("db/schema.rb", "ActiveRecord::Schema.define do\n create_table :widgets do |t|\n t.string :label\n end\nend\n"),
        ("app/models/widget.rb", src),
    ]);
    let model = app.models.remove(0);
    app.models = (0..n).map(|i| {
        let mut model = model.clone();
        model.name = ClassId(Symbol::from(format!("Widget{i}")));
        model
    }).collect();
    app
}

fn output(files: Vec<EmittedFile>) -> Vec<(String, String)> {
    files.into_iter().map(|f| (f.path.to_string_lossy().into_owned(), f.content)).collect()
}

#[test]
fn reader_index_matches_load_time_scan_including_ivars_and_bare_nested_reads() {
    let mut app = App::default();
    app.library_classes = classes("module Owner\n VALUE = 1\nend\nclass Qualified\n COPY = Owner::VALUE\nend\nclass Owner::Nested\n COPY = VALUE\nend\nclass Unrelated\n COPY = VALUE\nend\n");
    let read = Expr::new(Span::synthetic(), ExprNode::Const {
        path: vec![Symbol::from("Owner"), Symbol::from("VALUE")],
    });
    app.library_classes.iter_mut().find(|c| c.name.0.as_str() == "Unrelated").unwrap()
        .class_ivar_initializers.push(read);
    let owner = app.library_classes.iter().find(|c| c.name.0.as_str() == "Owner").unwrap();
    let resolve = |path: &Vec<String>, _: bool| Some(path.join("/"));
    let scanned = constant_readers(owner, &app, &app.library_classes, &resolve);
    assert_eq!(scanned["VALUE"], BTreeSet::from([
        "Qualified".into(), "Owner/Nested".into(), "Unrelated".into(),
    ]));
    let _guard = install_lookup_indexes(&app);
    assert_eq!(constant_readers(owner, &app, &app.library_classes, &resolve), scanned);
    let mut other = App::default();
    other.library_classes = classes("module Owner\n VALUE = 2\nend\n");
    assert!(ConstantReaderIndex::current(&other).is_none());
    assert!(constant_readers(&other.library_classes[0], &other, &other.library_classes, &resolve).is_empty());
}

#[test]
fn ordered_reopenings_parent_precedence_and_rooted_modules_match_scans() {
    let mut app = models(1, false);
    app.models[0].name = ClassId(Symbol::from("Same"));
    app.models[0].parent = None;
    app.library_classes = classes("module Same\n def same; end\nend\nclass Same < First\n def first; end\nend\nclass Same < Second\n def second; end\nend\nmodule Hook\n def hook; end\nend\nmodule Outer::Base\n def base; end\nend\n");
    let mut rooted = app.library_classes.iter().find(|c| c.name.0.as_str() == "Hook").unwrap().clone();
    rooted.name = ClassId(Symbol::from("::Hook"));
    app.library_classes.insert(0, rooted);
    let positions = |app: &App| library_classes_named("Same", app, &app.library_classes).iter()
        .map(|c| (c.is_module, c.parent.clone())).collect::<Vec<_>>();
    let expected = positions(&app);
    let parent = outer_class_parent("Same", &app, &app.library_classes);
    assert!(parent.is_none(), "first model's None parent wins over library classes");
    let lexical = lexical_parent("Outer::Leaf", &ClassId(Symbol::from("Base")), &app, &app.library_classes);
    let bare = first_module_bare_named("Hook", &app, &app.library_classes).unwrap() as *const _;
    reset();
    let _guard = install_lookup_indexes(&app);
    assert_eq!(positions(&app), expected);
    assert_eq!(outer_class_parent("Same", &app, &app.library_classes), parent);
    assert_eq!(outer_class_parent("Missing", &app, &app.library_classes), None);
    assert_eq!(lexical_parent("Outer::Leaf", &ClassId(Symbol::from("Base")), &app, &app.library_classes), lexical);
    assert_eq!(first_module_bare_named("Hook", &app, &app.library_classes).unwrap() as *const _, bare);
    assert_eq!(first_module_bare_named("Hook", &app, &app.library_classes).unwrap() as *const _, bare);
    assert_eq!(BARE_MODULE_BUILDS.get(), 1);
}

#[test]
fn guards_restore_same_app_different_app_and_unwound_emit_scopes() {
    let first = App::default();
    let second = App::default();
    assert!(NameIndex::current(&first).is_none());
    let outer = install_lookup_indexes(&first);
    let names = NameIndex::current(&first).unwrap();
    let readers = ConstantReaderIndex::current(&first).unwrap();
    {
        let _inner = install_lookup_indexes(&first);
        assert!(!std::rc::Rc::ptr_eq(&names, &NameIndex::current(&first).unwrap()));
    }
    assert!(std::rc::Rc::ptr_eq(&names, &NameIndex::current(&first).unwrap()));
    {
        let _inner = install_lookup_indexes(&second);
        assert!(NameIndex::current(&first).is_none());
        assert!(ConstantReaderIndex::current(&first).is_none());
        assert!(NameIndex::current(&second).is_some());
        // The real entry restores these same outer guards too.
        let _ = super::super::emit_spinel(&second);
        assert!(NameIndex::current(&second).is_some());
    }
    assert!(std::rc::Rc::ptr_eq(&names, &NameIndex::current(&first).unwrap()));
    assert!(std::rc::Rc::ptr_eq(&readers, &ConstantReaderIndex::current(&first).unwrap()));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _inner = install_lookup_indexes(&second);
        panic!("test unwind");
    }));
    assert!(std::rc::Rc::ptr_eq(&names, &NameIndex::current(&first).unwrap()));
    drop(outer);
    assert!(NameIndex::current(&first).is_none());
    assert!(ConstantReaderIndex::current(&first).is_none());
}

#[test]
fn real_app_extension_selection_keeps_first_of_either_name_not_innermost_first() {
    let mut app = App::default();
    app.library_classes = classes("module Widget::Registry\n def nested; end\nend\nmodule Registry\n def top; end\nend\nclass Widget\n extend Registry\n nested\n top\nend\n");
    // Pin collection order deliberately; both names can be present.
    app.library_classes.sort_by_key(|c| match c.name.0.as_str() {
        "Widget::Registry" => 0, "Registry" => 1, _ => 2,
    });
    let widget = app.library_classes.iter().find(|c| c.name.0.as_str() == "Widget").unwrap();
    let scanned = app_extension_calls(widget, &app, &app.library_classes).iter().map(|e| super::super::emit_expr(e)).collect::<Vec<_>>();
    assert!(scanned.iter().any(|s| s.trim() == "nested"));
    assert!(!scanned.iter().any(|s| s.trim() == "top"));
    let _guard = install_lookup_indexes(&app);
    assert_eq!(app_extension_calls(widget, &app, &app.library_classes).iter().map(|e| super::super::emit_expr(e)).collect::<Vec<_>>(), scanned);
    assert_eq!(output(emit_library_class_decls(&app)), uncached(|| output(emit_library_class_decls(&app))));
}

#[test]
fn actual_library_emission_keeps_owner_and_require_answers_including_data_context() {
    let app = tree(&[("app/services/sample.rb", "module Catalog\n VALUE = 7\n class Reader\n  COPY = Catalog::VALUE\n  def label; COPY; end\n end\nend\nclass Container\n Item = Data.define(:label) do\n  def doubled; label + label; end\n end\n class Nested\n end\nend\n")]);
    reset();
    let indexed = output(emit_library_class_decls(&app));
    let scanned = uncached(|| output(emit_library_class_decls(&app)));
    assert_eq!(indexed, scanned);
    assert!(indexed.iter().any(|(p, _)| p.ends_with("container.rb")));
    assert!(NameIndex::current(&app).is_none());
}

#[test]
fn data_factory_methods_come_from_the_lowered_slice() {
    let app = tree(&[(
        "app/services/sample.rb",
        "class Container\n Item = Data.define(:label) do\n  def icon; sanitize(label); end\n end\nend\n",
    )]);
    reset();
    let indexed = output(emit_library_class_decls(&app));
    assert_eq!((READER_BUILDS.get(), NAME_BUILDS.get()), (2, 2));
    let scanned = uncached(|| output(emit_library_class_decls(&app)));
    assert_eq!(indexed, scanned);
    let rb = indexed.iter().find(|(path, _)| path.ends_with("container.rb")).expect("container.rb");
    assert!(rb.1.contains("Data.define(:label) do"), "{}", rb.1);
    assert!(rb.1.contains("ActionView::ViewHelpers.sanitize"), "{}", rb.1);
    assert!(!rb.1.contains("\n    sanitize("), "{}", rb.1);
    assert!(NameIndex::current(&app).is_none());
}

#[test]
fn real_entry_build_counts_scale_with_app_size_not_emitted_method_count() {
    for n in [100, 200, 1000] {
        let mut app = App::default();
        let template = classes("class Item\n VALUE = 1\n def quiet; nil; end\nend\n").remove(0);
        app.library_classes = (0..n).map(|i| {
            let mut lc = template.clone();
            lc.name = ClassId(Symbol::from(format!("Item{i}")));
            lc
        }).collect();
        reset();
        let emitted = emit_library_class_decls(&app);
        assert_eq!(emitted.len(), n * 2);
        assert_eq!((READER_BUILDS.get(), NAME_BUILDS.get()), (1, 1));
        assert_eq!((READER_CLASS_VISITS.get(), NAME_CLASS_VISITS.get()), (n, n));
        assert_eq!(demand::BUILDS.get(), 0);
        println!("library n={n}: reader/name builds=1/1, visits={n}/{n}");
        reset();
        let scanned = uncached(|| emit_library_class_decls(&app));
        assert_eq!(output(emitted), output(scanned));
        assert_eq!(READER_SCAN_VISITS.get(), n * n);
        println!("library n={n}: uncached reader scan visits={}", n * n);
    }
    for n in [8, 16, 32] {
        let app = models(n, true);
        reset();
        let lowered = super::super::emit_lowered_models(&app);
        assert!(!lowered.is_empty());
        assert_eq!(demand::BUILDS.get(), 1);
        assert_eq!(demand::MODEL_VISITS.get(), n);
        println!("lowered models n={n}: demand builds=1, model visits={n}");
        reset();
        let scanned = uncached(|| super::super::emit_lowered_models(&app));
        assert_eq!(output(lowered), output(scanned));
        assert_eq!((demand::BUILDS.get(), demand::MODEL_VISITS.get()), (n, n * n));
        println!("lowered models n={n}: uncached demand builds={n}, model visits={}", n * n);
        reset();
        let mut app = models(n, false);
        for model in &mut app.models {
            model.table.0 = Symbol::from("no_table");
        }
        let _ = super::super::emit_lowered_models(&app);
        assert_eq!(demand::BUILDS.get(), 0, "demand set stays lazy without schema consumers");
    }
}

#[test]
fn model_demand_guard_restores_outer_slice_and_real_lowering_matches_uncached() {
    let first = models(3, true);
    let second = models(2, false);
    let cache = schema::ModelBodyNames::default();
    let outer = schema::share_model_body_names(&first.models, &cache);
    let indexed = output(super::super::emit_lowered_models(&first));
    assert_eq!(indexed, uncached(|| output(super::super::emit_lowered_models(&first))));
    {
        let _inner = schema::share_model_body_names(&second.models, &Default::default());
        let other = output(super::super::emit_lowered_models(&second));
        assert_eq!(other, uncached(|| output(super::super::emit_lowered_models(&second))));
        assert!(other.iter().filter(|(p, _)| p.ends_with(".rb")).all(|(_, s)| !s.contains("def label_changed?")));
    }
    assert_eq!(output(super::super::emit_lowered_models(&first)), indexed);
    drop(outer);
}

#[test]
fn whole_ruby_and_jruby_projects_are_scan_equivalent_and_pass_scoped() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog");
    let mut app = crate::ingest::ingest_app(&fixture).unwrap();
    crate::session::analyze_and_lower(&mut app);
    for target in [crate::project::BuildTarget::Ruby, crate::project::BuildTarget::Jruby] {
        reset();
        let indexed = crate::project::target_files(&app, &fixture, target).unwrap();
        let counts = (READER_BUILDS.get(), NAME_BUILDS.get(), demand::BUILDS.get());
        assert_eq!(counts.0, counts.1);
        assert!(counts.0 >= 3 && counts.0 <= 5, "assembly, model/controller/view emit and library scopes: {counts:?}");
        assert!(counts.2 > 0 && counts.2 < 8, "per-pass demand builds, not per-model: {counts:?}");
        let scanned = uncached(|| crate::project::target_files(&app, &fixture, target).unwrap());
        assert_eq!(indexed, scanned);
        assert!(NameIndex::current(&app).is_none());
        assert!(ConstantReaderIndex::current(&app).is_none());
        println!("{target:?}: {} files byte-identical, reader/name/demand builds={counts:?}", indexed.len());
    }
}

#[test]
fn hook_position_uses_the_first_reopening_with_or_without_indexes() {
    let mut app = App::default();
    app.library_classes = classes("module Hook\n def self.included(base); base.ready; end\nend\nmodule Hook\n def self.included(base); base.other; end\nend\nclass Container\n def self.ready; 1; end\n def self.other; 2; end\n include Hook\nend\n");
    let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "Container").unwrap();
    let target = ClassId(Symbol::from("::Hook"));
    let scanned = hooked_include_position(class, &target, &app, &app.library_classes);
    assert_eq!(scanned, Some(1));
    let _guard = install_lookup_indexes(&app);
    assert_eq!(hooked_include_position(class, &target, &app, &app.library_classes), scanned);
    assert_eq!(output(emit_library_class_decls(&app)), uncached(|| output(emit_library_class_decls(&app))));
}

#[test]
fn delegate_ingest_and_contract_survey_share_actual_model_body_walks() {
    let schema = "ActiveRecord::Schema.define do\n create_table :accounts do |t|\n t.string :name\n end\n create_table :profiles do |t|\n t.integer :account_id\n t.string :label\n end\nend\n";
    let source = [
        ("db/schema.rb", schema),
        ("app/models/account.rb", "class Account < ApplicationRecord\n has_one :profile\n delegate :label, to: :profile\n def check; name_changed?; end\nend\n"),
        ("app/models/profile.rb", "class Profile < ApplicationRecord\n belongs_to :account\n def label; \"fixed\"; end\nend\n"),
    ];
    reset();
    let app = tree(&source);
    assert!(app.models.iter().find(|m| m.name.0.as_str() == "Account").unwrap()
        .methods().any(|m| m.name.as_str() == "label"));
    assert_eq!(demand::BUILDS.get(), 1);
    assert_eq!(demand::MODEL_VISITS.get(), 2);
    let scanned = uncached(|| tree(&source));
    assert_eq!(serde_json::to_value(&app.models).unwrap(), serde_json::to_value(&scanned.models).unwrap());
    reset();
    let missing = crate::lower::model_to_library::unretained_model_contracts(&app, |model| model.methods().collect());
    assert_eq!(demand::BUILDS.get(), 1);
    assert_eq!(demand::MODEL_VISITS.get(), 2);
    assert_eq!(missing, uncached(|| crate::lower::model_to_library::unretained_model_contracts(&app, |model| model.methods().collect())));
}

/// Compile the actual guard/cache implementation, not a lookalike. Small
/// stand-ins for the IR keep this a fast borrow-check rather than a second
/// build of the whole compiler. Every rejection has a compiling control.
#[test]
fn installed_indexes_keep_the_app_and_models_slice_immutably_borrowed() {
    fn between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let start = src.find(start).unwrap();
        &src[start..start + src[start..].find(end).unwrap()]
    }
    let library = include_str!("library.rs");
    let readers_and_names = between(library, "struct ConstantReaderIndex;", "/// Whether `name` is a model of this app.");
    let schema = include_str!("../../lower/model_to_library/schema.rs");
    let body_names = between(schema, "#[derive(Default)]\npub(crate) struct ModelBodyNames", "/// Every method name a model body SENDS");
    let prelude = r#"
        #![allow(dead_code)]
        use std::collections::{HashMap, HashSet, BTreeSet};
        type Symbol = String;
        #[derive(Default)] struct ClassId(String);
        #[derive(Default)] struct Model { name: ClassId }
        #[derive(Default)] struct Controller { name: ClassId }
        #[derive(Default)] struct LibraryClass {
            name: ClassId, constants: Vec<(String, ())>, unknown_calls: Vec<()>,
            class_ivar_initializers: Vec<()>,
        }
        #[derive(Default)] struct App {
            models: Vec<Model>, controllers: Vec<Controller>, library_classes: Vec<LibraryClass>,
        }
        fn walk_const_paths(_: &(), _: &mut BTreeSet<Vec<String>>) {}
        fn model_body_names(_: &[Model]) -> HashSet<Symbol> { HashSet::new() }
    "#;
    let dir = std::env::temp_dir().join(format!("lookup-borrows-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    struct Remove(std::path::PathBuf);
    impl Drop for Remove { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let _cleanup = Remove(dir.clone());
    for (i, (guard, mutation)) in [
        ("NameIndex::install(&app)", "app.library_classes.clear();"),
        ("ConstantReaderIndex::install(&app)", "app.models.clear();"),
        ("install_lookup_indexes(&app)", "app.controllers.clear();"),
        ("share_model_body_names(&app.models, &Default::default())", "app.models[0].name.0.clear();"),
    ].iter().enumerate() {
        for fail in [false, true] {
            let steps = if fail {
                format!("{mutation} drop(guard);")
            } else {
                format!("drop(guard); {mutation}")
            };
            let src = format!("{prelude}\n{readers_and_names}\n{body_names}\nfn probe() {{ let mut app = App::default(); app.models.push(Model::default()); let guard = {guard}; {steps} }}");
            let path = dir.join(format!("probe_{i}_{fail}.rs"));
            std::fs::write(&path, src).unwrap();
            let result = std::process::Command::new("rustc").args([
                "--edition=2024", "--crate-type=lib", "--emit=metadata", "--out-dir",
            ]).arg(&dir).arg(&path).output().expect("rustc from the pinned test toolchain");
            let stderr = String::from_utf8_lossy(&result.stderr);
            if fail {
                assert!(!result.status.success() && stderr.contains("E0502"), "{guard} did not reject live mutation: {stderr}");
            } else {
                assert!(result.status.success(), "positive control for {guard}: {stderr}");
            }
        }
    }
}

#[test]
fn namespace_prefix_set_reuses_original_algorithm_without_changing_shadowing() {
    let mut app = models(1, false);
    app.models[0].name = ClassId(Symbol::from("ModelSpace::Record"));
    app.library_classes = classes("module Outer::Catalog\n VALUE = 1\nend\nclass Catalog::Base\n def label; 1; end\nend\nclass Root::Leaf\n def label; 1; end\nend\n");
    app.library_classes.iter_mut().find(|c| c.name.0.as_str() == "Root::Leaf").unwrap().name = ClassId(Symbol::from("::Root::Leaf"));
    let names = ["ModelSpace", "ModelSpace::Record", "Model", "Outer", "Outer::Catalog", "Outer::Cat", "Catalog", "Catalog::Base", "Root", "Root::Leaf", "Missing"];
    let expected = names.map(|name| declares_constant_path(name, &app, &app.library_classes));
    let qualified = lexically_qualified("Catalog::Base", &["Outer"], &app, &app.library_classes);
    assert_eq!(qualified, "::Catalog::Base");
    reset();
    let _guard = install_lookup_indexes(&app);
    assert_eq!(names.map(|name| declares_constant_path(name, &app, &app.library_classes)), expected);
    assert_eq!(lexically_qualified("Catalog::Base", &["Outer"], &app, &app.library_classes), qualified);
    assert_eq!(CONSTANT_PATH_BUILDS.get(), 1);
    assert_eq!(output(emit_library_class_decls(&app)), uncached(|| output(emit_library_class_decls(&app))));
}

#[test]
fn retained_sti_index_keeps_first_parent_and_bounded_cycles() {
    let mut app = models(1, false);
    app.models[0].name = ClassId(Symbol::from("Record"));
    let mut column = app.schema.tables.values().next().unwrap().columns.iter().find(|c| c.name.as_str() == "label").unwrap().clone();
    column.name = Symbol::from("type");
    app.schema.tables.values_mut().next().unwrap().columns.push(column);
    app.library_classes = classes("class Child < Record\n def marker; end\nend\nclass Child < Missing\n def other; end\nend\nclass Grandchild < Child\n def marker; end\nend\nclass CycleA < CycleB\n def marker; end\nend\nclass CycleB < CycleA\n def marker; end\nend\n");
    let bases = crate::lower::sti_scope::sti_bases(&app);
    assert_eq!(bases.get(&ClassId(Symbol::from("Grandchild"))), Some(&ClassId(Symbol::from("Record"))));
    assert!(!bases.contains_key(&ClassId(Symbol::from("CycleA"))));
    assert!(!bases.contains_key(&ClassId(Symbol::from("CycleB"))));
    let _guard = install_lookup_indexes(&app);
    assert_eq!(crate::lower::sti_scope::sti_bases(&app), bases);
}
