//! `__FILE__` / `__LINE__` — Ruby's magic constants for the current
//! source path and line number. Ingested as literal String/Int values
//! rather than left as a runtime-only construct: the path and line are
//! static facts the ingest already has (`file` is the ingest's own
//! notion of "this file", and the parse's line index covers the node's
//! byte offset), and Spinel needs a literal here regardless.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::expr::{ExprNode, Literal};
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// `__FILE__` reads as a String literal of the file's own path, and
/// `__LINE__` as an Int literal of its own 1-based line number — both
/// computed at ingest time, not left for a runtime to resolve.
#[test]
fn file_and_line_ingest_as_literals() {
    let app = ingest_app_from_tree(tree(&[(
        "app/services/file_probe.rb",
        "class FileProbe\n  FILE = __FILE__\n  LINE = __LINE__\nend\n",
    )]))
    .expect("ingest");
    let class = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "FileProbe")
        .expect("FileProbe class");
    let file_const = class
        .constants
        .iter()
        .find(|(name, _)| name.as_str() == "FILE")
        .map(|(_, e)| e)
        .expect("FILE constant");
    match &*file_const.node {
        ExprNode::Lit { value: Literal::Str { value } } => {
            assert_eq!(value, "app/services/file_probe.rb");
        }
        other => panic!("expected a String literal for __FILE__, got {other:?}"),
    }
    let line_const = class
        .constants
        .iter()
        .find(|(name, _)| name.as_str() == "LINE")
        .map(|(_, e)| e)
        .expect("LINE constant");
    match &*line_const.node {
        ExprNode::Lit { value: Literal::Int { value } } => {
            // `LINE = __LINE__` is the third line of the source.
            assert_eq!(*value, 3);
        }
        other => panic!("expected an Int literal for __LINE__, got {other:?}"),
    }
}

#[test]
fn template_line_literals_use_template_not_compiled_ruby_offsets() {
    let source = "<p>é and a long prefix that shifts compiled offsets</p>\n<%= __LINE__ %>\n\n<% value = __LINE__ %>\n<%= value %>";
    let app = ingest_app_from_tree(tree(&[("app/views/probes/show.html.erb", source)]))
        .expect("template ingests");
    fn collect(expr: &roundhouse::expr::Expr, lines: &mut Vec<i64>) {
        if let ExprNode::Lit { value: Literal::Int { value } } = &*expr.node {
            lines.push(*value);
        }
        expr.node.for_each_child(&mut |child| collect(child, lines));
    }
    let mut lines = Vec::new();
    collect(&app.views[0].body, &mut lines);
    assert_eq!(lines, [2, 4]);
}

#[test]
fn file_literals_are_relative_to_the_real_root_without_changing_source_ids() {
    use roundhouse::ingest::ingest_app_with_vfs;
    use roundhouse::vfs::MapVfs;
    for root in ["/tmp/myapp", "/tmp/lib/repo", "relative/myapp"] {
        let root = PathBuf::from(root);
        let path = root.join("app/services/file_probe.rb");
        let vfs = MapVfs::new([(path.clone(), b"class FileProbe\n FILE = __FILE__\nend\n".to_vec())].into());
        let app = ingest_app_with_vfs(&vfs, &root).expect("ingest at an explicit root");
        let class = app.library_classes.iter().find(|c| c.name.0.as_str() == "FileProbe").unwrap();
        assert!(matches!(&*class.constants[0].1.node, ExprNode::Lit { value: Literal::Str { value } } if value == "app/services/file_probe.rb"));
        assert!(app.sources.iter().any(|f| f.path == path.to_string_lossy()));
    }
}

#[test]
fn repeated_standalone_ingest_uses_the_current_parse_for_line_literals() {
    use roundhouse::ingest::{ingest_library_classes, sources};
    sources::reset();
    let first = "class Probe\n LINE = __LINE__\nend\n";
    for (source, expected) in [
        (first, 2),
        ("# é and a long prefix shifts offsets beyond the first source\n\n\nclass Probe\n LINE = __LINE__\nend\n", 5),
        ("class Probe; LINE = __LINE__; end", 1),
    ] {
        let classes = ingest_library_classes(source.as_bytes(), "probe.rb").unwrap();
        assert!(matches!(&*classes[0].constants[0].1.node,
            ExprNode::Lit { value: Literal::Int { value } } if *value == expected), "{source}");
    }
    // Preserve the registry's first-text-wins contract for issued spans.
    let registered = sources::drain();
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].text, first);
}

#[test]
fn failed_app_ingest_does_not_relocate_a_later_standalone_file_literal() {
    use roundhouse::ingest::{ingest_app_with_vfs, ingest_library_classes, sources};
    use roundhouse::vfs::MapVfs;
    let root = PathBuf::from("/tmp/failed_app");
    let vfs = MapVfs::new([(root.join("roundhouse.yml"),
        b"test_paths: [../outside]".to_vec())].into());
    let error = ingest_app_with_vfs(&vfs, &root).expect_err("invalid app-relative test root");
    assert!(error.to_string().contains("test_paths"));
    let path = root.join("app/services/fresh.rb");
    let file = path.to_string_lossy();
    let classes = ingest_library_classes(b"class Fresh; FILE=__FILE__; end", &file).unwrap();
    assert!(matches!(&*classes[0].constants[0].1.node,
        ExprNode::Lit { value: Literal::Str { value } } if value == file.as_ref()));
    sources::drain();
}

#[test]
fn roda_file_literals_keep_the_registered_root_during_route_ingest() {
    use roundhouse::ingest::ingest_app_with_vfs;
    use roundhouse::vfs::MapVfs;
    let root = PathBuf::from("/tmp/roda");
    let vfs = MapVfs::new([
        (root.join("config.ru"), b"run App.app".to_vec()),
        (root.join("app.rb"), b"class App < Roda\n route do |r|\n r.root { __FILE__ }\n end\nend\n".to_vec()),
    ].into());
    let app = ingest_app_with_vfs(&vfs, &root).expect("detected Roda app ingests");
    fn collect(expr: &roundhouse::expr::Expr, paths: &mut Vec<String>) {
        if let ExprNode::Lit { value: Literal::Str { value } } = &*expr.node
            && value.ends_with("app.rb") {
            paths.push(value.clone());
        }
        expr.node.for_each_child(&mut |child| collect(child, paths));
    }
    let mut paths = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() { collect(&action.body, &mut paths); }
    }
    assert_eq!(paths, ["app.rb"]);
    assert!(app.sources.iter().any(|source| source.path == "/tmp/roda/app.rb"));
}
