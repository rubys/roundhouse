# An attribute value that is an ARRAY, for the ruby family: Rails'
# `tag_option` joins it with spaces, and for `class:` first runs
# `build_tag_values` — its conditional-class form, where a String is
# itself and a Hash contributes the keys whose value is truthy. MEASURED
# against Rails 8.1: nil and "" are dropped, nothing is deduplicated
# (`["direct", "direct"]` renders both), and an empty list is
# `class=""`. The shared `ActionView::ViewHelpers.attr_value_text`
# (runtime/ruby) renders every value with `to_s` and says why the rest
# lives here: a walk over an untyped Array is not a shape every strict
# emitter answers.
#
# The site is campfire's `link_to_room(room, **attributes, &)`, which
# forwards `class: [ "direct", unread: membership.unread? ]` to the
# runtime's `render_attrs` — every room link in the sidebar.
#
# Required by BOTH boots (the spinel scaffold's and the CRuby overlay's)
# after the chain that defines the shared method, and `walk_dir_flat`
# copies it into every tree under runtime/attr_value_text.rb.
#
# Not modelled: a NESTED Array inside the class list (Rails flattens
# it); no corpus site nests one, and it renders through `to_s`.
module ActionView
  module ViewHelpers
    def self.attr_value_text(name, v)
      return v.to_s unless v.is_a?(Array)
      tokens = []
      v.each do |item|
        if item.is_a?(Hash)
          item.each do |ik, iv|
            tokens << ik.to_s unless iv.nil? || iv.to_s == "false"
          end
        elsif !item.nil?
          tokens << item.to_s unless item.to_s == ""
        end
      end
      tokens.join(" ")
    end
  end
end
