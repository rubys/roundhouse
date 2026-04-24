module ActiveRecord
  # Instance-level broadcast helpers. Transpiled models call these
  # from their lifecycle hook overrides. Phase-1 stubs log the call
  # for test inspection; target-native Turbo integration is a later
  # concern.
  module Broadcasts
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
