module ActiveRecord
  module Callbacks
    HOOKS = %i[
      before_validation after_validation
      before_save after_save
      before_create after_create
      before_update after_update
      before_destroy after_destroy
      after_commit
      after_create_commit after_update_commit after_destroy_commit after_save_commit
      after_touch
    ].freeze

    def self.included(base)
      base.extend(ClassMethods)
    end

    module ClassMethods
      def callbacks
        @callbacks ||= Hash.new { |h, k| h[k] = [] }
      end

      def inherited(subclass)
        super
        parent_cbs = callbacks
        sub_cbs = Hash.new { |h, k| h[k] = [] }
        parent_cbs.each { |k, v| sub_cbs[k] = v.dup }
        subclass.instance_variable_set(:@callbacks, sub_cbs)
      end

      HOOKS.each do |hook|
        define_method(hook) do |*method_names, &block|
          if block
            callbacks[hook] << block
          else
            method_names.each { |m| callbacks[hook] << m }
          end
        end
      end
    end

    def run_callbacks(hook)
      self.class.callbacks[hook].each do |cb|
        if cb.is_a?(Symbol)
          send(cb)
        else
          instance_exec(&cb)
        end
      end
    end
  end
end
