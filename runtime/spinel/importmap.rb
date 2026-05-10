# Fallback Importmap module. The lowerer-emitted config/importmap.rb
# (generated per-app from the source's config/importmap.rb) reopens
# this module and overrides `pins` / `entry` with the app's actual
# values; for source apps without an importmap the lowerer skips
# that emission and these stubs stand.
#
# Without this fallback, spinel-AOT's static analyzer can't see
# Importmap as defined: the per-app file is required via begin/rescue
# (which spinel's analyzer doesn't follow as guaranteed), and any
# direct reference (`Importmap.pins` from the emitted layout) would
# also fail to resolve under spinel. Pre-defining the module here
# means the constant is always reachable; the per-app config just
# adjusts return values.
module Importmap
  def self.pins
    []
  end

  def self.entry
    "application"
  end
end
