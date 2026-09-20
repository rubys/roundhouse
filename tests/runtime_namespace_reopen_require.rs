//! A library class declared INSIDE a namespace the runtime owns —
//! lobsters' `extras/sponge.rb` opens `module Net; class HTTP` to add
//! `attr_accessor`s and a lazy `#start` — emits the compound header the
//! source wrote (`class Net::HTTP`), because nesting would have to
//! guess `class` or `module` for the outer segment and the wrong guess
//! is a TypeError at load. A compound header looks the outer constant
//! UP rather than creating it, and nothing else loads
//! `runtime/net_http` first: boot.rb carries no line for it, and the
//! models aggregator reaches this file before any body-constant anchor
//! does. So the file requires the runtime file its own header depends
//! on. Without it the ruby lane's boot died at
//! `app/models/net/http.rb:1: uninitialized constant Net (NameError)`
//! and the lobsters ruby lanes were red for ten days.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_library;
use roundhouse::ingest::ingest_app_from_tree;

const SPONGE: &str = r#"require "net/https"

module Net
  class HTTP
    attr_accessor :skip_close

    def start
      do_start
      self
    end
  end
end

class Sponge
  def fetch(host)
    Net::HTTP.new(host)
  end
end
"#;

#[test]
fn a_reopen_inside_a_runtime_namespace_requires_the_runtime_file_its_header_reads() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("extras/sponge.rb"), SPONGE.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = emit_library(&app);
    let http = files
        .iter()
        .find(|f| f.path.ends_with("net/http.rb"))
        .unwrap_or_else(|| {
            panic!(
                "no net/http.rb among {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        });
    let src = &http.content;
    assert!(src.contains("\nclass Net::HTTP\n"), "compound header:\n{src}");
    // The require comes BEFORE the header — a `require_relative` after
    // `class Net::HTTP` is too late to define `Net`.
    let require_at = src
        .find("require_relative \"../../../runtime/net_http\"")
        .unwrap_or_else(|| panic!("no runtime/net_http require:\n{src}"));
    let header_at = src.find("class Net::HTTP").unwrap();
    assert!(require_at < header_at, "{src}");
}
