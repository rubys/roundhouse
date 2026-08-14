#!/usr/bin/env ruby
# frozen_string_literal: true
#
# In-process walk driver for an emitted campfire app. `scripts/campfire-walk`
# copies this file into the emitted tree and runs it from there.
#
# Why in-process and not Puma + curl: a crash is the POINT of this walk, and
# through a server every one of them looks the same — a 500 with no frame. Here
# the exception arrives with its backtrace, so a step reports the emitted file
# and line it died on. No Puma, no rack gem, no Playwright; the only thing
# between the walk and the app is `Main.run_rack`, which is what config.ru
# calls too.
#
# The walk is the milestone path (see project_campfire_first_milestone):
# sign up through /first_run, land in a room, post a message. It prints each
# step as it happens — a walk that dies on step 5 has still told you steps 1-4
# work, and that transcript IS the ledger.

require "stringio"
require "uri"

# Loading the app IS the first step of the walk, and the one that fails most
# often when a new gap appears: an unmodeled base class or a class-body call
# raises at `require`, before any request exists. Report it as a step rather
# than as a bare Ruby backtrace.
begin
  require_relative "main"
rescue Interrupt, SignalException, SystemExit
  raise
rescue Exception => e # rubocop:disable Lint/RescueException
  puts "\n  LOAD \e[31mCRASH\e[0m #{e.class}: #{e.message}"
  Array(e.backtrace).reject { |f| f.include?("/runtime/") }.first(3).each do |frame|
    puts "         #{frame}"
  end
  puts "\n\e[1;31mwalk stopped\e[0m — the emitted app does not load"
  exit 1
end

Main.configure_default_adapter!

# Ledger patches that OVERRIDE emitted app code — they have to run here,
# after main.rb has pulled in app/models, or the app's own definitions win.
# See scripts/campfire-walk-stubs.rb.
WALK_LATE_PATCHES.each(&:call) if defined?(WALK_LATE_PATCHES)

class Walk
  # `verb`/`path` name the request; `status` is the HTTP code or :crash.
  Step = Struct.new(:verb, :path, :status, :note, :location, :ms, :frames, keyword_init: true)

  HOST = "walk.test"

  def initialize
    @cookies = {}
    @steps = []
  end

  attr_reader :steps

  def get(path, accept: "text/html")
    request("GET", path, nil, accept)
  end

  def post(path, form, accept: "text/html")
    request("POST", path, form, accept)
  end

  # Follow up to `limit` redirects, recording each hop, and answer the path
  # finally landed on. The milestone path redirects three times before it
  # shows anything, so following is the difference between a walk that
  # reaches a room and one that stops at the front door.
  def follow(path, limit: 5)
    step = get(path)
    limit.times do
      break unless step.status.is_a?(Integer) && step.status.between?(300, 399)
      target = step.location
      break if target.nil? || target.empty?
      path = target.sub(%r{\Ahttps?://[^/]+}, "")
      step = get(path)
    end
    path
  end

  def any_crashes?
    @steps.any? { |s| s.status == :crash || (s.status.is_a?(Integer) && s.status >= 500) }
  end

  private

  def request(verb, path, form, accept)
    # Clock starts before anything that can raise, so a crashed step still
    # reports a duration rather than dying inside its own error handler.
    started = now_ms
    body = form && URI.encode_www_form(form)
    env = build_env(verb, path, body, accept)
    status, headers, chunks = Db.with_connection { Main.run_rack(env) }
    absorb_cookies(headers)
    location = headers["location"] || headers["Location"]
    record(verb, path, status, describe(location, chunks), location, now_ms - started, [])
  rescue Interrupt, SignalException, SystemExit
    raise
  rescue Exception => e # rubocop:disable Lint/RescueException
    # ScriptError (a missing constant's NotImplementedError stub, say) is not
    # a StandardError, and those are exactly the gaps this walk exists to
    # find — so catch everything the app can raise, not just StandardError.
    record(verb, path, :crash, "#{e.class}: #{e.message}", nil, now_ms - started, app_frames(e))
  end

  def build_env(verb, path, body, accept)
    request_path, _, query = path.partition("?")
    env = {
      "REQUEST_METHOD" => verb,
      "PATH_INFO" => request_path,
      "QUERY_STRING" => query,
      "HTTP_ACCEPT" => accept,
      "HTTP_HOST" => HOST,
      "SERVER_NAME" => HOST,
      "rack.input" => StringIO.new(body || ""),
    }
    env["HTTP_COOKIE"] = @cookies.map { |k, v| "#{k}=#{v}" }.join("; ") unless @cookies.empty?
    if body
      env["CONTENT_TYPE"] = "application/x-www-form-urlencoded"
      env["CONTENT_LENGTH"] = body.bytesize.to_s
    end
    env
  end

  # The jar is what makes this a SESSION rather than six unrelated requests:
  # /first_run signs the new user in by setting a cookie, and every step after
  # it is only reachable while that cookie rides along.
  def absorb_cookies(headers)
    raw = headers["set-cookie"] || headers["Set-Cookie"]
    return if raw.nil?
    Array(raw).each do |line|
      pair = line.split(";").first.to_s
      name, _, value = pair.partition("=")
      next if name.empty?
      if line.include?("Max-Age=0")
        @cookies.delete(name)
      else
        @cookies[name] = value
      end
    end
  end

  def describe(location, chunks)
    return "→ #{location}" if location
    bytes = Array(chunks).sum { |c| c.to_s.bytesize }
    "#{bytes} bytes"
  end

  # The app's own frames, innermost first. Runtime frames are dropped: a
  # NoMethodError raised inside `runtime/active_record.rb` is nearly always
  # the emitted caller's gap, and that caller is the line worth reporting.
  def app_frames(error, limit: 3)
    frames = Array(error.backtrace)
    own = frames.reject { |f| f.include?("/runtime/") || f.start_with?(__FILE__) }
    (own.empty? ? frames : own).first(limit)
  end

  def record(verb, path, status, note, location, ms, frames)
    step = Step.new(verb: verb, path: path, status: status, note: note,
                    location: location, ms: ms, frames: frames)
    @steps << step
    print_step(step)
    step
  end

  def print_step(step)
    code = step.status == :crash ? "CRASH" : step.status.to_s
    color = if step.status == :crash then 31
            elsif step.status.is_a?(Integer) && step.status >= 500 then 31
            elsif step.status.is_a?(Integer) && step.status >= 300 then 33
            else 32
            end
    printf("  %-4s %-34s \e[%dm%-5s\e[0m %s (%.1f ms)\n",
           step.verb, step.path, color, code, step.note, step.ms)
    step.frames.each { |f| puts "         #{f}" }
  end

  def now_ms
    Process.clock_gettime(Process::CLOCK_MONOTONIC) * 1000
  end
end

# ── the milestone path ────────────────────────────────────────────────

walk = Walk.new

puts "\n\e[1;34m==>\e[0m first run (fresh database, no account yet)"
walk.get "/"
walk.get "/first_run"
walk.post "/first_run", {
  "user[name]" => "Walker",
  "user[email_address]" => "walker@example.com",
  "user[password]" => "secret123",
}

puts "\n\e[1;34m==>\e[0m signed in — follow the root redirect into a room"
landed = walk.follow("/")

puts "\n\e[1;34m==>\e[0m the milestone: read the room, then post a message"
room = landed[%r{\A/rooms/\d+}]
if room.nil?
  # A record rendered into a URL whole (`/rooms/#<Room:0x…>`) is itself a
  # finding, and one the walk should REPORT rather than stop on: the room
  # steps behind it are what we came for. First room of a fresh campfire.
  puts "  root did not land on a usable room path (#{landed.inspect}) — " \
       "continuing with /rooms/1"
  room = "/rooms/1"
end

walk.get room
walk.post "#{room}/messages", {
  "message[body]" => "hello from the walk",
  "message[client_message_id]" => "walk-1",
}

crashed = walk.any_crashes?
puts "\n#{crashed ? "\e[1;31mwalk stopped\e[0m" : "\e[1;32mwalk complete\e[0m"} — " \
     "#{walk.steps.count { |s| s.status.is_a?(Integer) && s.status < 400 }}/#{walk.steps.size} steps answered"
exit(crashed ? 1 : 0)
