module ActiveRecord
  # Broadcasts log holder. The instance methods (`broadcast_replace_to`
  # etc.) live on `Base` — inlined rather than mixed in via a module
  # so the body-typer can resolve `self.class.X` cross-method calls
  # against Base's class methods. This class keeps the per-process
  # log state and reset hook used by tests.
  class Broadcasts
    def self.log
      @log ||= []
    end

    def self.reset_log
      @log = []
    end
  end
end
