# ActiveRecord::ConnectionNotEstablished — named by `rescue_from
# ActiveRecord::ConnectionNotEstablished` (lobsters answers it with a
# plain-text 500). Nothing here raises it: the emitted `Db` opens its
# connections at boot, before any request. It exists because a rescue
# clause evaluates its class list whenever an exception passes through
# it, so an undefined name turns every OTHER error the action raises
# into a NameError that hides the real one.
#
# Ruby-family home (off the strict-target tables, unlike errors.rb),
# same reasoning as `ActionView::MissingTemplate`: exception classes as
# control flow ride the BeginRescue lowering, which the ruby-family
# targets and spinel AOT share.
module ActiveRecord
  class ConnectionNotEstablished < StandardError
  end
end
