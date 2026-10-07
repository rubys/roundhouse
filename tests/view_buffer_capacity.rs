//! Pre-sized view buffers: returning wrappers allocate with
//! `ViewBufferCap.alloc(:cap_…)` sized from the last render, then
//! `ViewBufferCap.store` the bytesize. Ports keep the same idea in
//! `Ractor[:cap_<page>]`.
//!
//! Correctness under capacity underestimate (string grows; HTML
//! unchanged) is pinned on the CRuby overlay helper; Soft Bar B is
//! untouched (helper lives outside `runtime/ruby/`).

use roundhouse::emit::{ruby, EmittedFile};
use roundhouse::ingest::ingest_app;

fn lowered_real_blog_views() -> Vec<EmittedFile> {
    let app = ingest_app(roundhouse::fixtures::real_blog()).expect("ingest real-blog");
    ruby::emit_lowered_views(&app)
}

fn find<'a>(files: &'a [EmittedFile], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

#[test]
fn returning_wrapper_allocates_via_view_buffer_cap() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    assert!(
        src.contains("def self.show("),
        "expected returning show wrapper; got:\n{src}",
    );
    assert!(
        src.contains("io = ViewBufferCap.alloc(:cap_Views_Articles_show)"),
        "expected capacity-keyed alloc; got:\n{src}",
    );
    assert!(
        src.contains("Views::Articles.show_into(io,"),
        "expected _into call after alloc; got:\n{src}",
    );
    assert!(
        src.contains("ViewBufferCap.store(:cap_Views_Articles_show, io.bytesize)"),
        "expected store of bytesize before return; got:\n{src}",
    );
}

#[test]
fn into_variant_does_not_allocate_its_own_buffer() {
    let files = lowered_real_blog_views();
    let src = find(&files, "app/views/articles/show.rb");
    let into = src
        .split("def self.show_into(")
        .nth(1)
        .expect("show_into variant")
        .split("def self.")
        .next()
        .unwrap();
    assert!(
        !into.contains("ViewBufferCap.alloc"),
        "_into must append into the caller's buffer, not alloc; got:\n{into}",
    );
    assert!(
        !into.contains("ViewBufferCap.store"),
        "_into does not own the page-size memo; got:\n{into}",
    );
}

#[test]
fn overlay_cap_underestimate_still_yields_full_string() {
    // Load the CRuby overlay helper directly — capacity is a hint;
    // growing past it must not truncate or raise.
    let overlay = include_str!(
        "../runtime/spinel/scaffold/ruby_overlay/runtime/view_buffer_cap.rb"
    );
    let status = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!(
            r#"{overlay}
key = :cap_test_underestimate
ViewBufferCap.store(key, 4)
io = ViewBufferCap.alloc(key)
io << "hello world"
ViewBufferCap.store(key, io.bytesize)
abort("truncated") unless io == "hello world"
abort("memo") unless Thread.current.thread_variable_get(key) == "hello world".bytesize
puts "ok"
"#
        ))
        .output()
        .expect("run overlay capacity check");
    assert!(
        status.status.success(),
        "stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&status.stderr),
        String::from_utf8_lossy(&status.stdout),
    );
}

#[test]
fn spinel_stub_alloc_is_plain_string_new() {
    let stub = include_str!("../runtime/spinel/view_buffer_cap.rb");
    let status = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!(
            r#"{stub}
io = ViewBufferCap.alloc(:cap_anything)
io << "x"
ViewBufferCap.store(:cap_anything, io.bytesize)
abort("body") unless io == "x"
puts "ok"
"#
        ))
        .output()
        .expect("run spinel stub check");
    assert!(
        status.status.success(),
        "stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&status.stderr),
        String::from_utf8_lossy(&status.stdout),
    );
}
