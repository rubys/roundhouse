# `ActionText::Content#to_json` on spinel — the VALUE half of a rich
# text's two answers, the way the CRuby overlay's
# `action_view_safe_buffer.rb` states it: `to_s` RENDERS (through the
# app's content layout, `<div class="trix-content">`), and the JSON
# form is the fragment itself. campfire's webhook payload pins the
# split — `{ body: { html: message.body.body } }.to_json` is asserted
# as `"First post!"`, unwrapped, on the same content the page renders
# inside the div.
#
# Spinel's bundled `json` serialises an object through its own
# `#to_json` when the class defines one, and through `#to_s` otherwise
# (packages/json/sp_json.c) — so without this every webhook payload
# carried the wrapper and the stub asserting the body never matched.
# The shared runtime's `as_json` already answers the fragment; this is
# the encoder asking it. Spinel-only: the overlay carries its own
# `to_json(*args)` for the stdlib gem's calling convention.
class ActionText::Content
  def to_json
    "\"" + JsonBuilder.encode_string(as_json) + "\""
  end
end
