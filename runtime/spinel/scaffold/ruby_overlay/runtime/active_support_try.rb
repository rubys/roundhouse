# CRuby-only ActiveSupport `Object#try` / `NilClass#try`.
#
# `try` is ActiveSupport's nil-tolerant dispatch
# (active_support/core_ext/object/try.rb); this reopens Object/NilClass
# exactly as ActiveSupport does. Lives on the CRuby overlay: the shared
# runtime is statically resolvable and transpiled (no built-in reopening
# there), and strict targets will lower `.try(:m)` to their native
# safe-navigation form when lobsters comes up on them. Lobsters leans on
# it heavily (~30 call sites, `@user.try(:is_moderator?)` etc.); the
# block-only `try { ... }` form is not in the corpus and stays out.
class Object
  def try(method_name, *args, &block)
    public_send(method_name, *args, &block) if respond_to?(method_name)
  end

  def try!(method_name, *args, &block)
    public_send(method_name, *args, &block)
  end
end

class NilClass
  def try(*args)
    nil
  end

  def try!(*args)
    nil
  end
end
