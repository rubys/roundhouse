require_relative "test_helper"

# Direct unit tests for `runtime/ruby/action_text.rb`.
#
# EVERY `to_plain_text` expectation below was MEASURED, not reasoned
# about: each input was run through Rails' own
# `ActionText::PlainTextConversion.node_to_plain_text` (actiontext
# 8.1.3, over a `Nokogiri::HTML5.fragment`) and the string it returned
# is what is asserted here. That is the same discipline the inflector
# tests follow — port the framework's answers, don't derive them —
# and it is what makes the surprising cases (`<h2>` is transparent,
# `<pre>` is transparent, a blockquote gets curly quotes, an empty
# `<div>` still contributes its newline) trustworthy rather than
# accidental.
#
# The one deliberate departure is attachments: the oracle above is the
# NODE converter, which renders an `<action-text-attachment>` as
# nothing. `ActionText::Content#to_plain_text` — the method this file
# implements — first runs `render_attachments(&:to_plain_text)`, so
# the caption survives. `test_attachment_renders_its_caption` asserts
# the Content-level behavior, not the node-level oracle.
# NOTE on the shape: every case spells `ActionText::Content.new(x)
# .to_plain_text` out in full rather than routing through a `plain(x)`
# helper. The helper is nicer to read and does not survive the spinel
# lane — a test-local method has no RBS, so its return widened to
# untyped and every `assert_equal` became an equality between a String
# literal and an unknown, which the strict target refuses. Spelling the
# call out keeps both lanes on the same file.
class ActionTextContentTest < Minitest::Test
  def test_plain_div_is_its_text
    assert_equal "Hello world", ActionText::Content.new("<div>Hello world</div>").to_plain_text
  end

  def test_sibling_divs_are_newline_separated
    assert_equal "Hello\nWorld", ActionText::Content.new("<div>Hello</div><div>World</div>").to_plain_text
  end

  def test_empty_div_still_contributes_its_newline
    # The case that a whole-accumulator chomp gets wrong: the middle
    # div has no text but its trailing newline is still emitted.
    assert_equal "line1\n\nline3",
      ActionText::Content.new("<div>line1</div><div></div><div>line3</div>").to_plain_text
  end

  def test_paragraphs_are_blank_line_separated
    assert_equal "one\n\ntwo", ActionText::Content.new("<p>one</p><p>two</p>").to_plain_text
  end

  def test_h1_is_a_block_but_h2_is_transparent
    assert_equal "Funny times!", ActionText::Content.new("<h1>Funny times!</h1>").to_plain_text
    # Rails aliases the block rule to `h1` and `p` ONLY.
    assert_equal "Subafter", ActionText::Content.new("<h2>Sub</h2><div>after</div>").to_plain_text
  end

  def test_pre_is_transparent
    assert_equal "code here", ActionText::Content.new("<pre>code here</pre>").to_plain_text
  end

  def test_inline_elements_are_transparent
    assert_equal "italic and bold",
      ActionText::Content.new("<em>italic</em> and <strong>bold</strong>").to_plain_text
  end

  def test_br_is_a_newline
    assert_equal "a\nb", ActionText::Content.new("<div>a<br>b</div>").to_plain_text
    assert_equal "para with \n break", ActionText::Content.new("<p>para with <br> break</p>").to_plain_text
  end

  def test_unordered_list_items_get_bullets
    assert_equal "• a\n• b", ActionText::Content.new("<ul><li>a</li><li>b</li></ul>").to_plain_text
  end

  def test_ordered_list_items_get_ordinals
    assert_equal "1. one\n2. two\n3. three",
      ActionText::Content.new("<ol><li>one</li><li>two</li><li>three</li></ol>").to_plain_text
  end

  def test_nested_lists_indent_and_break
    assert_equal "• a\n  • inner\n• b",
      ActionText::Content.new("<ul><li>a<ul><li>inner</li></ul></li><li>b</li></ul>").to_plain_text
    assert_equal "1. a\n  1. a1\n  2. a2\n2. b",
      ActionText::Content.new("<ol><li>a<ol><li>a1</li><li>a2</li></ol></li><li>b</li></ol>").to_plain_text
  end

  def test_list_inside_a_div_does_not_break_before_itself
    # `break_if_nested_list` fires only for a list inside another
    # LIST, so the "a" runs straight into the first bullet.
    assert_equal "a• x\n• y\n\nb",
      ActionText::Content.new("<div>a<ul><li>x</li><li>y</li></ul>b</div>").to_plain_text
  end

  def test_blockquote_gets_curly_quotes
    assert_equal "“quoted”", ActionText::Content.new("<blockquote>quoted</blockquote>").to_plain_text
    assert_equal "x\n“q”\n\ny",
      ActionText::Content.new("<div>x</div><blockquote>q</blockquote><div>y</div>").to_plain_text
  end

  def test_empty_blockquote_is_bare_quotes
    assert_equal "“”", ActionText::Content.new("<blockquote></blockquote>").to_plain_text
  end

  def test_blockquote_quotes_sit_inside_surrounding_space
    assert_equal "  “spaced”  ", ActionText::Content.new("<blockquote>  spaced  </blockquote>").to_plain_text
  end

  def test_figcaption_is_bracketed
    assert_equal "[cap]", ActionText::Content.new("<figcaption>cap</figcaption>").to_plain_text
  end

  def test_script_and_style_content_is_dropped
    assert_equal "safe", ActionText::Content.new("<div>safe<script>unsafe()</script></div>").to_plain_text
    assert_equal "visible", ActionText::Content.new("<style>.x{}</style><div>visible</div>").to_plain_text
  end

  def test_entities_decode
    assert_equal "Tom & Jerry", ActionText::Content.new("<div>Tom &amp; Jerry</div>").to_plain_text
    assert_equal "nbsp here", ActionText::Content.new("<div>nbsp&nbsp;here</div>").to_plain_text
    assert_equal "\"quoted\" 'single'",
      ActionText::Content.new("<div>&quot;quoted&quot; &#39;single&#39;</div>").to_plain_text
  end

  def test_escaped_markup_decodes_to_visible_text
    # Rails documents this exact pair: the return value is NOT html
    # safe, which is the whole reason `to_plain_text` is never rendered
    # without re-escaping.
    assert_equal "<script>alert()</script>",
      ActionText::Content.new("&lt;script&gt;alert()&lt;/script&gt;").to_plain_text
  end

  def test_unknown_entity_passes_through
    # Stated divergence: only the named entities Rails' own escaper
    # emits are decoded. An exotic name stays verbatim rather than
    # decoding, because decoding needs a codepoint intrinsic the
    # framework runtime does not carry.
    assert_equal "5 &lowast; 3", ActionText::Content.new("<div>5 &lowast; 3</div>").to_plain_text
  end

  def test_empty_content_is_empty
    assert_equal "", ActionText::Content.new("").to_plain_text
    assert_equal "", ActionText::Content.new("<div></div>").to_plain_text
  end

  def test_trailing_newlines_are_removed
    assert_equal "trailing", ActionText::Content.new("<div>trailing</div>\n\n").to_plain_text
  end

  def test_attachment_renders_its_caption
    html = '<div>hi <action-text-attachment sgid="abc" caption="A cap">' \
           "</action-text-attachment> there</div>"
    assert_equal "hi A cap there", ActionText::Content.new(html).to_plain_text
  end

  def test_attachment_falls_back_to_filename
    html = '<action-text-attachment sgid="abc" filename="racecar.jpg">' \
           "</action-text-attachment>"
    assert_equal "racecar.jpg", ActionText::Content.new(html).to_plain_text
  end

  def test_links_are_extracted_in_order_without_duplicates
    html = '<div><a href="http://a.example/">A</a> ' \
           '<a href="http://b.example/">B</a> ' \
           '<a href="http://a.example/">A again</a></div>'
    assert_equal ["http://a.example/", "http://b.example/"],
      ActionText::Content.new(html).links
  end

  def test_links_ignores_anchors_without_href
    assert_equal [], ActionText::Content.new("<div><a>plain</a></div>").links
  end

  def test_attachments_parse_every_attribute
    html = '<action-text-attachment sgid="SGID" content-type="image/jpeg" ' \
           'caption="Cap" filename="racecar.jpg" url="http://x/1"></action-text-attachment>'
    attachments = ActionText::Content.new(html).attachments
    assert_equal 1, attachments.length
    a = attachments[0]
    assert_equal "SGID", a.sgid
    assert_equal "image/jpeg", a.content_type
    assert_equal "Cap", a.caption
    assert_equal "racecar.jpg", a.filename
    assert_equal "http://x/1", a.url
  end

  def test_attachment_attributes_are_entity_decoded
    html = '<action-text-attachment caption="Tom &amp; Jerry"></action-text-attachment>'
    assert_equal "Tom & Jerry", ActionText::Content.new(html).attachments[0].caption
  end

  def test_attachables_is_empty_by_design
    # DIVERGENCE, pinned so it is a decision and not a surprise:
    # dereferencing an attachment's signed GlobalID back to a record
    # needs SignedGlobalID verification, which does not exist here yet.
    html = '<action-text-attachment sgid="SGID"></action-text-attachment>'
    assert_equal [], ActionText::Content.new(html).attachables
  end

  def test_to_html_and_to_s_are_the_stored_markup
    html = "<div>Hello <b>world</b></div>"
    content = ActionText::Content.new(html)
    assert_equal html, content.to_html
    assert_equal html, content.to_s
  end

  def test_blank_tracks_plain_text_not_markup
    assert ActionText::Content.new("").blank?
    assert ActionText::Content.new("<div></div>").blank?
    assert ActionText::Content.new("<div><br></div>").blank?
    refute ActionText::Content.new("<div>x</div>").blank?
    assert ActionText::Content.new("<div>x</div>").present?
  end

  def test_tag_name_is_the_canonical_attachment_element
    assert_equal "action-text-attachment", ActionText::Attachment.tag_name
  end
end
