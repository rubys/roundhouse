# scripts/campfire-oracle-seed.rb — the benchmark dataset, seeded through
# Rails so the emit can be handed the SAME sqlite file.
#
# Run by `scripts/campfire-oracle prepare` via `bin/rails runner`; kept as
# its own file rather than a heredoc so it is diffable and so the emit lane
# can point at the identical rows.
#
# Shaped after campfire's own config/environments/performance.rb (users,
# sessions with `"a"*19 + id`, rooms, memberships) but SMALLER and
# parameterised: their 10k users / 201 rooms is a socket fan-out fixture,
# and a request-throughput lane wants pages that render in a comparable
# time, not the largest DB it can build.
#
# INSERT_ALL, NOT create!, for the same reason Rails' own seeder does it:
# callbacks here would mint push notifications and broadcasts, and this is
# fixture construction, not traffic.
#
# WHICH MEANS THE RICH TEXT HAS TO BE WRITTEN BY HAND. `has_rich_text
# :body` hangs the body off an `after_save` callback, so an `insert_all`
# message has NO `action_text_rich_texts` row and `message.body.body` is
# nil. That is not a missing nicety — it is the message's CONTENT, and
# for a while this file produced 500 empty ones:
#
#   * campfire's `message_presentation` rescues the resulting
#     `DelegationError` per message, so the reference server raised,
#     Sentry-captured and logged FORTY exceptions per `/rooms/1` — real
#     work charged to Rails that no other lane does;
#   * both lanes rendered message SHELLS, so the benchmark measured
#     everything about a message except the expensive part (Action Text
#     to HTML, the ContentFilters chain, `auto_link`);
#   * and the emitted tree issued 40 rich-text queries per page that
#     returned nothing.
#
# Rails stores the body column verbatim (measured: `m.body = "Hello
# <b>there</b>"` writes exactly that), so the rows go in beside the
# messages.

users    = Integer(ENV.fetch("CF_USERS", 50))
rooms    = Integer(ENV.fetch("CF_ROOMS", 5))
messages = Integer(ENV.fetch("CF_MESSAGES", 100))

Account.create!(name: "Campfire") if Account.none?

if User.none?
  digest = User.new(password: "secret123456").password_digest
  User.insert_all((1..users).map { |i|
    { name: "User #{i}",
      role: i == 1 ? :administrator : :member,
      email_address: "user#{i}@example.com",
      password_digest: digest }
  })
  # The token shape is campfire's own (performance.rb): 19 filler chars plus
  # a zero-padded id, so a token is derivable from a user id in either lane.
  Session.insert_all(User.pluck(:id).map { |id|
    { user_id: id, user_agent: "campfire-oracle", ip_address: "127.0.0.1",
      last_active_at: Time.now, token: "a" * 19 + id.to_s.rjust(5, "0") }
  })
end

if Room.none?
  creator = User.first.id
  # Rooms::Open, so every user is a member and /rooms/N renders the same
  # page for any seeded session — a closed room would make the result
  # depend on which user the load generator picked.
  Room.insert_all((1..rooms).map { |i|
    { name: "Room #{i}", creator_id: creator, type: "Rooms::Open" }
  })
  ids = User.pluck(:id)
  Room.pluck(:id).each do |room_id|
    Membership.insert_all(ids.map { |uid| { room_id: room_id, user_id: uid } })
  end
end

# Message bodies, cycled by index so a run is reproducible and so the
# render path sees the spread a real room has: one-liners, a couple of
# paragraphs, inline markup, and URLs for `auto_link` to find. Stored as
# the HTML Action Text keeps in the column.
BODIES = [
  "Morning, all.",
  "Deploy is green — <b>3.4.10</b> on all boxes.",
  "Anyone seen the flaky test on <code>messages_controller_test</code>? " \
    "It only fails when the suite runs in parallel, which makes me think " \
    "it is the fixture loader and not the assertion.",
  "https://example.com/runbooks/rollback has the steps",
  "👍",
  "Pushed a fix. The short version is that the scope dropped its " \
    "relation, so the page was querying the whole table and paging in " \
    "Ruby. Every tag count matched, which is why nobody caught it.",
  "Lunch?",
  "Numbers from the run: <i>142 req/s</i> before, 151 after. Not nothing.",
  "Re-reading https://sqlite.org/queryplanner.html for the third time today",
  "That is a good catch, thank you.",
  "Standup in five.",
  "The whole thing is one missing index. I will open a PR after lunch and " \
    "we can argue about the column order there.",
]

if Message.none? && messages > 0
  ids = User.pluck(:id)
  # DISTINCT, INCREASING timestamps. Every message used to be stamped
  # `Time.now`, which made `scope :ordered, -> { order(:created_at) }` a
  # sort over 500 equal keys — so "the last 40 messages" was whichever 40
  # the sort happened to surface, and two runtimes sorting the same rows
  # were not guaranteed to agree. One second apart, oldest first.
  base = Time.now - (rooms * messages)
  Room.pluck(:id).each_with_index do |room_id, r|
    Message.insert_all((1..messages).map { |i|
      t = base + (r * messages) + i
      { room_id: room_id,
        creator_id: ids[i % ids.size],
        client_message_id: "seed-#{room_id}-#{i}",
        created_at: t, updated_at: t }
    })
  end
  # The rich text, keyed back by the client_message_id the insert above
  # set — `insert_all` does not hand back ids portably, and re-deriving
  # them from a count would break the first time this file seeds into a
  # non-empty table.
  now = Time.now
  Message.pluck(:id, :client_message_id).each_slice(500) do |slice|
    ActionText::RichText.insert_all(slice.map { |id, cmid|
      i = cmid.split("-").last.to_i
      { record_type: "Message", record_id: id, name: "body",
        body: BODIES[i % BODIES.size], created_at: now, updated_at: now }
    })
  end
end

puts "seeded: accounts=#{Account.count} users=#{User.count} rooms=#{Room.count} " \
     "memberships=#{Membership.count} sessions=#{Session.count} " \
     "messages=#{Message.count} rich_texts=#{ActionText::RichText.count}"
