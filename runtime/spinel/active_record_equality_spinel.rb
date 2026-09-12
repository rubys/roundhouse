# Rails record equality, for the compiled family: same class + same
# persisted id (AR core's `==`). Two hydrations of the same row are
# equal; a new record equals only itself.
#
# NOT in the shared `runtime/ruby/active_record/base.rb`, whose own note
# says why: the Ruby equality protocol (`==`/`eql?`/`hash`) has no
# cross-target analog and the one attempt to put it there broke the
# emit of targets with no `[Klass, @id].hash`. The CRuby overlay carries
# its copy in `active_record_bang.rb`; this is spinel's, a reopen of the
# base class that every model inherits (verified: a subclass value, a
# `User | nil` slot and `Array#include?` all reach it).
#
# Loaded from spinel's boot.rb only — `walk_dir_flat` copies every
# runtime/spinel/*.rb into every tree, and the ruby family never
# requires this one.
#
# campfire's `assert_equal @bot, boost.booster` and
# `User#can_administer?`'s `self == record&.creator` are the callers:
# the second compared a bot signed in by key against the message's
# creator loaded through the association — never the same object — so
# every bot update and destroy was a 403.
module ActiveRecord
  class Base
    def ==(other)
      return true if equal?(other)
      other.instance_of?(self.class) && persisted? && other.id == id
    end
  end
end
