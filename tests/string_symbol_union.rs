use roundhouse::emit::{EmittedFile, go, kotlin, rust};
use roundhouse::ty::Ty;

fn file(files: &[EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("missing {suffix}"))
        .content
        .clone()
}

#[test]
fn base_index_writers_keep_the_models_string_parameter_contract() {
    let app =
        roundhouse::ingest::ingest_app(std::path::Path::new("fixtures/tiny-blog")).expect("ingest");
    let rust = file(&rust::emit(&app), "active_record_base.rs");
    assert!(
        rust.contains("pub fn set_index(&self, _name: &str,"),
        "{rust}"
    );
    let kotlin = file(&kotlin::emit(&app), "ActiveRecordBase.kt");
    assert!(kotlin.contains("fun set(_name: String,"), "{kotlin}");
    let go = file(&go::emit_overlay_files(&app), "active_record_base.go");
    assert!(go.contains("OpSet(_name string,"), "{go}");
}

#[test]
fn nullable_and_mixed_unions_are_not_nonnullable_strings() {
    assert!(
        !Ty::Union {
            variants: vec![Ty::Str, Ty::Sym, Ty::Nil]
        }
        .is_stringish()
    );
    assert!(
        !Ty::Union {
            variants: vec![Ty::Str, Ty::Int]
        }
        .is_stringish()
    );
}
