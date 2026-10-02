//! An ERB code tag can open more than one block.
//!
//! Shopify's internal diagnostics views iterate and filter in a single
//! tag:
//!
//! ```erb
//! <%= section.row do |row| %>
//!   <% table_sections.values.each do |content|
//!        unless content.nil? %>
//!     <%= row.cell(content) %>
//!   <% end %>
//! <% end %>
//! <% end %>
//! ```
//!
//! The tag opens two blocks (`each do`, `unless`), so it takes two
//! `<% end %>` to close them. The compiler recognised only the block
//! its head line named, so the first `<% end %>` closed the enclosing
//! OUTPUT block instead, emitting `end).to_s` in the wrong place. The
//! compiled Ruby no longer parsed, and Prism's `MissingNode` made the
//! whole template — every helper call in it — unresolvable.

use roundhouse::erb::compile_erb;

fn parse_errors(ruby: &str) -> Vec<String> {
    ruby_prism::parse(ruby.as_bytes()).errors().map(|e| e.message().to_string()).collect()
}

#[test]
fn a_tag_opening_two_blocks_is_closed_by_two_ends() {
    let compiled = compile_erb(
        r#"<%= section.row do |row| %>
  <% table_sections.values.each do |content|
       unless content.nil? %>
    <%= row.cell(content) %>
  <% end %>
  <% end %>
<% end %>
"#,
    );
    let errors = parse_errors(&compiled);
    assert!(errors.is_empty(), "{errors:?}\n{compiled}");
}

#[test]
fn a_single_line_opener_is_unchanged() {
    let compiled = compile_erb(
        "<%= a.b do |t| %>\n<% xs.each do |x| %>\n<%= x %>\n<% end %>\n<% end %>\n",
    );
    let errors = parse_errors(&compiled);
    assert!(errors.is_empty(), "{errors:?}\n{compiled}");
}

/// A multi-line tag that opens nothing must not swallow an `end`.
#[test]
fn a_multi_line_tag_that_opens_nothing_pushes_nothing() {
    let compiled = compile_erb(
        "<%= a.b do |t| %>\n<% ts = {\n  \"a\" => 1,\n} %>\n<% end %>\n",
    );
    let errors = parse_errors(&compiled);
    assert!(errors.is_empty(), "{errors:?}\n{compiled}");
}

/// `<% end # card.section %>`: a trailing Ruby comment on a tag. It must
/// neither hide the `end` from the block stack (leaving the enclosing
/// output block unclosed) nor swallow the `).to_s` an output tag closes
/// with. `"#{…}"` is interpolation, not a comment.
#[test]
fn a_trailing_comment_on_a_tag_is_not_code() {
    let compiled = compile_erb(
        "<%= ui_card do |card| %>\n  <%= card.section do %>\n    <%= \"n: #{1}\" # shown %>\n  <% end # card.section %>\n<% end # ui_card %>\n",
    );
    let errors = parse_errors(&compiled);
    assert!(errors.is_empty(), "{errors:?}\n{compiled}");
    assert!(compiled.contains("\"n: #{1}\""), "{compiled}");
}
