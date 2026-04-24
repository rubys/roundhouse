module ActiveRecord
  # Phase-1 stubs — record calls for test inspection. Target-native
  # Turbo integration is a Phase-3+ concern.
  module Broadcasts
    def self.included(base)
      base.extend(ClassMethods)
    end

    module ClassMethods
      def broadcast_declarations
        @broadcast_declarations ||= []
      end

      def inherited(subclass)
        super
        subclass.instance_variable_set(:@broadcast_declarations, broadcast_declarations.dup)
      end

      def broadcasts_to(channel = nil, target: nil, inserts_by: nil, &block)
        broadcast_declarations << {
          channel: channel || block,
          target: target,
          inserts_by: inserts_by
        }
      end
    end

    def broadcast_replace_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :replace, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_append_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :append, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_prepend_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :prepend, record: self, channels: channels, target: target }
      nil
    end

    def broadcast_remove_to(*channels, target: nil)
      ActiveRecord::Broadcasts.log << { action: :remove, record: self, channels: channels, target: target }
      nil
    end

    def self.log
      @log ||= []
    end

    def self.reset_log
      @log = []
    end
  end
end
