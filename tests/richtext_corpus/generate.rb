# VENDORED from basecamp/once-campfire-rust (MIT), reference-tools/richtext/generate.rb at
# 080f9035141ac09e1db81476a1b229ed24f08d8f. Run by scripts/campfire-richtext-corpus with our Rails
# oracle: it builds the corpus database (users, a room, one stored body per case) and writes
# expected.json beside the database.
#
# Generates the expected outputs for crates/richtext/tests/corpus by running Campfire's real rich
# text pipeline. Run inside the campfire-reference image with reference-tools/richtext/run.sh.
#
# For every stored body in inputs.yml it records what Rails produces for:
#   presentation  MessagesHelper#message_presentation (rescued) and whether it raised
#   plain_text    message.body.to_plain_text
#   editable      the <lexxy-editor> value: editable_body + Lexxy's render_custom_attachments_in
#   mentioned     Message::Mentionee#mentioned_users (ids)
#   filtered      TextMessagePresentationFilters.apply(...).to_html, for debugging

require "yaml"
require "json"

INPUT = ENV.fetch("RICHTEXT_INPUT", "/corpus/inputs.yml")
OUTPUT = ENV.fetch("RICHTEXT_OUTPUT", "/corpus/expected.json")
DEFAULT_HOST = "once.campfire.test"

Rails.logger.level = :error
ActiveRecord::Base.logger = nil

def outcome
  { "ok" => yield }
rescue Exception => e
  cause = e.cause ? " (cause: #{e.cause.class.name})" : ""
  { "error" => e.class.name, "message" => (e.message.to_s.scrub[0, 300] + cause) }
end

# --- Records -------------------------------------------------------------------------------------

TIME = Time.utc(2024, 1, 2, 3, 4, 5)

def create_user(key, name, bio: nil)
  User.insert!({ name: name, email_address: "#{key}@example.com", password_digest: BCrypt::Password.create("secret123456", cost: 4),
    bio: bio, role: 0, status: 0, created_at: TIME, updated_at: TIME })
  User.find_by!(email_address: "#{key}@example.com")
end

users = {
  "david" => create_user("david", "David", bio: "Founder"),
  "jason" => create_user("jason", "Jason"),
  "kevin" => create_user("kevin", "Kevin", bio: "Ops"),
  "mallory" => create_user("mallory", "Mallory <b>&amp;</b> \"Evil\"", bio: "<script>alert(1)</script>"),
  "obrien" => create_user("obrien", "Seán O'Brien 🎉")
}
deleted = create_user("deleted", "Deleted")
deleted_sgid = deleted.attachable_sgid
deleted.delete

Room.insert!({ name: "Pets", type: "Rooms::Open", creator_id: users["david"].id, created_at: TIME, updated_at: TIME })
room = Room.first
creator = users["jason"]

def tampered(sgid) = "#{sgid.split("--").first}--invalid"

def rails7_sgid(user)
  marshaled = Base64.urlsafe_encode64(Marshal.dump(user.to_gid.to_s))
  payload = { "_rails" => { "message" => marshaled, "exp" => nil, "pur" => "attachable" } }
  "#{Base64.strict_encode64(JSON.generate(payload))}--invalidsignature"
end

def mention_attachment_for(user)
  body = ApplicationController.render partial: "users/mention", locals: { user: user }
  "<action-text-attachment sgid=\"#{user.attachable_sgid}\" content-type=\"application/vnd.campfire.mention\" content=\"#{body.gsub('"', '&quot;')}\"></action-text-attachment>"
end

def expand(body, users:, room:, deleted_sgid:)
  body = "<b>" * 400 + "deep" if body == "DEEP_400"
  body = "<b>" * 401 + "deep" if body == "DEEP_401"
  body = "<p " + (1..401).map { |i| "a#{i}=\"#{i}\"" }.join(" ") + ">many</p>" if body == "ATTRS_401"
  body
    .gsub(/\{\{sgid:(\w+)\}\}/) { users.fetch($1).attachable_sgid }
    .gsub(/\{\{mention:(\w+)\}\}/) { mention_attachment_for(users.fetch($1)) }
    .gsub(/\{\{tampered:(\w+)\}\}/) { tampered(users.fetch($1).attachable_sgid) }
    .gsub(/\{\{rails7:(\w+)\}\}/) { rails7_sgid(users.fetch($1)) }
    .gsub(/\{\{expired:(\w+)\}\}/) { users.fetch($1).to_sgid(expires_at: Time.utc(2020, 1, 1), for: ActionText::Attachable::LOCATOR_NAME).to_s }
    .gsub(/\{\{cross_purpose:(\w+)\}\}/) { users.fetch($1).to_sgid(expires_in: nil, for: "transfer").to_s }
    .gsub("{{tampered_room}}") { tampered(room.to_sgid(expires_in: nil, for: ActionText::Attachable::LOCATOR_NAME).to_s) }
    .gsub("{{deleted_sgid}}") { deleted_sgid }
end

# --- Pipeline ------------------------------------------------------------------------------------

# Renders the way a request to MessagesController does: Current.request set, and Action Text's
# renderer being the controller itself (ActionText::Engine's "action_text.renderer" around_action).
def with_request(host)
  request = ActionDispatch::Request.new(Rack::MockRequest.env_for("http://#{host}/"))
  controller = MessagesController.new
  controller.set_request!(request)
  controller.set_response!(ActionDispatch::Response.new)
  Current.set(request: request) do
    ActionText::Content.with_renderer(controller) { yield controller.view_context }
  end
end

def store_message(body, room:, creator:, index:)
  Message.insert!({ room_id: room.id, creator_id: creator.id, client_message_id: "corpus-#{index}", created_at: TIME, updated_at: TIME })
  message = Message.find_by!(client_message_id: "corpus-#{index}")
  # Bypass the attribute type, which would canonicalize the body: the column holds it as given
  ActiveRecord::Base.connection.raw_connection.execute(
    "INSERT INTO action_text_rich_texts (record_type, record_id, name, body, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    [ "Message", message.id, "body", body, TIME.iso8601, TIME.iso8601 ])
  Message.find(message.id)
end

# Seeded random bodies built from pieces the handwritten cases exercise one at a time
module Fuzz
  TAGS = %w[p div span a b strong i em s u mark code pre h1 h2 blockquote ul ol li table tr td th tbody figure figcaption
    img br hr script style iframe svg math textarea select option template noscript object form input button video source
    action-text-attachment font center marquee details summary sub sup del ins small]
  ATTRIBUTES = [
    %(href="https://example.com/a?b=1&amp;c=2"), %(href="javascript:alert(1)"), %(href=" jav&#x09;ascript:x"), %(href="/rooms/1"),
    %(href="data:text/html,x"), %(href="mailto:a@b.co"), %(src="https://example.com/i.png"), %(src="x" onerror="alert(1)"),
    %(onclick="x()"), %(style="color: red; background: url(javascript:x)"), %(class="og-embed mention"), %(data-controller="x"),
    %(data-language="ruby"), %(title="t > http://example.com/in-title"), %(title='q"uote'), %(lang="en"), %(id="i"),
    %(content-type="application/vnd.actiontext.opengraph-embed"), %(content-type="application/vnd.campfire.mention"),
    %(content-type="text/html" content="&lt;b&gt;inner&lt;/b&gt;"), %(content-type="image/png" url="https://example.com/p.png"),
    %(filename="Title" href="https://example.org/"), %(caption="Cap"), %(presentation="gallery"), %(width="10" height="20"),
    %(sgid="{{sgid:david}}"), %(sgid="{{tampered:jason}}"), %(sgid="{{rails7:kevin}}"), %(sgid="bad"), %(value="3")
  ]
  TEXTS = [ "hello", "https://example.com/x.", "www.example.com", "(https://example.com/(a))", "me@example.com",
    "&amp; &lt; &gt; &quot; &nbsp;", "😀 👍🏽", "שלום", "\n", "  ", "a\u00a0b", "https://x.com/dhh/status/1?s=2",
    "&#106;avascript:x", "<!-- c -->", "]]>", "&", "<", "http://example.com/&gt;", "{{mention:david}}", "x@y" ]

  def self.body(random, depth = 0)
    Array.new(random.rand(1..4)) do
      if depth > 3 || random.rand < 0.35
        TEXTS.sample(random: random)
      else
        tag = TAGS.sample(random: random)
        attributes = Array.new(random.rand(0..2)) { ATTRIBUTES.sample(random: random) }.join(" ")
        inner = body(random, depth + 1)
        closer = random.rand < 0.9 ? "</#{tag}>" : ""
        "<#{tag}#{" " unless attributes.empty?}#{attributes}>#{inner}#{closer}"
      end
    end.join
  end
end

# Seeded random edits of the handwritten cases
module Mutate
  TOKENS = [ "<", ">", "\"", "'", "&", "&#", "&#x", ";", "</", "/>", "<b>", "</b>", "<p>", "</p>", "<div>", "<svg>", "<math>",
    "<table>", "<td>", "<select>", "<a href=", "<!--", "-->", "<![CDATA[", "\n", "\r", "\t", "\u00a0", "\u0000", "javascript:",
    "http://", "https://x.com/", "www.", "@", "=", " ", "(", ")", "]", "--", "=\"", "action-text-attachment", "sgid=",
    "content-type=\"application/vnd.actiontext.opengraph-embed\"", "é", "😀", "\u202e", "%", "#", "?" ]

  def self.apply(body, random)
    body = body.dup
    random.rand(1..6).times do
      position = body.empty? ? 0 : random.rand(0..body.length)
      case random.rand(4)
      when 0, 1 then body.insert(position, TOKENS.sample(random: random))
      when 2 then body.slice!(position, random.rand(1..12))
      else body.insert(position, body[position, random.rand(1..40)].to_s)
      end
    end
    body
  end
end

random = Random.new(20240102)
handwritten = YAML.load_file(INPUT).fetch("cases")
fuzz_cases = Array.new(Integer(ENV.fetch("RICHTEXT_FUZZ_CASES", 400))) { |i| { "name" => "fuzz #{i}", "body" => Fuzz.body(random) } }
mutation_random = Random.new(20240103)
mutation_cases = Array.new(Integer(ENV.fetch("RICHTEXT_MUTATION_CASES", 400))) do |i|
  source = handwritten.sample(random: mutation_random)
  { "name" => "mutation #{i} of #{source["name"]}", "body" => source["body"], "host" => source["host"], "mutate" => true }
end

cases = (handwritten + fuzz_cases + mutation_cases).each_with_index.map do |c, index|
  body = expand(c.fetch("body"), users:, room:, deleted_sgid:)
  body = Mutate.apply(body, mutation_random) if c["mutate"]
  host = c["host"] || DEFAULT_HOST
  message = store_message(body, room:, creator:, index:)

  with_request(host) do |v|
    raw = outcome do
      v.auto_link ERB::Util.html_escape(ContentFilters::TextMessagePresentationFilters.apply(message.body.body)),
        html: { target: "_blank" }, sanitize_options: { tags: MessagesHelper::AUTO_LINK_ALLOWED_TAGS, attributes: MessagesHelper::AUTO_LINK_ALLOWED_ATTRIBUTES }
    end
    {
      "name" => c.fetch("name"),
      "host" => host,
      "body" => body,
      # message_presentation rescues and logs, but the log formatter itself raises on a message
      # with invalid UTF-8, and then the message renders as messages/unrenderable
      "presentation" => outcome { v.message_presentation(message).to_s },
      "presentation_raised" => raw["error"],
      "presentation_raised_message" => raw["message"],
      "plain_text" => outcome { message.body.to_plain_text },
      "editable" => outcome { value = v.send(:render_custom_attachments_in, v.editable_body(message)); value&.to_s },
      "mentioned" => outcome { message.send(:mentioned_users).map(&:id) },
      "filtered" => outcome { ContentFilters::TextMessagePresentationFilters.apply(message.body.body).to_html }
    }
  end
end

web_urls = %w[
  http://example.com/page https://example.com/image.png javascript:alert(1) data:text/html,pwned vbscript:msgbox(1)
  //example.com/image.png /rooms/1 rooms/1 https:/rooms/1 https:rooms/1 http:/rooms/1 https:// http://:80/rooms/1
  https://once.campfire.test/rooms/1 http://once.campfire.test/rooms/1 https://ONCE.Campfire.Test/rooms/1
  https://once.campfire.test./rooms/1 https://%6fnce.campfire.test/rooms/1 https://%77ww.example.com/x.png
  http://127.0.0.1/rooms/1 http://2130706433/rooms/1 http://0177.0.0.1/rooms/1 http://0x7f.0.0.1/rooms/1
  http://1.2.3.0xff/rooms/1 http://[::1]/rooms/1 http://localhost/rooms/1 https://203.0.113.10/image.png
  https://xn--80aswg.xn--p1ai/page HTTPS://EXAMPLE.COM/x https://user:pw@example.com:8080/p?q=1#f
  https://example.com/p?q=%zz https://example.com/p?q=%z https://example.com/p#frag#more https://example.com/a%20b
  https://exa_mple.com/ https://example.com./ https://example..com/ https://.example.com/ https://[v1.x]/
  ftp://example.com/x mailto:nobody mailto:a@b.com ldap://example.com/x#f https://.../x https://a.0x1/
  https://a.b1/ https://a.1b/ http://example.com:99999999999999999999/ https://ex%41mple.com/
] + ["", "   ", "http://exa mple.com/ ", "https://example.com/é", "https://example.com/\n"]

web_url_results = with_request(DEFAULT_HOST) do
  web_urls.map do |value|
    result = outcome { ActionText::Attachment::OpengraphEmbed.send(:web_url, value) }
    { "value" => value, "host" => DEFAULT_HOST, "result" => result }
  end
end

expected = {
  "generated_by" => "reference-tools/richtext/generate.rb",
  "request_host" => DEFAULT_HOST,
  "users" => users.values.map do |u|
    { "key" => users.key(u), "id" => u.id, "name" => u.name, "title" => u.title, "attachable_sgid" => u.attachable_sgid,
      "user_path" => ApplicationController.render(inline: "<%= user_path(u) %>", locals: { u: }),
      "avatar_path" => ApplicationController.render(inline: "<%= fresh_user_avatar_path(u) %>", locals: { u: }) }
  end,
  "rooms" => [ room.id ],
  # SGIDs whose signature verifies for the attachable purpose (and haven't expired)
  "signed" => users.values.map { |u| { "sgid" => u.attachable_sgid, "model" => "User", "id" => u.id, "exists" => true } } +
    [ { "sgid" => deleted_sgid, "model" => "User", "id" => deleted.id, "exists" => false } ],
  "cases" => cases,
  "web_urls" => web_url_results
}

File.write(OUTPUT, JSON.pretty_generate(expected) + "\n")
puts "Wrote #{cases.size} cases to #{OUTPUT}"
