//! `ActiveStorage::AttachedMany`'s Enumerable reads: `map`, `size`,
//! `empty?`.
//!
//! Rails' `has_many_attached` reader hands every Enumerable call to its
//! `attachments` (`delegate_missing_to`). The runtime proxy carried
//! `each` and `first` only, so a jbuilder
//! `json.pictures(widget.pictures) do |picture| … end`, which maps the
//! proxy, raised `undefined method 'map' for an instance of
//! ActiveStorage::AttachedMany`.
//!
//! Exercised on CRuby against the runtime file itself, with rows
//! installed the way the batch preloader installs them (no database).

use std::path::Path;
use std::process::Command;

#[test]
fn the_proxy_answers_map_size_and_empty() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/active_storage.rb");
    let script = format!(
        r#"require {runtime:?}
many = ActiveStorage::AttachedMany.new("Widget", 1, "pictures")
many._preload_rows([ActiveStorage::ManyAttachment.new(3, nil), ActiveStorage::ManyAttachment.new(5, nil)])
none = ActiveStorage::AttachedMany.new("Widget", 2, "pictures")
none._preload_rows([])
p [many.map {{ |a| a.id * 10 }}, many.size, many.empty?, none.map(&:id), none.size, none.empty?]
"#,
        runtime = runtime.display().to_string()
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[[30, 50], 2, false, [], 0, true]");
}
