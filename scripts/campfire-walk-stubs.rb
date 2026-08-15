# frozen_string_literal: true
#
# THE LEDGER. Every entry below stands for something the compiler does not
# model yet, spliced into the emitted campfire app ahead of `app/models` so
# `scripts/campfire-walk` can get past it and report what lies BEHIND it.
#
# Rules, and they are the whole point of keeping this file in the repo:
#
#   * One `# GAP:` comment per entry, naming what is missing and why the
#     compiler cannot produce it yet. The count is reported by the walk.
#   * A gap closed in the compiler is an entry DELETED here. The diff of this
#     file over time is the progress report.
#   * Never resolve a compiler bug by adding a stub. A stub is for something
#     deliberately out of scope (a gem we do not intend to transpile, a Rails
#     subsystem parked by design) or for a gap already ledgered elsewhere —
#     it buys the walk another few steps, it does not fix anything.
#   * `scripts/campfire-walk --no-stubs` walks without this file, which is
#     how you check whether an entry is still earning its place.
#
# TWO LOAD POINTS, and getting them backwards is a silent no-op:
#   * a stub for a MISSING constant has to exist BEFORE the app loads, so it
#     is written at the top level of this file (spliced ahead of `app/models`);
#   * a patch that OVERRIDES emitted app code has to run AFTER the app loads,
#     or the app's own definition simply replaces it. Those go in
#     `WALK_LATE_PATCHES`, which the driver calls once `main.rb` is loaded.
#
# Nothing here is compiler output and nothing here ships: it exists so a
# single walk reports the FULL list of walls instead of one per session.

# Patches applied after the emitted app has loaded; see the header.
WALK_LATE_PATCHES = []

# GAP: the `platform_agent` gem (Basecamp's User-Agent parser).
# campfire's ApplicationPlatform subclasses it. Out of scope by design —
# a gem we do not intend to transpile — so this answers the surface the
# subclass actually calls: `match?`, and a `user_agent` facade carrying
# `browser` / `platform`.
class PlatformAgent
  def initialize(user_agent = "")
    @user_agent = user_agent.to_s
  end

  def match?(pattern)
    @user_agent.match?(pattern)
  end

  # campfire writes `user_agent.browser` / `user_agent.platform`; the raw
  # string answers both well enough for a walk.
  def user_agent
    self
  end

  def browser
    @user_agent
  end

  def platform
    @user_agent
  end

  def os
    @user_agent
  end
end

# Action Text is MODELLED now — `has_rich_text`, `ActionText::RichText`,
# `ActionText::Content` and `rich_text_area` all lower (src/lower/
# rich_text.rs + runtime/ruby/action_text.rb), so `Content`,
# `Attachment.tag_name` and `Attachable` have left this file.
#
# The classes campfire REOPENS under the ActionText namespace
# (`lib/rails_ext/{filter,filters,actiontext_opengraph_embeds}.rb`) are
# emitted from the app's own source now — they used to be dropped, and
# the stand-ins that stood here with them are gone.
#
# GAP: `ActiveModel::Model`, which `ActionText::Attachment
# ::OpengraphEmbed` includes. Rails' module gives a keyword-ish
# `initialize(attributes = {})` plus validations; only the constructor
# is reached here.
module ActiveModel
  module Model
    def initialize(attributes = {})
      attributes.each { |name, value| instance_variable_set("@#{name}", value) }
    end
  end
end

# GAP: `ActionView::Helpers::SanitizeHelper` as an INCLUDABLE module.
# Opengraph::Metadata includes it to call `sanitize(strip_tags(title))` on a
# model. Both helpers are known to analyze as view-side functions; what is
# missing is the module a model can mix in. Deliberately crude here — the
# walk only needs the calls to answer, and a real implementation is Rails'
# sanitizer, not a regex.
module ActionView
  module Helpers
    module SanitizeHelper
      def sanitize(html)
        html.to_s
      end

      def strip_tags(html)
        html.to_s.gsub(/<[^>]*>/, "")
      end
    end
  end
end

# GAP: a superclass EXPRESSION is dropped, silently — `class Image <
# Struct.new(:asset_path, :width, :height)` emits as bare `class Image`, so
# its `super(...)` lands on BasicObject and the app dies at LOAD time.
# COMPILER BUG, already on the ledger (milestone walk finding #6); this is
# not a fix, it just declares the class first so the emitted body REOPENS it
# and keeps the Struct it should have had. Delete this the day the emitter
# carries the expression.
class Sound
  class Image < Struct.new(:asset_path, :width, :height)
  end
end

# GAP: `Enumerable#index_by`, an ActiveSupport core extension our shared
# runtime does not carry. campfire builds `Sound::INDEX` with it in a class
# body, so it fires at LOAD time. Small and genuinely ours to implement in
# runtime/ruby — this entry should be short-lived.
module Enumerable
  def index_by
    each_with_object({}) { |element, memo| memo[yield(element)] = element }
  end
end

# GAP ×2, both COMPILER gaps already on the ledger (milestone walk finding
# #6 and #3), patched together because they sit in one method:
#
#   * belongs_to AUTOSAVE — `room.creator = User.new(...)` then `room.save!`
#     saves the unsaved target first in Rails. We read `value.id` (0) and
#     the save fails `Creator must exist`, which is the first wall of the
#     whole walk.
#   * a `has_many … do … end` EXTENSION BLOCK is dropped with no diagnostic,
#     so `room.memberships.grant_to` does not exist anywhere in the emit.
#
# Rewritten here the way Rails would sequence it. This is app logic, not a
# library stand-in — the loudest kind of ledger entry, and the first two to
# delete.
WALK_LATE_PATCHES << lambda do
  class FirstRun
    def self.create!(user_params)
      Account.create!(name: ACCOUNT_NAME)
      room = Rooms::Open.new(name: FIRST_ROOM_NAME)
      administrator = User.from_params(user_params)
      administrator.role = 1
      administrator.save! # ← autosave would have done this
      room.creator = administrator
      room.save!
      # ← `memberships.grant_to administrator`, inlined
      Membership.create!(room_id: room.id, user_id: administrator.id,
                         involvement: room.default_involvement)
      administrator
    end
  end
end

# GAP: `ActionDispatch::Request#user_agent` and `#remote_ip`, absent from
# our runtime Request. The auth spine reads both — `start_new_session_for`
# records them on the Session row, `reject_banned_ip` filters on the IP — so
# no request that signs anybody in gets past this. Ours to implement, and
# the values are already in the Rack env.
module ActionDispatch
  class Request
    def user_agent
      "campfire-walk"
    end

    def remote_ip
      "127.0.0.1"
    end
  end
end

# GAP: association-loading scopes GENERATED by macros we do not model —
# `has_rich_text :body` generates `with_rich_text_body_and_embeds`,
# `has_one_attached :attachment` generates `with_attached_attachment`. The
# emitted `Message.with_attachment_details` calls both, so the room page dies
# before it renders a single message. Dropping them costs QUERIES, not rows
# (they are preloads), which is the same argument `scope_is_row_preserving`
# already makes about `includes`.
WALK_LATE_PATCHES << lambda do
  class Message
    def self.with_attachment_details(__rel = ActiveRecord::Relation.new(self))
      __rel
    end
  end
end

# GAP: Active Storage. `has_one_attached :logo` on Account and
# `has_one_attached :avatar` on User are not modeled, and the reader is
# absent — so `Current.account.logo.attached?` takes down the LAYOUT and
# rooms/show's nav, i.e. every page, not just upload flows. Milestone walk
# finding #8, which argues that a facade answering `attached? == false` is
# worth separating from variants and uploads. This is that facade, in the
# crudest form: nothing is ever attached.
WALK_LATE_PATCHES << lambda do
  not_attached = Object.new
  def not_attached.attached?
    false
  end
  # `process_attachment` runs on every message create and walks the
  # attachment even when there is none.
  def not_attached.analyze
    nil
  end

  def not_attached.video?
    false
  end

  def not_attached.representable?
    false
  end

  { Account => %i[logo], User => %i[avatar], Message => %i[attachment] }.each do |model, names|
    names.each do |name|
      next if model.method_defined?(name)
      model.define_method(name) { not_attached }
    end
  end
end

# GAP: `belongs_to :creator, class_name: "User", default: -> { Current.user }`
# — the DEFAULT LAMBDA is dropped, so nothing sets `creator_id` and every
# message fails `Creator must exist`. Milestone walk finding #6, and now the
# only thing standing between the walk and a posted message: the association
# scope itself works (`@room.messages.create_with_attachment!` carries
# `room_id` through `where_scope` / `scope_attributes`).
#
# Rails evaluates the lambda at INITIALIZATION; this stands in for it at
# validation time, which is close enough to measure what lies behind.
WALK_LATE_PATCHES << lambda do
  class Message
    alias_method :walk_orig_validate, :validate

    def validate
      self.creator_id = Current.user.id if @creator_id.nil? || @creator_id == 0
      walk_orig_validate
    end
  end
end

# GAP, and a NEW one this walk found: A VIEW HELPER THAT READS A CONTROLLER
# IVAR. campfire's `link_to_edit_room(room)` ignores its own argument and
# uses `@room` — legal in Rails, where a helper runs in the view's context
# and shares the controller's ivars. Our helpers lower to module functions
# with no ivar context at all, so `@room` is nil and the room page dies in
# its nav. The emit even passes `room` in; nothing connects the two.
WALK_LATE_PATCHES << lambda do
  module RoomsHelper
    def self.link_to_edit_room(room, &blk)
      ActionView::ViewHelpers.link_to(
        RouteHelpers.edit_room_path(room.id).to_s,
        { class: "btn", style: "view-transition-name: edit-room-#{room.id}",
          data: { room_id: room.id } }, &blk
      )
    end
  end
end

# GAP, NEW: `scope defaults: { user_id: "me" }` is not modeled. Rails' route
# DEFAULTS supply the segment, so campfire's views call
# `user_push_subscriptions_path` with no arguments at all and get
# `/users/me/push_subscriptions`. We emit the helper with a REQUIRED
# `user_id`, so the bare call raises ArgumentError from the room page's nav.
# The default belongs in the emitted signature.
WALK_LATE_PATCHES << lambda do
  module RouteHelpers
    def self.user_push_subscriptions_path(user_id = "me")
      "/users/#{user_id}/push_subscriptions"
    end
  end
end

# GAP: `pluck` on a folded association (`room.memberships.pluck(:user_id)`).
# Same family as `find_by!` above and `index_by` further up — an ActiveRecord
# /ActiveSupport collection method the folded Array does not answer.
class Array
  def pluck(*names)
    map do |record|
      values = names.map { |name| record.public_send(name) }
      names.one? ? values.first : values
    end
  end
end

# GAP: the `sentry-ruby` gem. campfire's message helpers rescue every
# Exception and report it before falling back to an "unrenderable"
# partial, so the constant is reached the moment ANY message render
# raises. Out of scope by design — a gem we do not intend to transpile.
module Sentry
  def self.capture_exception(_error, **_context)
    nil
  end
end
