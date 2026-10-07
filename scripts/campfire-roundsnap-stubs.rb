# frozen_string_literal: true
#
# Minimal load-time stubs for `scripts/campfire-roundsnap`.
# Prefer fixing gaps in `runtime/ruby/` (they re-emit into ISeq). This
# file is a safety net for --reuse emits that predate a runtime fix, and
# for any remaining NameError walls we deliberately ledger here.
#
# Required AFTER `runtime/active_storage` (or equivalent) has loaded and
# BEFORE the rest of `Loader.boot!` pulls in `app/models`.

module ActiveStorage
  class Previewer
    # Rails: ActiveStorage::Previewer::VideoPreviewer. Campfire subclasses
    # it. runtime/ruby/active_storage.rb now defines this; reopen is a
    # no-op when the constant already exists.
    class VideoPreviewer
    end unless const_defined?(:VideoPreviewer, false)
  end
end if defined?(ActiveStorage) && ActiveStorage.const_defined?(:Previewer)
