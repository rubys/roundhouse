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
# Attachments wrap Active Storage blobs (a filename is an
# `ActiveStorage::Filename`), as in Rails.
require_relative "active_storage"

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
    # `locate(model_name, id)` — the record an sgid names, or nil. THIS
    # body is the default, and it is REDEFINED per app: the models that
    # mix this module in are a compile-time set, so
    # `project::apply_attachable_locate` generates the real method into
    # the emitted tree's `global_id_locator.rb` (the ruby family's
    # locator file, required after this one by both boots) as a `case`
    # over them — `when "User" then User.find_by(id: id)`. The switch IS
    # the registry: no `const_get`, nothing filled in at load time. An
    # app with no attachable model keeps this nil, so every sgid reads
    # as missing there, which is what Rails answers for a gid naming no
    # class. Redefinition by a later reopen is a shape both CRuby and
    # spinel take (the later definition wins; probed).
    def self.locate(model_name, id)
      nil
    end
  end

  module Attachables
    # Rails' stand-in for an attachment whose sgid names nothing: the
    # node carried none, it did not verify, it named a model no locator
    # knows, or the row is gone. Rendered through its own partial, as
    # Rails does.
    class MissingAttachable
      DEFAULT_PARTIAL_PATH = "action_text/attachables/missing_attachable"

      def initialize(sgid)
        @sgid = sgid
      end

      def sgid
        @sgid
      end

      def to_partial_path
        DEFAULT_PARTIAL_PATH
      end
    end
  end

  # The signed GlobalID an `<action-text-attachment>` node carries —
  # Rails' `SignedGlobalID` at the wire level: a `gid://<app>/<Model>/
  # <id>` URI in the `data` envelope `GlobalID::Verifier` writes
  # (`MessageVerifier.gid_envelope` holds the measurement). An sgid a
  # real Rails process minted verifies here and one minted here
  # verifies there, which is what a database written by Rails needs:
  # every @mention in `action_text_rich_texts.body` is one of these,
  # and under the `<Model>/<id>` shape this file used to sign they
  # all read as missing.
  #
  # The MODEL NAME is a parameter, not `self.class.name`: the caller is
  # the per-model `attachable_sgid` that `lower::attachable`
  # synthesizes with the name baked in, which is the same rule
  # `ActiveRecord::SignedId` states for the purpose it is handed.
  module SignedGlobalId
    # globalid's railtie: `GlobalID::Verifier.new(app.key_generator
    # .generate_key('signed_global_ids'))`.
    SALT = "signed_global_ids"
    # `ActionText::Attachable::LOCATOR_NAME`, the `for:` every
    # attachable sgid is minted and verified under.
    PURPOSE = "attachable"

    def self.generate(model_name, id)
      ActionController::MessageVerifier.gid_envelope(
        Rails.application.secret_key_base,
        SALT,
        ActionController::MessageVerifier.json_string(uri(model_name, id)),
        PURPOSE,
        ""
      )
    end

    # The URI as `attachable_sgid` signs it: `to_sgid(expires_in: nil,
    # for: LOCATOR_NAME)` hands globalid an `expires_in: nil` param,
    # and `URI::GID` serializes a nil param as a bare key — so the
    # signed bytes carry `?expires_in`, and reproducing Rails' sgid
    # means minting it too. The readers below strip any query.
    def self.uri(model_name, id)
      GlobalID.uri(model_name, id) + "?expires_in"
    end

    # The model name `sgid` was minted for, or "" when it does not
    # verify — a tampered sgid, one signed for another purpose, one
    # naming another app, and a malformed one are all the same answer,
    # matching what `MessageVerifier` does everywhere else.
    def self.model_of(sgid)
      model_in(verified_uri(sgid))
    end

    # Its record id, or 0 — the same unsaved/absent sentinel
    # `ActiveRecord::SignedId.verified` answers with, and for the same
    # reason.
    def self.id_of(sgid)
      id_in(verified_uri(sgid))
    end

    # The gid URI `sgid` carries once its signature, purpose and expiry
    # have been checked, or "".
    def self.verified_uri(sgid)
      json = ActionController::MessageVerifier.verified_data_json(
        Rails.application.secret_key_base, SALT, sgid, PURPOSE, true
      )
      return "" if json == ""
      ActionController::MessageVerifier.json_value(json)
    end

    # The gid URI `sgid` carries WITHOUT checking its signature, or "".
    # This is the read campfire's `lib/rails_ext/action_text_attachables.rb`
    # does by hand so that rotating SECRET_KEY_BASE does not orphan
    # every @mention; `Attachment#attachable` asks it only for the
    # models that reopen names (`Attachment.permitted_without_signature`),
    # and only after the signed read has failed. Both envelopes Rails
    # has minted are read, as that file reads them:
    #
    #   * 7.1+: `{"_rails":{"data":"gid://…","pur":…}}` — the URI is
    #     the `data` string;
    #   * 7.0:  `{"_rails":{"message":<base64(Marshal)>,…}}` — the URI is
    #     found inside the marshaled bytes by its `gid://<app>/` prefix,
    #     since unmarshaling an unverified payload is what nobody does.
    #
    # Nothing here is trusted: the URI that comes back is a name, and
    # `Attachable.locate` is a `find_by` on a model the app listed.
    def self.unverified_uri(sgid)
      sep = sgid.index("--")
      payload = sep.nil? ? sgid : sgid[0, sep]
      env = decode_base64(payload)
      return "" if env == ""
      data = ActionController::MessageVerifier.extract(env, "\"data\":\"")
      return data if data != ""
      message = ActionController::MessageVerifier.extract(env, "\"message\":\"")
      return "" if message == ""
      gid_in(decode_base64(message))
    end

    # `Model` in `gid://<app>/<Model>/<id>[?…]`, or "" for a URI that is
    # not one this app mints — another app's gid is not a name here,
    # the rule `GlobalID::Locator.parts_from` states.
    def self.model_in(uri)
      head = gid_head
      return "" unless uri.start_with?(head)
      rest = uri[head.length, uri.length - head.length]
      slash = rest.index("/")
      return "" if slash.nil?
      rest[0, slash]
    end

    # `id` in the same, or 0. Digits only: a `?expires_in` (or any
    # query) after them is dropped, and anything that is not an
    # integer id reads as the absent sentinel.
    def self.id_in(uri)
      head = gid_head
      return 0 unless uri.start_with?(head)
      rest = uri[head.length, uri.length - head.length]
      slash = rest.index("/")
      return 0 if slash.nil?
      tail = rest[slash + 1, rest.length - slash - 1]
      q = tail.index("?")
      tail = tail[0, q] unless q.nil?
      tail.to_i
    end

    def self.gid_head
      "gid://" + Rails.application.global_id_app + "/"
    end

    # The first `gid://<app>/<Model>/<digits>` inside `bytes` (a Marshal
    # dump, where the URI is a plain string among binary tokens),
    # rebuilt from just those parts, or "".
    def self.gid_in(bytes)
      head = gid_head
      at = bytes.index(head)
      return "" if at.nil?
      rest = bytes[at + head.length, bytes.length - at - head.length]
      slash = rest.index("/")
      return "" if slash.nil?
      model = rest[0, slash]
      digits = +""
      i = slash + 1
      while i < rest.length
        c = rest[i]
        break if c < "0" || c > "9"
        digits << c.to_s
        i = i + 1
      end
      return "" if digits.empty?
      head + model + "/" + digits
    end

    # Either base64 alphabet, padded or not, and "" rather than a
    # raise for text that is neither — the same tolerance campfire's
    # `decode_base64` (strict, then url-safe) gives, in one call:
    # `urlsafe_decode64` maps `-`/`_` and leaves `+`/`/` alone.
    def self.decode_base64(text)
      Base64.urlsafe_decode64(text)
    rescue ArgumentError
      ""
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

    # actiontext 8.1.2, `lib/action_text/attachment.rb:24`, verbatim and
    # in order. A rule table, ported rather than derived: campfire's
    # `SanitizeAttributes` adds it to the sanitizer's own allow-list, so
    # a name missing here is an attribute silently stripped out of every
    # attachment that carries it.
    #
    # A real CONSTANT, unlike `tag_name` above, because the app reads it
    # as one (`... + ActionText::Attachment::ATTRIBUTES`).
    ATTRIBUTES = [
      "sgid", "content-type", "url", "href", "filename", "filesize",
      "width", "height", "previewable", "presentation", "caption", "content"
    ]

    def initialize(attributes)
      @attributes = attributes
    end

    # Rails' `Attachment.from_node(node, attachable = nil)` — the
    # attachment over one parsed `<action-text-attachment>` element.
    # The attachable is not taken here: it is dereferenced lazily by
    # `#attachable` from the node's sgid, which is the only source this
    # runtime has for it (Rails also accepts a caller-supplied record).
    def self.from_node(node)
      Attachment.new(node.attributes)
    end

    def attributes
      @attributes
    end

    def [](name)
      @attributes.fetch(name, "")
    end

    # The record this attachment points at — Rails' `attachable`,
    # resolved from the node's SIGNED sgid through `Attachable.locate`
    # — or a `MissingAttachable` when nothing answers: no sgid on the
    # node, a signature that does not verify (tampered, or minted under
    # another secret), a model no locator knows, a row since deleted.
    #
    # WITH ONE TOLERANCE, which is the app's, not this runtime's:
    # campfire's `lib/rails_ext/action_text_attachables.rb` reopens
    # `from_node` to accept a `User` sgid whose signature FAILS, so
    # rotating SECRET_KEY_BASE does not orphan every @mention. The
    # ingest reads that reopen for the list it holds
    # (`ATTACHABLES_PERMITTED_WITH_INVALID_SIGNATURES = %w[ User ]`)
    # and `permitted_without_signature` below is that list; the
    # decoding the reopen does by hand is `SignedGlobalId.unverified_uri`.
    # An app without the reopen keeps the empty default, and a tampered
    # sgid is missing here as it is in stock Rails.
    def attachable
      model_name = resolved_model_name
      record = nil
      if model_name != ""
        record = ActionText::Attachable.locate(model_name, resolved_id)
      end
      record.nil? ? ActionText::Attachables::MissingAttachable.new(self["sgid"]) : record
    end

    # The model NAME the node's sgid resolves to, or "" — the signed
    # read first, then the app's tolerance for a failed signature. A
    # String rather than a class so a generated caller can `case` on
    # it: `Content.render_attachment` picks the attachable's partial
    # this way, with the finder spelled as a literal constant, the
    # same shape `Attachable.locate` takes.
    def resolved_model_name
      sgid = self["sgid"]
      return "" if sgid == ""
      model_name = ActionText::SignedGlobalId.model_of(sgid)
      return model_name if model_name != ""
      uri = ActionText::SignedGlobalId.unverified_uri(sgid)
      model_name = ActionText::SignedGlobalId.model_in(uri)
      Attachment.permitted_without_signature.include?(model_name) ? model_name : ""
    end

    # Its id, by the same two reads, or 0 when neither answers.
    def resolved_id
      sgid = self["sgid"]
      return 0 if sgid == ""
      model_name = ActionText::SignedGlobalId.model_of(sgid)
      return ActionText::SignedGlobalId.id_of(sgid) if model_name != ""
      uri = ActionText::SignedGlobalId.unverified_uri(sgid)
      model_name = ActionText::SignedGlobalId.model_in(uri)
      Attachment.permitted_without_signature.include?(model_name) ? ActionText::SignedGlobalId.id_in(uri) : 0
    end

    # Model names whose sgid resolves even when its signature does not
    # verify — EMPTY here, and REDEFINED per app the way
    # `Attachable.locate` is: when the app's `lib/` reopens
    # `Attachment.from_node` with such a list, `project::
    # apply_attachable_locate` writes it into the emitted
    # `global_id_locator.rb` as a literal, and the later definition
    # wins on both boots. A model name, not a class: `Attachable.locate`
    # is what turns it into a finder.
    def self.permitted_without_signature
      []
    end

    def sgid
      self["sgid"]
    end

    def content_type
      self["content-type"]
    end

    # Rails: `node_attributes["caption"].presence` — nil when the node
    # carries no caption (or an empty one), which is what
    # `active_storage/blobs/_blob.html.erb`'s `if caption =
    # blob.try(:caption)` tests before falling back to the filename.
    # An empty String here rendered an empty caption instead.
    def caption
      v = self["caption"]
      v.empty? ? nil : v
    end

    # Rails delegates this to the blob, whose `filename` is an
    # `ActiveStorage::Filename` (extension, base, …); the node carries
    # the same name as text, so wrap it the same way.
    def filename
      ActiveStorage::Filename.new(self["filename"])
    end

    def url
      self["url"]
    end

    # Rails renders an attachment into plain text as its caption, and
    # falls back to the filename when there is none
    # (`Attachment#to_plain_text`).
    def to_plain_text
      caption || filename.to_s
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

    # Index of the ">" closing the tag that starts at `at`. A ">" inside
    # a quoted attribute value is part of the value, not the tag's end —
    # `<meta content="Hey!<script>alert('hi')</script>">` is one tag
    # whose `content` holds markup, which is exactly what an opengraph
    # page under sanitisation looks like.
    def self.tag_end(html, at)
      i = at + 1
      n = html.length
      quote = ""
      while i < n
        c = html[i, 1].to_s
        if quote != ""
          quote = "" if c == quote
        elsif c == "\"" || c == "'"
          quote = c
        elsif c == ">"
          break
        end
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

    # The stored markup, unchanged — `to_html` is the FRAGMENT, which
    # is what the filter chain and persistence read.
    def to_html
      @html
    end

    # `to_s` RENDERS: Rails routes it through the app's
    # `layouts/action_text/contents/_content` layout, which is how a
    # rich text arrives wrapped (campfire's is
    # `<div class="trix-content">`, and its CSS keys on that class).
    # This comment used to claim the wrapper was "view-side decoration
    # that the emitted views apply themselves"; measured, no emitted
    # view applies it — the CRuby overlay corrected it first
    # (action_view_safe_buffer.rb), and `scripts/campfire-compare`
    # caught the strict lane rendering one div short of Rails.
    #
    # The layout is per-APP and the strict targets resolve every call
    # statically, so the dispatch below is GENERATED: when the emitted
    # tree carries the layout view module, `project.rs` rewrites the
    # marked span to call it (`apply_content_layout`, the same
    # re-appliable span replace as `apply_cable_connection`); a tree
    # without the layout keeps the bare fragment, which is Rails'
    # fallback too.
    def to_s
      rendered_html
    end

    # >>> generated: content-layout
    def rendered_html
      render_attachments
    end
    # <<< generated: content-layout

    # Rails' `render_action_text_attachments`: every
    # `<action-text-attachment>` node gets its attachable's partial as
    # its INNER html — the node stays, the partial's markup goes inside
    # it — before the layout wraps the whole. That order is
    # load-bearing for what happens next in an app: campfire hands the
    # result to `auto_link`, whose safe-list pass strips the attachment
    # tag it does not allow but keeps the children, so a mention
    # arrives on the page as `users/_mention` and a preview as its
    # figure. With no render, that strip left NOTHING — `Hey @bender`
    # served as `Hey ` — and a lane that does not strip served a bare
    # element a browser draws as nothing.
    #
    # A scanner over the stored markup rather than a parse: the nodes
    # are Trix's own output, never nested, and always spelled
    # `<action-text-attachment …>…</action-text-attachment>`. A node
    # that already carries children (a stored gallery, an editor's
    # round trip) has them REPLACED, as Rails' `inner_html=` does.
    # The partial itself is per app, so what goes inside is
    # `Content.render_attachment` below — generated, like the layout.
    def render_attachments
      html = @html
      open_tag = "<" + ActionText::Attachment.tag_name
      close_tag = "</" + ActionText::Attachment.tag_name + ">"
      out = +""
      i = 0
      n = html.length
      while i < n
        at = html.index(open_tag, i)
        if at.nil?
          out << html[i, n - i].to_s
          break
        end
        gt = Content.tag_end(html, at)
        if gt < 0
          out << html[i, n - i].to_s
          break
        end
        close_at = html.index(close_tag, gt + 1)
        if close_at.nil?
          out << html[i, n - i].to_s
          break
        end
        out << html[i, at - i].to_s
        raw = html[at + 1, gt - at - 1].to_s
        # `<action-text-attachment …/>` — the self-closing spelling has
        # no children to replace and no close tag of its own.
        if raw[raw.length - 1, 1].to_s == "/"
          out << html[at, gt - at + 1].to_s
          i = gt + 1
          next
        end
        attrs = Content.parse_attributes(raw)
        attrs["__name"] = ActionText::Attachment.tag_name
        inner = Content.render_attachment(ActionText::Attachment.new(attrs))
        out << html[at, gt - at + 1].to_s
        out << inner
        out << close_tag
        i = close_at + close_tag.length
      end
      out
    end

    # The inner html for ONE attachment node — its attachable's
    # partial, rendered. GENERATED per app by `project::
    # apply_content_layout`, beside the layout: one arm per model that
    # mixes `ActionText::Attachable` in and names a partial through
    # `to_attachable_partial_path` (campfire's `User::Mentionable`
    # answers "users/mention"), dispatched on the node's resolved
    # model NAME with the finder and the view module spelled as
    # literals. Rails passes the partial the Attachment, which
    # delegates to the record; the emitted partial takes the record,
    # which is what campfire's `_mention` reads (`user.name`,
    # `user.attachable_sgid`). A node whose sgid resolves to nothing,
    # or to a model with no partial, keeps an empty inner — the bare
    # node, which is what it was before.
    # >>> generated: attachment-render
    def self.render_attachment(attachment)
      ""
    end
    # <<< generated: attachment-render

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
          j = Content.tag_end(html, i)
          j = n if j < 0
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
    # The index of the `>` that closes the tag opening at `at`, or -1
    # — skipping any `>` inside a quoted attribute value. A mention
    # node as campfire's own tests write it carries the rendered
    # mention in its `content` attribute (`content="<div
    # class=&quot;mention&quot;…>"`), and the first `>` in that tag is
    # inside the quotes: a scan that stopped there read the tag as
    # ending mid-attribute, and a render spliced there landed inside
    # the value.
    def self.tag_end(html, at)
      n = html.length
      j = at + 1
      quote = ""
      while j < n
        c = html[j, 1].to_s
        if quote != ""
          quote = "" if c == quote
        elsif c == "\"" || c == "'"
          quote = c
        elsif c == ">"
          return j
        end
        j = j + 1
      end
      -1
    end

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
    # The named entities are only the ones Rails' own escaper produces
    # (`ActionView::ViewHelpers::HTML_ESCAPES` beside this file) plus
    # `&nbsp;` and `&apos;`. A NUMERIC reference (`&#60;`, `&#x3c;`)
    # decodes when it names a printable ASCII character — the range an
    # attacker spells markup in, which is the case campfire's opengraph
    # sanitiser exists for (`&#x3c;&#x2f;&#x73;…` is `</script><img
    # onerror=…>` hidden from a tag stripper). Anything else — a
    # codepoint past ASCII, an exotic name — passes through verbatim
    # rather than decoding: that needs a codepoint-to-character
    # intrinsic the runtime does not carry. Ledgered in
    # docs/pipeline/runtime.md; the round-trip that matters (escape
    # then extract) is closed, since every entity `html_escape` can
    # emit is in the table.
    def self.decode_entity(entity)
      known = ENTITIES.fetch(entity, "")
      return known if known != ""
      if entity.start_with?("&#") && entity.length > 3
        digits = entity[2, entity.length - 3].to_s
        hex = digits.start_with?("x") || digits.start_with?("X")
        digits = digits[1, digits.length - 1].to_s if hex
        code = numeric_reference(digits, hex ? 16 : 10)
        return printable_ascii(code) if code >= 32 && code <= 126
      end
      entity
    end

    # The character for a printable ASCII codepoint, read out of the
    # range as a String — the runtime carries no codepoint-to-character
    # intrinsic, and a table is the same on every target.
    def self.printable_ascii(code)
      table = " !\"\#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
      table[code - 32, 1].to_s
    end

    # The digits of a numeric reference as an Integer, -1 when any
    # digit is outside the base. Hand-rolled over `index` rather than
    # `to_i(base)` so it prices the same on every target.
    def self.numeric_reference(digits, base)
      return -1 if digits == ""
      table = base == 16 ? "0123456789abcdef" : "0123456789"
      value = 0
      i = 0
      while i < digits.length
        d = table.index(digits[i, 1].to_s.downcase)
        return -1 if d.nil?
        value = value * base + d
        return -1 if value > 1114111
        i = i + 1
      end
      value
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

# `ActionText::ContentHelper` — the sanitizer Action Text runs rich text
# through, and the allow-lists it runs it with.
#
# WHY IT IS HERE AT ALL. campfire's presentation filter chain ends in
# `ContentFilters::SanitizeAttributes`, which asks for
# `ActionText::ContentHelper.sanitizer.class`, `.new`s it, and calls
# `.sanitize(html, tags:, attributes:)`. Without this the chain raised
# `uninitialized constant ActionText::ContentHelper` — and campfire wraps
# the whole chain in its own `rescue Exception` returning `""`, so every
# message body rendered EMPTY behind a 200. It was the second wall behind
# the class-side `new` one, found the same way.
#
# actiontext 8.0.5, `app/helpers/action_text/content_helper.rb`:
#
#   mattr_accessor(:sanitizer, default: Rails::HTML4::Sanitizer.safe_list_sanitizer.new)
#   mattr_accessor(:allowed_attributes)
#
# so `allowed_attributes` is nil until an app sets it, and the caller
# falls back to `sanitizer_class.allowed_attributes + Attachment::
# ATTRIBUTES`. Both halves are reproduced rather than guessed.
module ActionText
  module ContentHelper
    # The list Action Text sanitizes with — the sanitizer's own set plus
    # the attachment attributes, which is what `sanitizer_allowed_
    # attributes` computes in `actiontext/app/helpers/action_text/
    # content_helper.rb` and what campfire's `SanitizeAttributes` copies
    # verbatim as its `||` fallback.
    #
    # A DELIBERATE DIVERGENCE, ledgered in docs/pipeline/runtime.md:
    # Rails' `mattr_accessor(:allowed_attributes)` has no default, so
    # this reader answers nil in an app that never configured it, and
    # every caller reaches its own `||` fallback. We answer the list
    # that fallback computes.
    #
    # Why: the fallback goes through `sanitizer.class`, and a method
    # call on a CLASS OBJECT is dynamic for us and for spinel alike —
    # `Array[untyped]`, handed to a `sanitize` that takes
    # `Array[String]` twelve lines down. Answering nil made the two
    # descriptions of one list contradict, and the campfire binary
    # failed to LINK on it. The VALUES are identical either way, so no
    # app that uses the list can tell; only one that asks whether it is
    # nil can, and none does.
    def self.allowed_attributes
      SafeListSanitizer.allowed_attributes + Attachment::ATTRIBUTES
    end

    def self.sanitizer
      SafeListSanitizer.new
    end
  end

  # What `ContentHelper.sanitizer` answers, and what `.class.new` on it
  # builds again. Thin on purpose: the sanitizing itself belongs to
  # `ActionView::ViewHelpers`, which is where each lane already decides
  # whether it has a real safe-list sanitizer (the ruby family binds the
  # `rails-html-sanitizer` gem; the strict targets raise on markup and
  # say so). One implementation of that decision, not two.
  class SafeListSanitizer
    # rails-html-sanitizer 1.7.1's own default set, sorted as the gem
    # reports it. PORTED, not derived — the caller adds `class` to this
    # list and hands the result to the sanitizer, so a name missing here
    # is an attribute stripped from every message body.
    def self.allowed_attributes
      [
        "abbr", "alt", "cite", "class", "datetime", "height", "href",
        "lang", "name", "src", "title", "width", "xml:lang"
      ]
    end

    def sanitize(html, tags:, attributes:)
      ActionView::ViewHelpers.sanitize_allowing(html, tags, attributes)
    end
  end
end
