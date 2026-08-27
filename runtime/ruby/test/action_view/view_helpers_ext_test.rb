require_relative "../test_helper"

# Direct unit tests for the ruby-family ViewHelpers extensions
# (`runtime/ruby/action_view/view_helpers_ext.rb`).
#
# Ruby-family lanes only, for the same reason the file itself is: it is
# outside the strict-target runtime tables, so there is no
# crystal/kotlin/swift/typescript sibling to run this against. That is
# also why these expectations cannot live in `view_helpers_test.rb`
# beside them — that file IS run by those lanes.
#
# Every expectation here was measured against Rails 8.1, not derived
# from the implementation.
class ViewHelpersExtTest < Minitest::Test
  include ActionView

  # The plain form. Note the closing ` />`: `hidden_field_tag` goes
  # through Rails' legacy `tag()` spelling, unlike `image_tag` beside it
  # in the runtime, which closes `>`. The id is the NAME sanitized —
  # `boost[content]` loses its `]` and its `[` becomes `_`.
  def test_hidden_field_tag_derives_its_id_from_the_name
    assert_equal %(<input type="hidden" name="boost[content]" id="boost_content" value="x" />),
                 ViewHelpers.hidden_field_tag("boost[content]", "x")
  end

  # A Symbol name is as ordinary as a String one — campfire writes both,
  # which is why the parameter cannot be declared `String`.
  def test_hidden_field_tag_accepts_a_symbol_name
    assert_equal %(<input type="hidden" name="push_endpoint" id="push_endpoint" />),
                 ViewHelpers.hidden_field_tag(:push_endpoint, nil)
  end

  # Rails omits `value` ENTIRELY when it is nil rather than rendering it
  # empty — the difference an `unless value.nil?` buys, and a detail no
  # amount of reading the API docs supplies.
  def test_hidden_field_tag_omits_a_nil_value
    out = ViewHelpers.hidden_field_tag("k", nil)
    assert_equal %(<input type="hidden" name="k" id="k" />), out
  end

  # `id: nil` SUPPRESSES the derived id. It works because the option
  # overwrites the derived entry with nil and `render_attrs` drops nil
  # values — which is how Rails suppresses it too. campfire's
  # `hidden_field_tag "user_ids[]", user.id, id: nil` is the call site;
  # the overlay implementation this replaced emitted the id as a literal
  # and could not honour it.
  def test_hidden_field_tag_id_nil_suppresses_the_id
    assert_equal %(<input type="hidden" name="user_ids[]" value="7" />),
                 ViewHelpers.hidden_field_tag("user_ids[]", 7, id: nil)
  end

  # A nested `data:` hash expands to `data-*`, underscores becoming
  # dashes — `render_attrs`' existing contract, exercised here because
  # this helper is the one campfire reaches it through.
  def test_hidden_field_tag_expands_a_data_hash
    assert_equal %(<input type="hidden" name="k" id="k" data-sessions-target="pushEndpoint" />),
                 ViewHelpers.hidden_field_tag("k", nil, data: { sessions_target: "pushEndpoint" })
  end

  # The value is escaped, like every other attribute render_attrs emits.
  def test_hidden_field_tag_escapes_its_value
    assert_equal %(<input type="hidden" name="k" id="k" value="&lt;b&gt;" />),
                 ViewHelpers.hidden_field_tag("k", "<b>")
  end

  # `sanitize_to_id`'s own contract, pinned here because
  # `hidden_field_tag` is now its first in-tree caller.
  def test_sanitize_to_id_drops_brackets_and_replaces_the_rest
    assert_equal "tags_foo", ViewHelpers.sanitize_to_id("tags[foo]")
    assert_equal "a-b:c.d", ViewHelpers.sanitize_to_id("a-b:c.d")
    assert_equal "a_b", ViewHelpers.sanitize_to_id("a b")
  end
  # `capture` answers the block's own value — Rails' `buffer.presence ||
  # value`, and the only half an emitted block can reach (a `concat`
  # that would have filled the buffer is inlined into an append by
  # `capture_inline` long before this runs).
  def test_capture_answers_the_blocks_value
    assert_equal "<b>hi</b>", ViewHelpers.capture { "<b>hi</b>" }
  end

  # A non-String block value is NOT the capture: Rails answers the empty
  # buffer there rather than stringifying whatever the block ended on.
  def test_capture_answers_empty_for_a_non_string_value
    assert_equal "", ViewHelpers.capture { 42 }
  end

  # Loud beats silently dropped output: emitted views buffer through
  # `io <<`, so a concat has nowhere to append.
  def test_concat_raises
    err = assert_raises(RuntimeError) { ViewHelpers.concat("x") }
    assert_match(/concat outside capture/, err.message)
  end
end
