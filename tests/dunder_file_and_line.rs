//! `__FILE__` / `__LINE__` — Ruby's magic constants for the current
//! source path and line number. Ingested as literal String/Int values
//! rather than left as a runtime-only construct: the path and line are
//! static facts the ingest already has (`file` is the ingest's own
//! notion of "this file", and `sources::line_at` counts newlines up to
//! the node's byte offset), and Spinel needs a literal here regardless.

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
