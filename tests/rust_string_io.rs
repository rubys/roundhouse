#[path = "../runtime/rust/string_io.rs"]
mod string_io;

use string_io::StringIO;

#[test]
fn appends_chunks_in_order_and_returns_the_complete_string() {
    let mut body = StringIO::new();
    body.append("first ").append("second");

    let value = body.string();
    assert_eq!(value, "first second");
    assert_eq!(value.as_bytes().len(), 12);
}

#[test]
fn string_io_starts_empty_and_preserves_utf8_contents() {
    let mut body = StringIO::new();
    assert_eq!(body.string(), "");

    body.append("雪").append("☕");
    assert_eq!(body.string(), "雪☕");
    assert_eq!(body.string().as_bytes().len(), 6);
}
