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
      # `User.new(attrs)`, not `User.from_params`: the call site is
      # `FirstRun.create!(self.user_params.to_attrs)` — an unmodeled callee
      # is handed plain attrs, not the typed *Params object whose
      # `name_provided` flags `from_params` reads. The attrs hash is the
      # same one the emitted `initialize` takes (`password` included),
      # and `User.create!(user_params)` is what campfire itself writes.
      administrator = User.new(user_params)
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

# RETIRED: `ActionDispatch::Request#user_agent` / `#remote_ip`. Both runtime
# Request classes read them off the env now, and this stub was answering
# CONSTANTS over the top of working code — `reject_banned_ip` saw 127.0.0.1
# no matter what the caller sent, so campfire's ban tests could not fail and
# could not pass. A stub that outlives its gap does not go quiet; it lies.

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
# message fails `Creator must exist`. Milestone walk finding #6. The
# association scope itself works (`@room.messages.create_with_attachment!`
# carries `room_id` through `where_scope` / `scope_attributes`), so this
# lambda is the whole of what is missing.
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

# GAP: A VIEW HELPER THAT READS A CONTROLLER IVAR. campfire's `link_to_edit_room(room)` ignores its own argument and
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

# RETIRED, all six on 2026-08-23, and the diff of this file over that
# session IS the progress report the header promises. Each was a
# LOAD-time wall — a class body, an include, a superclass — so none was
# a route that 500s; each was a tree that did not boot, which is why
# they had to fall before any benchmark could name a subject.
#
#   * `PlatformAgent` — the gem is a Gemfile line after all
#     (`project.rs::RUNTIME_GEMS`). It needed ActiveSupport's
#     `Module#delegate` and nothing else; supplying that one method is
#     thirty lines (`runtime/spinel/module_delegate.rb`).
#   * `ActiveModel::Model` — synthesized for a LIBRARY class by
#     `lower::active_model_model`, the twin of the tableless-model path
#     that already did it for anything under `app/models/`.
#   * `ActionView::Helpers::SanitizeHelper` — its members qualify to
#     `ActionView::ViewHelpers.<name>`, and `strip_tags` is real now
#     (24 of 25 probes agree with `Rails::HTML5::FullSanitizer`).
#   * `Sound::Image` — a superclass EXPRESSION is carried now: the
#     anonymous `Struct.new(...)` gets a name.
#   * `Enumerable#index_by` — grounded to `ActiveSupport.index_by(list)`.
#   * `Turbo::Streams::StreamName` — `runtime/turbo_streams.rb`, the
#     third end of the `--unsigned` wire the other two already shared.
#
# And a seventh, a LATE PATCH rather than a constant: the
# `Message.with_attachment_details` override. The macro-generated
# preload scopes it stood in for (`with_attached_attachment`,
# `with_rich_text_body_and_embeds`) are in the scope registry now, so
# the emitted method threads its relation instead of dropping it — the
# bug that made `/rooms/1` serve room 5's messages.
#
# THE DELETIONS ARE NOT OPTIONAL BOOKKEEPING. A stub that outlives its
# gap is not inert: `class Image < Struct.new(...)` here, against the
# emit's `class Image < Sound::ImageStruct`, is a superclass mismatch
# TypeError at splice time — the whole file fails to load and the suite
# reports 0 of 240 rather than one broken test.

# RETIRED: the `sentry-ruby` gem. `project.rs::RUNTIME_GEMS` declares it
# in the emitted Gemfile now, from the `Sentry` constant `app/` names,
# and the CRuby tree's `gem_facades.rb` guarded-requires the real gem —
# which loads. A gem the app depends on is a Gemfile line, not a stub.
