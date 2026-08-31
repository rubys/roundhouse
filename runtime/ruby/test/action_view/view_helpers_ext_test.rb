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

  # ── the safe-list sanitizer ─────────────────────────────────────────
  #
  # The full gem-parity corpus lives in `tests/shared_sanitize.rb`
  # (CRuby, measured against rails-html-sanitizer 1.7.1). What runs
  # HERE is the strict-lane pricing set: one probe per code path, so a
  # typer or codegen regression in any branch fails a test that spinel
  # actually executes. Every expectation below is the gem's own answer
  # unless the comment says otherwise.

  # The demo path: what Trix posts is what must survive.
  def test_sanitize_allowing_keeps_a_trix_body
    assert_equal "<div>hello <strong>bold</strong><br>next</div>",
                 ViewHelpers.sanitize_allowing(
                   "<div>hello <strong>bold</strong><br>next</div>",
                   ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # Attribute scrub: allowed names stay in source order, the rest go,
  # names are downcased, values re-quoted double.
  def test_sanitize_allowing_scrubs_attributes
    assert_equal %(<div class="a b">t</div>),
                 ViewHelpers.sanitize_allowing(
                   %(<div class="a b" id="x" onclick="alert(1)">t</div>),
                   ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal %(<a href="HTTPS://X.CO" class="c">up</a>),
                 ViewHelpers.sanitize_allowing(
                   %(<a HREF="HTTPS://X.CO" CLASS='c'>up</a>),
                   ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # The protocol check, in its plain and entity-encoded spellings. The
  # encoded colon and encoded scheme letter are the attack forms the
  # two-pass decode exists for.
  def test_sanitize_allowing_drops_unsafe_protocols
    assert_equal "<a>j</a>",
                 ViewHelpers.sanitize_allowing(
                   %(<a href="javascript:alert(1)">j</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "<a>j</a>",
                 ViewHelpers.sanitize_allowing(
                   %(<a href="javascript&#58alert(1)">j</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "<a>j</a>",
                 ViewHelpers.sanitize_allowing(
                   %(<a href="&#106;avascript:x">j</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "<a>j</a>",
                 ViewHelpers.sanitize_allowing(
                   %(<a href=" javascript:alert(1)">j</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  def test_sanitize_allowing_keeps_safe_urls
    assert_equal %(<a href="https://example.com/x">l</a>),
                 ViewHelpers.sanitize_allowing(
                   %(<a href="https://example.com/x">l</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal %(<a href="/relative#frag">l</a>),
                 ViewHelpers.sanitize_allowing(
                   %(<a href="/relative#frag">l</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal %(<a href="mailto:x@y.z">l</a>),
                 ViewHelpers.sanitize_allowing(
                   %(<a href="mailto:x@y.z">l</a>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # A disallowed ordinary element strips to its children; a rawtext
  # container's content is character data and comes out escaped.
  def test_sanitize_allowing_strips_disallowed_elements
    assert_equal "go",
                 ViewHelpers.sanitize_allowing(
                   %(<form action="/x"><button>go</button></form>), ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "alert(1)",
                 ViewHelpers.sanitize_allowing(
                   "<script>alert(1)</script>", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "&lt;b&gt;hi&lt;/b&gt;",
                 ViewHelpers.sanitize_allowing(
                   "<script><b>hi</b></script>", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # Foreign content is pruned whole — children, text and all.
  def test_sanitize_allowing_prunes_foreign_content
    assert_equal "tail",
                 ViewHelpers.sanitize_allowing(
                   "<svg><script>alert(1)</script></svg>tail", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # The open stack: unclosed tags are closed at end of input, a close
  # nothing opened is dropped — both the gem's own answers.
  def test_sanitize_allowing_balances_tags
    assert_equal "<b>unclosed</b>",
                 ViewHelpers.sanitize_allowing("<b>unclosed", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "stray close",
                 ViewHelpers.sanitize_allowing("</b>stray close", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # Entity POLICY divergence, deliberate and ledgered: well-formed
  # references stay as written (the gem decodes; identical rendering),
  # a bare `&` is escaped as the gem escapes it.
  def test_sanitize_allowing_entity_policy
    assert_equal "caf&eacute; <b>bold</b>",
                 ViewHelpers.sanitize_allowing(
                   "caf&eacute; <b>bold</b>", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
    assert_equal "a &amp; b",
                 ViewHelpers.sanitize_allowing("a & b", ViewHelpers.sanitize_default_tags, ViewHelpers.sanitize_default_attributes)
  end

  # `sanitize` is the same engine on the gem's default tables — `img`
  # is in those tables and campfire's list excludes it, which is the
  # observable difference.
  def test_sanitize_uses_the_default_tables
    assert_equal %(<img src="x.png">),
                 ViewHelpers.sanitize(%(<img src="x.png" onerror="alert(1)">))
    assert_equal "tagless stays", ViewHelpers.sanitize("tagless stays")
  end

  # The honest boundary that remains: an allow-list naming a rawtext or
  # foreign container is refused, as is the style attribute.
  def test_sanitize_allowing_refuses_unservable_lists
    assert_raises(NotImplementedError) do
      ViewHelpers.sanitize_allowing("<p>x</p>", ["p", "script"], ["class"])
    end
    assert_raises(NotImplementedError) do
      ViewHelpers.sanitize_allowing("<p>x</p>", ["p"], ["class", "style"])
    end
  end
end
