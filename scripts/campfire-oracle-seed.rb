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

if Message.none? && messages > 0
  ids = User.pluck(:id)
  Room.pluck(:id).each do |room_id|
    Message.insert_all((1..messages).map { |i|
      { room_id: room_id,
        creator_id: ids[i % ids.size],
        client_message_id: "seed-#{room_id}-#{i}",
        created_at: Time.now, updated_at: Time.now }
    })
  end
end

puts "seeded: accounts=#{Account.count} users=#{User.count} rooms=#{Room.count} " \
     "memberships=#{Membership.count} sessions=#{Session.count} messages=#{Message.count}"
