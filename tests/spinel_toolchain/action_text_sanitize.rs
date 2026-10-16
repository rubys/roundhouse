//! The Campfire sanitizer's AOT path: Array#to_set and Set#include? select
//! allowed names, then Fragment#update removes disallowed nodes and their
//! descendants while retaining allowed nodes around and inside the tree.

use super::emit_and_run;
use roundhouse::project::BuildTarget;

const SANITIZER: &str = r#"class ActionTextSanitizer
  ALLOWED_TAG_NAMES = %w[ a div p span ].to_set

  def self.apply(fragment)
    fragment.update do |source|
      source.css("*").each do |node|
        node.remove unless ALLOWED_TAG_NAMES.include?(node.name)
      end
    end
  end
end
"#;

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/action_text_sanitizer.rb", SANITIZER)
}

#[test]
fn sanitizer_is_accepted_by_the_spinel_analyzer() {
    let (_emitted, errors) = overlay().emit(BuildTarget::Spinel);
    assert!(
        errors.is_empty(),
        "Spinel analysis/emission errors: {errors:#?}"
    );
}

#[test]
#[ignore = "requires the Spinel toolchain, run in its CI lane"]
fn sanitizer_removes_forbidden_nodes_and_their_contents_natively() {
    let run = overlay().run_spinel(
            r#"
require_relative "app/models/action_text_sanitizer"
input = "<p>keep</p><svg><script>nested</script><a href=\"https://example.com\">gone</a></svg><iframe>frame</iframe><div><script>alert(1)</script>ok<span>stay</span></div>"
fragment = ActionText::Content.new(input, canonicalize: false).fragment
clean = ActionTextSanitizer.apply(fragment)
raise "sanitized HTML: #{clean}" unless clean.to_s == "<p>keep</p><div>ok<span>stay</span></div>"
raise "update mutated its receiver: #{fragment}" unless fragment.to_s == input
puts "ActionText sanitizer contract passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("ActionText sanitizer contract passed"));
}
