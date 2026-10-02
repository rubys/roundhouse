//! A nested request-params read runs on the ruby-family targets and is refused, by name, on a strict one.

use std::path::Path;

use roundhouse::diagnostic::{DiagnosticKind, Severity};
use roundhouse::ingest::ingest_app;
use roundhouse::lower::params_residue::elevate_nested_params_for_target;
use roundhouse::project::BuildTarget;

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if ["tmp", "log", "storage", "node_modules", ".git"].contains(&name.to_string_lossy().as_ref()) {
                continue;
            }
            copy_tree(&entry.path(), &dst.join(name));
        }
    } else {
        std::fs::copy(src, dst).unwrap();
    }
}

#[test]
fn nested_params_are_ledgered_and_refused_by_a_strict_emit() {
    let dir = std::env::temp_dir().join(format!("roundhouse-params-residue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(roundhouse::fixtures::real_blog(), &dir);
    let controller = dir.join("app/controllers/articles_controller.rb");
    let source = std::fs::read_to_string(&controller).unwrap().replacen(
        "    @article = Article.new(article_params)\n",
        "    @article = Article.new(article_params)\n    @article.title = params[:extra][:suffix].to_s if params[:extra].key?(:suffix)\n",
        1,
    );
    std::fs::write(&controller, source).unwrap();

    let mut app = ingest_app(&dir).unwrap();
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    let ledgered = |d: &roundhouse::diagnostic::Diagnostic| {
        matches!(&d.kind, DiagnosticKind::LowerResidue { pass, .. } if pass.as_str() == "params_residue")
    };
    assert_eq!(diags.iter().filter(|d| ledgered(d)).count(), 2, "{diags:?}");

    let mut ruby = diags.clone();
    elevate_nested_params_for_target(&mut ruby, BuildTarget::Ruby);
    assert!(ruby.iter().filter(|d| ledgered(d)).all(|d| d.severity != Severity::Error));

    let mut rust = diags;
    elevate_nested_params_for_target(&mut rust, BuildTarget::Rust);
    assert!(rust.iter().filter(|d| ledgered(d)).all(|d| d.severity == Severity::Error));
    let _ = std::fs::remove_dir_all(&dir);
}
