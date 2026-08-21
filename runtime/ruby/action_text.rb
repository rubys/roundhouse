# Action Text's value layer: the HTML fragment a `has_rich_text`
# attribute holds, and the attachment nodes embedded in it.
#
# `ActionText::RichText` is NOT here — it is a table-backed model
# (`action_text_rich_texts`), so roundhouse synthesizes it through the
# ordinary model lowering (`src/lower/rich_text.rs`) and every target
# gets columns, `where`, hydration and persistence for free. What that
# model cannot get from the schema is the `serialize :body, coder:
# ActionText::Content` half — the column stores HTML text, the
# attribute reads back as this object. So this file is exactly the
# coder, and nothing else.
#
# WHY A HAND-ROLLED SCANNER. Rails builds a Nokogiri document and walks
# it bottom-up (`ActionText::PlainTextConversion`). There is no
# Nokogiri here and no regex engine in the framework runtime, so
# `to_plain_text` is a single left-to-right pass with a small tag
# stack. The rules below are transcribed from
# actiontext/lib/action_text/plain_text_conversion.rb rather than
# derived — same reason the inflector ports Rails' tables instead of
# guessing them.
#
# WHAT IS DELIBERATELY MISSING: `attachables`. An attachment node
# carries a SIGNED GlobalID (`sgid`), and turning that back into a
# record needs SignedGlobalID verification plus a GlobalID URI parse
# plus a registry lookup — a body of work of its own, and none of it
# is expressible over the attributes this scanner already has.
# `attachments` therefore returns the PARSED nodes (complete: every
# attribute the markup carried), and `attachables` returns `[]` with
# the divergence stated at its definition and ledgered in
# docs/pipeline/runtime.md.
module ActionText
  # The marker a model mixes in to say "I can be attached to rich text"
  # (campfire's `User::Mentionable` is one). EMPTY, and that is the
  # whole of it here: in Rails the module also supplies the sgid round
  # trip (`attachable_sgid`, `from_node`) and partial-path defaults,
  # and an app that includes it overrides the partial paths itself —
  # which is the half that is actually reached, since nothing here
  # dereferences an sgid (see `Content#attachables`). Declared so the
  # `include` resolves at load time rather than taking the app down
  # before it serves a request.
  module Attachable
  end

  # The signed GlobalID an `<action-text-attachment>` node carries.
  #
  # Rails signs a `gid://<app>/<Model>/<id>` URI with
  # `SignedGlobalID`; the wire format here is `<Model>/<id>` through
  # `MessageVerifier` — the same envelope signed cookies and
  # `ActiveRecord::SignedId` use, so there is one signing
  # implementation rather than three.
  #
  # DIVERGENCE, stated: an sgid minted by a real Rails process does not
  # verify here and vice versa, because the payload differs. Both ends
  # of every round trip in a transpiled app are this file, and nothing
  # in the corpus hands an sgid across that boundary. Recorded in
  # docs/pipeline/runtime.md.
  #
  # The MODEL NAME is a parameter, not `self.class.name`: the caller is
  # the per-model `attachable_sgid` that `lower::attachable`
  # synthesizes with the name baked in, which is the same rule
  # `ActiveRecord::SignedId` states for the purpose it is handed.
  module SignedGlobalId
    SALT = "ActionText::Attachable"
    PURPOSE = "attachable"

    def self.generate(model_name, id)
      ActionController::MessageVerifier.envelope(
        Rails.application.secret_key_base,
        SALT,
        ActionController::MessageVerifier.json_string(model_name + "/" + id.to_s),
        PURPOSE,
        "null",
        false
      )
    end

    # The model name `sgid` was minted for, or "" when it does not
    # verify — a tampered sgid, one signed for another purpose, and a
    # malformed one are all the same answer, matching what
    # `MessageVerifier` does everywhere else.
    def self.model_of(sgid)
      value = verified_value(sgid)
      at = value.index("/")
      return "" if at.nil?
      value[0, at]
    end

    # Its record id, or 0 — the same unsaved/absent sentinel
    # `ActiveRecord::SignedId.verified` answers with, and for the same
    # reason.
    def self.id_of(sgid)
      value = verified_value(sgid)
      at = value.index("/")
      return 0 if at.nil?
      value[at + 1, value.length - at - 1].to_i
    end

    def self.verified_value(sgid)
      ActionController::MessageVerifier.verified(
        Rails.application.secret_key_base, SALT, sgid, PURPOSE, false
      )
    end
  end

  # One `<action-text-attachment>` node, as parsed. Rails' Attachment
  # wraps the node plus the dereferenced attachable; this half is the
  # node.
  class Attachment
    # The element name Action Text canonicalizes every attachment to.
    # Read as a constant by app code (campfire's content filters), so
    # it is a class method rather than a bare literal.
    def self.tag_name
      "action-text-attachment"
    end

    def initialize(attributes)
      @attributes = attributes
    end

    def attributes
      @attributes
    end

    def [](name)
      @attributes.fetch(name, "")
    end

    def sgid
      self["sgid"]
    end

    def content_type
      self["content-type"]
    end

    def caption
      self["caption"]
    end

    def filename
      self["filename"]
    end

    def url
      self["url"]
    end

    # Rails renders an attachment into plain text as its caption, and
    # falls back to the filename when there is none
    # (`Attachment#to_plain_text`).
    def to_plain_text
      text = caption
      text = filename if text == ""
      text
    end
  end

  # One element from a [`Fragment`] scan: its name, its attributes, and
  # its OUTER html (the open tag through its matching close tag, or the
  # tag alone when it is void). A filter reads it the way it reads a
  # Nokogiri node — `node["href"]`, `node.to_s`.
  class Node
    def initialize(name, attributes, outer)
      @name = name
      @attributes = attributes
      @outer = outer
    end

    def name
      @name
    end

    def [](key)
      @attributes.fetch(key, nil)
    end

    def attributes
      @attributes
    end

    def to_s
      @outer
    end

    def to_html
      @outer
    end
  end

  # A parsed [`Fragment`] selector — see `Fragment.parse_selector`.
  class Selector
    def initialize(kind, name, excluded, keys, values, ops)
      @kind = kind
      @name = name
      @excluded = excluded
      @keys = keys
      @values = values
      @ops = ops
    end

    def kind
      @kind
    end

    def name
      @name
    end

    def excluded
      @excluded
    end

    def keys
      @keys
    end

    def values
      @values
    end

    def ops
      @ops
    end
  end

  # `ActionText::Fragment` — the element view of a `Content`'s markup,
  # which is the surface `ActionText::Content::Filter` subclasses work
  # through.
  #
  # Rails wraps a Nokogiri document and takes any CSS or XPath. This is
  # a SCANNER over the same string every other method in this file
  # scans, and it answers the selector shapes an app's filters actually
  # write, refusing the rest. Refusing matters more than usual here: a
  # filter chain runs inside a rescue in every app that has one (one bad
  # message must not take down a room), so a selector quietly matching
  # nothing is indistinguishable from a message with no content.
  #
  # The three shapes:
  #
  #   "div"                                  an element name
  #   "action-text-attachment[@content-type='x'][url*='y']"
  #                                          name plus attribute
  #                                          predicates, `=` exact and
  #                                          `*=` substring; a leading
  #                                          `@` on the name is XPath
  #                                          spelling of the same thing
  #   ":not(a):not(b):not(…)"                any element named by NONE
  #                                          of them — how an allow-list
  #                                          sanitizer spells itself
  class Fragment
    def initialize(html)
      @html = html.to_s
    end

    # Rails' `ActionText::Fragment.wrap` — a Fragment passes through, a
    # String becomes one. Tests reach for it directly to get at an
    # attachment node without building a Content first.
    def self.wrap(value)
      return value if value.is_a?(ActionText::Fragment)
      ActionText::Fragment.new(value.to_s)
    end

    def to_s
      @html
    end

    def to_html
      @html
    end

    def source
      @html
    end

    # Elements matching `selector`, in document order.
    def find_all(selector)
      out = []
      matcher = Fragment.parse_selector(selector)
      i = 0
      n = @html.length
      while i < n
        open_at = Fragment.next_element(@html, i)
        if open_at < 0
          i = n
        else
          tag_end = Fragment.tag_end(@html, open_at)
          raw = @html[open_at + 1, tag_end - open_at - 1].to_s
          name = Content.tag_name_of(raw)
          attrs = Content.parse_attributes(raw)
          stop = Fragment.element_end(@html, open_at, tag_end, name, raw)
          if Fragment.matches?(name, attrs, matcher)
            out << Node.new(name, attrs, @html[open_at, stop - open_at].to_s)
          end
          # Into the children either way: a match's descendants are
          # elements too, and Rails' `css` returns them.
          i = tag_end + 1
        end
      end
      out
    end

    # Rails' `ActionText::Fragment#replace`: every matching element's
    # OUTER html becomes what the block answers for it. A block that
    # answers nil removes the element and its children, which is how a
    # `:not(...)` sanitizer strips a disallowed tag.
    #
    # A matched element is skipped over WHOLE — its children are gone
    # with it — while an unmatched one is copied open-tag-first so the
    # walk continues inside it. That is the same traversal Nokogiri's
    # `css(...).each { |n| n.replace(…) }` produces for these selectors,
    # without a second parser.
    def replace(selector)
      matcher = Fragment.parse_selector(selector)
      out = +""
      i = 0
      n = @html.length
      while i < n
        open_at = Fragment.next_element(@html, i)
        if open_at < 0
          out = out + @html[i, n - i].to_s
          i = n
        else
          out = out + @html[i, open_at - i].to_s
          tag_end = Fragment.tag_end(@html, open_at)
          raw = @html[open_at + 1, tag_end - open_at - 1].to_s
          name = Content.tag_name_of(raw)
          attrs = Content.parse_attributes(raw)
          stop = Fragment.element_end(@html, open_at, tag_end, name, raw)
          if Fragment.matches?(name, attrs, matcher)
            node = Node.new(name, attrs, @html[open_at, stop - open_at].to_s)
            out = out + (yield node).to_s
            i = stop
          else
            out = out + @html[open_at, tag_end - open_at + 1].to_s
            i = tag_end + 1
          end
        end
      end
      Fragment.new(out)
    end

    # ---- scanning -----------------------------------------------------

    # Index of the next OPEN tag at or after `from`, or -1. Close tags,
    # comments and doctypes are not elements.
    def self.next_element(html, from)
      i = from
      n = html.length
      while i < n
        if html[i, 1].to_s == "<"
          nxt = html[i + 1, 1].to_s
          return i if nxt != "/" && nxt != "!" && nxt != ""
        end
        i = i + 1
      end
      -1
    end

    # Index of the ">" closing the tag that starts at `at`.
    def self.tag_end(html, at)
      i = at + 1
      n = html.length
      while i < n && html[i, 1].to_s != ">"
        i = i + 1
      end
      i < n ? i : n - 1
    end

    # One past the last character of the ELEMENT opened at `at`. A void
    # or self-closed element is its own tag; anything else runs to its
    # matching close tag, counting nested opens of the same name so an
    # inner `<div>` does not end an outer one.
    #
    # An unclosed element runs to the end of the string — the same
    # forgiving reading `to_plain_text` gives bad nesting, and for the
    # same reason: repairing it would be a second, different parser.
    def self.element_end(html, at, tag_end, name, raw)
      return tag_end + 1 if raw[raw.length - 1, 1].to_s == "/"
      return tag_end + 1 if void_element(name)
      depth = 1
      i = tag_end + 1
      n = html.length
      stop = n
      while i < n && depth > 0
        if html[i, 1].to_s == "<"
          close = html[i + 1, 1].to_s == "/"
          start = close ? i + 2 : i + 1
          j = i + 1
          while j < n && html[j, 1].to_s != ">"
            j = j + 1
          end
          inner_raw = html[start, j - start].to_s
          if Content.tag_name_of(inner_raw) == name
            if close
              depth = depth - 1
              stop = j + 1 if depth == 0
            elsif inner_raw[inner_raw.length - 1, 1].to_s != "/"
              depth = depth + 1
            end
          end
          i = j + 1
        else
          i = i + 1
        end
      end
      stop
    end

    def self.void_element(name)
      name == "area" || name == "base" || name == "br" || name == "col" ||
        name == "embed" || name == "hr" || name == "img" || name == "input" ||
        name == "link" || name == "meta" || name == "param" ||
        name == "source" || name == "track" || name == "wbr"
    end

    # ---- selectors ----------------------------------------------------

    # A parsed selector. A CLASS rather than a Hash because its fields
    # are of different types and a `Hash[String, untyped]` bag is the
    # shape the strict targets are built to avoid — one element type
    # per container ([[reference_spinel_slow_shapes]]). `excluded`,
    # `keys`, `values` and `ops` are each `Array[String]`.
    def self.parse_selector(selector)
      text = selector.to_s.strip
      return Selector.new("not", "", not_names(text), [], [], []) if text.start_with?(":not(")
      head = text.split("[")[0].to_s
      # A plain element NAME, refused otherwise. Combinators, classes
      # and ids are shapes this scanner does not read, and one that
      # silently matched nothing would be indistinguishable from a
      # filter that had nothing to do — inside a rescue, from a message
      # with no content at all.
      unless element_name?(head)
        raise "ActionText::Fragment: unsupported selector #{selector.inspect}"
      end
      keys = []
      values = []
      ops = []
      parts = text.split("[")
      i = 1
      while i < parts.length
        pred = parts[i].to_s.split("]")[0].to_s
        eq = pred.index("=")
        raise "ActionText::Fragment: unsupported predicate #{pred.inspect}" if eq.nil?
        substring = pred[eq - 1, 1].to_s == "*"
        op = substring ? "*=" : "="
        key = pred[0, substring ? eq - 1 : eq].to_s
        key = key[1, key.length - 1].to_s if key.start_with?("@")
        value = pred[eq + 1, pred.length].to_s
        value = value.gsub("'", "").gsub("\"", "")
        keys << key
        values << value
        ops << op
        i = i + 1
      end
      Selector.new("name", head, [], keys, values, ops)
    end

    # Letters, digits, `-` and `_` only — `action-text-attachment` is a
    # name, `div > span` and `.cls` are not.
    def self.element_name?(text)
      return false if text == ""
      i = 0
      while i < text.length
        c = text[i, 1].to_s
        ok = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") ||
          (c >= "0" && c <= "9") || c == "-" || c == "_"
        return false unless ok
        i = i + 1
      end
      true
    end

    # `:not(a):not(abbr):not(…)` → the names, in order.
    def self.not_names(text)
      out = []
      parts = text.split(":not(")
      i = 1
      while i < parts.length
        out << parts[i].to_s.split(")")[0].to_s
        i = i + 1
      end
      raise "ActionText::Fragment: unsupported selector #{text.inspect}" if out.length == 0
      out
    end

    def self.matches?(name, attrs, matcher)
      return !matcher.excluded.include?(name) if matcher.kind == "not"
      return false if matcher.name != name
      keys = matcher.keys
      values = matcher.values
      ops = matcher.ops
      ok = true
      i = 0
      while i < keys.length
        actual = attrs.fetch(keys[i], nil)
        if actual.nil?
          ok = false
        elsif ops[i] == "*="
          ok = false unless actual.include?(values[i])
        else
          ok = false unless actual == values[i]
        end
        i = i + 1
      end
      ok
    end
  end

  class Content
    # `canonicalize:` is Rails' "re-render the attachments through their
    # partials before storing" switch. A filter chain passes
    # `canonicalize: false` for exactly that reason — it has just
    # rewritten the markup and does not want it rewritten again — and
    # nothing here canonicalizes in the first place, so the keyword is
    # ACCEPTED AND IGNORED rather than absent. Absent was a TypeError on
    # every filtered message, and campfire's `message_presentation`
    # rescues, so it read as an empty body rather than as an error.
    def initialize(html = "", canonicalize: true)
      @html = html.to_s
      @canonicalize = canonicalize
    end

    # `ActionText::Content#fragment` — the element view a
    # `Content::Filter` works through (campfire's filter base is
    # `delegate :fragment, to: :content`).
    def fragment
      ActionText::Fragment.new(@html)
    end

    # The stored markup, unchanged. `to_s` is the same string: Rails'
    # `to_s` renders through the `action_text/contents/_content`
    # layout, which only wraps the fragment in a `<div
    # class="rich-text">`; the wrapper is view-side decoration that the
    # emitted views apply themselves, so the value object stays the
    # fragment.
    def to_html
      @html
    end

    def to_s
      @html
    end

    def as_json
      @html
    end

    def blank?
      to_plain_text == ""
    end

    def empty?
      blank?
    end

    def present?
      !blank?
    end

    # Every `href` in the fragment, in document order, deduplicated —
    # `ActionText::Content#links`.
    def links
      out = []
      nodes = Content.scan_tags(@html)
      i = 0
      while i < nodes.length
        node = nodes[i]
        if node.fetch("__name", "") == "a"
          href = node.fetch("href", "")
          out << href if href != "" && !out.include?(href)
        end
        i = i + 1
      end
      out
    end

    # The parsed `<action-text-attachment>` nodes. Complete with
    # respect to the markup: every attribute the element carried is on
    # the returned Attachment.
    def attachments
      out = []
      nodes = Content.scan_tags(@html)
      i = 0
      while i < nodes.length
        node = nodes[i]
        out << ActionText::Attachment.new(node) if node.fetch("__name", "") == ActionText::Attachment.tag_name
        i = i + 1
      end
      out
    end

    # DIVERGENCE, stated: Rails resolves each attachment node's signed
    # GlobalID back to the record it points at (a Blob, a User, any
    # `ActionText::Attachable`). That needs SignedGlobalID verification
    # and a GlobalID registry, neither of which exists here yet, so the
    # list is empty. Callers that grep this for a model class (mention
    # extraction) see no mentions rather than wrong ones.
    def attachables
      []
    end

    # The record ids the attachment nodes minted for `model_name` carry,
    # in document order and without repeats.
    #
    # This is the half of `attachables` that IS answerable: the caller
    # (`lower::attachables_grep`, rewriting `attachables.grep(User)`)
    # names its model class at the call site, so the name-to-class
    # lookup a full dereference would need never arises — there is no
    # GlobalID registry here and none is required.
    #
    # A node with no `sgid`, one minted for another model, or one that
    # fails verification simply does not contribute an id. So does an
    # sgid whose record was deleted: the caller's `where` returns fewer
    # rows, which is what Rails' MissingAttachable stands in for.
    #
    # Deduped HERE rather than by the caller: `uniq` on records compares
    # by object identity in this runtime, so two reads of one row are
    # two objects and a trailing `.uniq` in app code would not collapse
    # them. Integers do.
    def attachable_ids(model_name)
      out = []
      nodes = attachments
      i = 0
      while i < nodes.length
        sgid = nodes[i].attributes.fetch("sgid", "")
        if sgid != "" && ActionText::SignedGlobalId.model_of(sgid) == model_name
          id = ActionText::SignedGlobalId.id_of(sgid)
          out << id if id != 0 && !out.include?(id)
        end
        i = i + 1
      end
      out
    end

    # Rails' PlainTextConversion, transcribed.
    #
    # Every rule there is stated over ONE NODE'S OWN TEXT — "chomp this
    # node's children, then add two newlines" — so a scanner that
    # chomps the whole accumulator gets a different answer as soon as a
    # node is empty (`<div>a</div><div></div><div>b</div>` must keep
    # the blank line; chomping the accumulator eats it). The stack
    # below is what buys back the node scope: an open tag records where
    # in `out` its text begins, and the close tag rewrites exactly that
    # slice. That is Rails' bottom-up reduce, done in one left-to-right
    # pass.
    #
    # `names`/`starts` are the open-element stack; `list_names` and
    # `list_counts` track enclosing `<ul>`/`<ol>` so `<li>` can pick
    # between a bullet and its ordinal, and indent by nesting depth.
    # Parallel arrays of ONE element type each, not a stack of records
    # — the container rule from the slow-shape catalog.
    #
    # A close tag that does not match the top of the stack is IGNORED
    # rather than unwinding to find its partner: Rails parses through
    # Nokogiri, which repairs bad nesting before conversion ever runs,
    # and guessing at a repair here would be a second, different
    # parser. Well-formed markup — everything the editors produce — is
    # unaffected.
    def to_plain_text
      out = +""
      names = []
      starts = []
      list_names = []
      list_counts = []
      skipping = ""
      skip_depth = 0
      i = 0
      n = @html.length
      while i < n
        c = @html[i, 1].to_s
        if c == "<"
          close = @html[i + 1, 1].to_s == "/"
          name_start = close ? i + 2 : i + 1
          j = name_start
          while j < n && @html[j, 1].to_s != ">"
            j = j + 1
          end
          raw = @html[name_start, j - name_start].to_s
          name = Content.tag_name_of(raw)
          void = raw[raw.length - 1, 1].to_s == "/"
          if skipping != ""
            if close && name == skipping
              skip_depth = skip_depth - 1
              skipping = "" if skip_depth <= 0
            elsif !close && name == skipping
              skip_depth = skip_depth + 1
            end
          elsif name == "script" || name == "style"
            unless close || void
              skipping = name
              skip_depth = 1
            end
          elsif name == "br"
            out = out + "\n"
          elsif name == ActionText::Attachment.tag_name
            unless close
              out = out + ActionText::Attachment.new(Content.parse_attributes(raw)).to_plain_text
            end
          elsif Content.scoped_element(name)
            if close
              if names.length > 0 && names[names.length - 1] == name
                start = starts[starts.length - 1]
                names.pop
                starts.pop
                segment = out[start, out.length - start].to_s
                if name == "ul" || name == "ol"
                  list_names.pop
                  list_counts.pop
                end
                out = out[0, start].to_s +
                      Content.close_scoped(name, segment, list_names, list_counts)
              end
            elsif !void
              if name == "li" && list_counts.length > 0
                list_counts[list_counts.length - 1] = list_counts[list_counts.length - 1] + 1
              end
              if name == "ul" || name == "ol"
                list_names << name
                list_counts << 0
              end
              names << name
              starts << out.length
            end
          end
          i = j + 1
        elsif c == "&"
          stop = Content.entity_end(@html, i)
          if stop > i
            out = out + Content.decode_entity(@html[i, stop - i + 1].to_s) if skipping == ""
            i = stop + 1
          else
            out = out + c if skipping == ""
            i = i + 1
          end
        else
          out = out + c if skipping == ""
          i = i + 1
        end
      end
      Content.chomp_newlines(out)
    end

    # The elements whose own text Rails rewrites on the way out. Every
    # other element — `<h2>`, `<pre>`, `<em>`, `<span>` — is
    # transparent, contributing its children and nothing else. That is
    # not an omission: PlainTextConversion aliases the block rule to
    # `h1` and `p` ONLY, and defines separate rules for the five below.
    def self.scoped_element(name)
      name == "p" || name == "h1" || name == "div" || name == "blockquote" ||
        name == "figcaption" || name == "li" || name == "ul" || name == "ol"
    end

    # `segment` is the element's own accumulated text; the return value
    # replaces it. `list_names`/`list_counts` are the enclosing-list
    # stacks — for `<ul>`/`<ol>` this element has already been popped
    # off them, for `<li>` its own list is still on top.
    def self.close_scoped(name, segment, list_names, list_counts)
      inner = chomp_newlines(segment)
      return inner + "\n\n" if name == "p" || name == "h1"
      return inner + "\n" if name == "div"
      return "[" + inner + "]" if name == "figcaption"
      return quote_wrap(inner + "\n\n") if name == "blockquote"
      if name == "ul" || name == "ol"
        # `break_if_nested_list` — a list inside another list starts on
        # its own line.
        return list_names.length > 0 ? "\n" + inner + "\n\n" : inner + "\n\n"
      end
      # `<li>`: indent by nesting depth, then the bullet its list type
      # dictates. Its own list is still on the stack, so depth 1 is a
      # top-level item and gets no indent.
      bullet = "•"
      depth = list_names.length
      if depth > 0 && list_names[depth - 1] == "ol"
        bullet = list_counts[depth - 1].to_s + "."
      end
      indent(depth) + bullet + " " + inner + "\n"
    end

    # Rails' `plain_text_for_blockquote_node`: the block's text is
    # wrapped in curly quotes placed AGAINST the text, inside whatever
    # surrounding whitespace it carries.
    def self.quote_wrap(text)
      first = first_non_space_index(text)
      return "“”" if first < 0
      last = last_non_space_index(text)
      text[0, first].to_s + "“" + text[first, last - first + 1].to_s + "”" +
        text[last + 1, text.length - last - 1].to_s
    end

    def self.first_non_space_index(text)
      i = 0
      while i < text.length
        return i unless space_at(text, i)
        i = i + 1
      end
      -1
    end

    def self.last_non_space_index(text)
      i = text.length - 1
      while i >= 0
        return i unless space_at(text, i)
        i = i - 1
      end
      -1
    end

    # Every tag in `html` as an attribute hash, with the element name
    # under the `__name` key. Closing tags are skipped — no consumer
    # here needs them, and the reserved key keeps the shape one flat
    # `Hash[String, String]` on every target rather than a pair type.
    def self.scan_tags(html)
      out = []
      i = 0
      n = html.length
      while i < n
        if html[i, 1].to_s == "<"
          j = i + 1
          while j < n && html[j, 1].to_s != ">"
            j = j + 1
          end
          raw = html[i + 1, j - i - 1].to_s
          if raw[0, 1].to_s != "/" && raw[0, 1].to_s != "!"
            attrs = parse_attributes(raw)
            attrs["__name"] = tag_name_of(raw)
            out << attrs
          end
          i = j + 1
        else
          i = i + 1
        end
      end
      out
    end

    # The element name from a tag's inner text ("a href=…" → "a"),
    # downcased. `raw` may still carry a leading "/" for a close tag.
    def self.tag_name_of(raw)
      text = raw
      text = text[1, text.length - 1].to_s if text[0, 1].to_s == "/"
      i = 0
      n = text.length
      while i < n
        c = text[i, 1].to_s
        break if c == " " || c == "\t" || c == "\n" || c == "\r" || c == "/"
        i = i + 1
      end
      text[0, i].to_s.downcase
    end

    # `name="value"` pairs from a tag's inner text. Single quotes and
    # unquoted values both parse; values are entity-decoded, which is
    # what an attribute read gives you in Rails.
    def self.parse_attributes(raw)
      attrs = {}
      i = 0
      n = raw.length
      # Step past the element name.
      while i < n && !Content.space_at(raw, i)
        i = i + 1
      end
      while i < n
        while i < n && Content.space_at(raw, i)
          i = i + 1
        end
        name_start = i
        while i < n && !Content.space_at(raw, i) && raw[i, 1].to_s != "="
          i = i + 1
        end
        name = raw[name_start, i - name_start].to_s.downcase
        break if name == ""
        value = ""
        if raw[i, 1].to_s == "="
          i = i + 1
          quote = raw[i, 1].to_s
          if quote == "\"" || quote == "'"
            i = i + 1
            value_start = i
            while i < n && raw[i, 1].to_s != quote
              i = i + 1
            end
            value = raw[value_start, i - value_start].to_s
            i = i + 1
          else
            value_start = i
            while i < n && !Content.space_at(raw, i)
              i = i + 1
            end
            value = raw[value_start, i - value_start].to_s
          end
        end
        attrs[name] = decode_entities(value) if name != "/"
      end
      attrs
    end

    def self.space_at(text, index)
      c = text[index, 1].to_s
      c == " " || c == "\t" || c == "\n" || c == "\r"
    end

    # Index of the ";" closing an entity that starts at `start`, or
    # `start` when what follows is a bare "&" rather than an entity.
    # Bounded at 10 characters — the longest entity this table knows is
    # "&nbsp;", and an unbounded scan would swallow the rest of a
    # document on every stray ampersand.
    def self.entity_end(html, start)
      i = start + 1
      limit = start + 10
      limit = html.length - 1 if limit > html.length - 1
      while i <= limit
        c = html[i, 1].to_s
        return i if c == ";"
        return start if c == " " || c == "<" || c == "&"
        i = i + 1
      end
      start
    end

    ENTITIES = {
      "&amp;" => "&",
      "&lt;" => "<",
      "&gt;" => ">",
      "&quot;" => "\"",
      "&#39;" => "'",
      "&apos;" => "'",
      "&nbsp;" => " ",
    }.freeze

    # One entity, including its "&" and ";".
    #
    # NAMED ENTITIES ONLY, and only the ones Rails' own escaper
    # produces (`ActionView::ViewHelpers::HTML_ESCAPES` beside this
    # file) plus `&nbsp;` and `&apos;`. Anything else — a numeric
    # reference, an exotic name — passes through verbatim rather than
    # decoding, because decoding it needs a codepoint-to-character
    # intrinsic the framework runtime does not carry. Ledgered in
    # docs/pipeline/runtime.md; the round-trip that matters (escape
    # then extract) is closed, since every entity `html_escape` can
    # emit is in the table.
    def self.decode_entity(entity)
      known = ENTITIES.fetch(entity, "")
      return known if known != ""
      entity
    end

    def self.decode_entities(text)
      out = +""
      i = 0
      n = text.length
      while i < n
        if text[i, 1].to_s == "&"
          stop = entity_end(text, i)
          if stop > i
            out = out + decode_entity(text[i, stop - i + 1].to_s)
            i = stop + 1
          else
            out = out + "&"
            i = i + 1
          end
        else
          out = out + text[i, 1].to_s
          i = i + 1
        end
      end
      out
    end

    # Two spaces per nesting level past the first — Rails'
    # `indentation_for_li_node`. A loop, not `"  " * n`: String#* has
    # no shape in the emitters this file transpiles through.
    def self.indent(list_depth)
      out = +""
      i = 1
      while i < list_depth
        out = out + "  "
        i = i + 1
      end
      out
    end

    def self.ends_with_newline(text)
      text.length > 0 && text[text.length - 1, 1].to_s == "\n"
    end

    # Rails' `remove_trailing_newlines`, which is `chomp("")` — every
    # trailing newline, not just one.
    def self.chomp_newlines(text)
      stop = text.length
      while stop > 0 && text[stop - 1, 1].to_s == "\n"
        stop = stop - 1
      end
      text[0, stop].to_s
    end
  end
end
