# `Module#delegate` — ActiveSupport's, for the GEMS in the emitted
# Gemfile, not for app code.
#
# CRuby/JRuby only. The shared scaffold base drops this file and the two
# ruby-family forks in `project.rs` push it back, because a reopen of
# `Module` that defines methods from a computed name is exactly what the
# strict targets cannot compile.
#
# WHY IT EXISTS AT ALL. An app's own `delegate :x, to: :y` is lowered at
# ingest (`src/ingest/delegate.rs`) into real methods, so nothing in the
# emitted app needs this. A GEM's is not: `platform_agent`'s second line
# is `delegate :browser, :version, :product, :os, to: :user_agent`, and
# the gem declares `activesupport` as a runtime dependency for that one
# call. Pulling ActiveSupport into the emitted tree to satisfy it would
# be the opposite of what the tree is for; supplying the one method it
# names is thirty lines. MEASURED: with this defined, the real
# `platform_agent` gem loads and answers correctly.
#
# Rails' own implementation `module_eval`s a generated method per name
# so the delegation costs nothing at call time; this one closes over the
# target instead, which is the same semantics at a small dispatch cost —
# and every consumer here is a gem's cold path.
#
# Faithful on the four keywords Rails accepts, because a partial
# implementation of a name this well-known is worse than none: a gem
# using `prefix:` would silently get methods under the wrong names.
class Module
  def delegate(*methods, to: nil, prefix: nil, allow_nil: nil, private: nil)
    raise ArgumentError, "Delegation needs a target. Supply a keyword argument 'to'" if to.nil?

    location = case prefix
    when nil, false then ""
    when true then "#{to}_"
    else "#{prefix}_"
    end

    methods.each do |method|
      name = :"#{location}#{method}"
      define_method(name) do |*args, **kwargs, &block|
        # `send`, not `public_send`: Rails delegates to private readers
        # routinely, and platform_agent's `user_agent` IS private.
        target = send(to)
        if target.nil? && allow_nil
          nil
        else
          target.send(method, *args, **kwargs, &block)
        end
      end
      send(:private, name) if private
    end
  end
end
