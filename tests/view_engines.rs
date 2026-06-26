//! Engine-parameterized view-ingest harness.
//!
//! The shared gate for the text-template engine family (ERB today; HAML
//! and herb as they land). Every engine compiles its template text to the
//! same `_buf`-append Ruby shape + segment map and flows through the one
//! `ingest_template` pipeline, so the invariants below are engine-agnostic
//! and parameterized over the engine table from day one: adding HAML/herb
//! is a new `Case` row, not a new harness.
//!
//! What this covers now (in-process, no toolchain): the compile contract
//! (the `_buf` prologue/epilogue shape) and the ingest contract (name/
//! format parsing + spans translated into template coordinates).
//!
//! What it deliberately does NOT cover yet: the byte-identical HTML
//! differential against each engine's *reference* renderer (erubi /
//! `haml` gem / herb). That needs emit + a CRuby render and is only
//! meaningful once a second engine exists to disagree, so it attaches
//! per-engine in the #59 (HAML) / #60 (herb) conformance step — reusing
//! this same `Case` table as its fixtures.

use roundhouse::Expr;
use roundhouse::ingest::view::{ViewEngine, ingest_template};

struct Case {
    engine: ViewEngine,
    /// View path relative to `app/views`.
    rel_path: &'static str,
    /// A small template exercising static text + a dynamic output tag.
    source: &'static str,
    expect_name: &'static str,
    expect_format: &'static str,
}

/// The engine table. HAML/herb rows drop in here once their
/// `compile_*_mapped` fns exist and `ViewEngine` carries the variant.
fn cases() -> Vec<Case> {
    vec![
        Case {
            engine: ViewEngine::Erb,
            rel_path: "posts/index.html.erb",
            source: "<h1><%= title %></h1>\n",
            expect_name: "posts/index",
            expect_format: "html",
        },
        Case {
            engine: ViewEngine::Haml,
            rel_path: "posts/show.html.haml",
            source: "%h1= @post.title\n.body\n  = @post.body\n",
            expect_name: "posts/show",
            expect_format: "html",
        },
    ]
}

fn for_each(e: &mut Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    e.node.for_each_child_mut(&mut |c| for_each(c, f));
}

#[test]
fn compile_contract_buf_shape() {
    for c in cases() {
        let (ruby, map) = (c.engine.compile_fn())(c.source);
        assert!(
            ruby.starts_with("_buf = \"\""),
            "{:?}: compiled Ruby must open with the `_buf` prologue, got:\n{ruby}",
            c.engine,
        );
        assert_eq!(
            ruby.trim_end().lines().last(),
            Some("_buf"),
            "{:?}: compiled Ruby must end with the `_buf` value expression",
            c.engine,
        );
        assert!(
            !map.is_empty(),
            "{:?}: a non-trivial template must produce ≥1 segment",
            c.engine,
        );
        // Segments stay within the template's bounds in template space.
        for seg in &map {
            assert!(
                seg.e_start <= seg.e_end && seg.e_end <= c.source.len() as u32,
                "{:?}: segment {}..{} outside template bounds 0..{}",
                c.engine,
                seg.e_start,
                seg.e_end,
                c.source.len(),
            );
        }
    }
}

#[test]
fn ingest_contract_name_format_and_template_spans() {
    for c in cases() {
        let file = format!("app/views/{}", c.rel_path);
        let view = ingest_template(
            c.source,
            std::path::Path::new(c.rel_path),
            &file,
            c.engine.compile_fn(),
        )
        .unwrap_or_else(|e| panic!("{:?}: ingest failed: {e}", c.engine));

        assert_eq!(view.name.as_str(), c.expect_name, "{:?}: name", c.engine);
        assert_eq!(
            view.format.as_str(),
            c.expect_format,
            "{:?}: format",
            c.engine,
        );

        // Every real span was translated into template coordinates.
        let mut body = view.body.clone();
        let mut saw_real = false;
        for_each(&mut body, &mut |e| {
            if !e.span.is_synthetic() {
                saw_real = true;
                assert!(
                    e.span.start <= e.span.end && e.span.end <= c.source.len() as u32,
                    "{:?}: span {}..{} outside template bounds 0..{}",
                    c.engine,
                    e.span.start,
                    e.span.end,
                    c.source.len(),
                );
            }
        });
        assert!(saw_real, "{:?}: expected template-coordinate spans", c.engine);
    }
}
