# The image processor, over ruby-vips: `require "vips"` is the
# spinel-ruby-vips spin package on the spinel tree (a subset of the gem
# over the system libvips, headerless carried C) and the gem itself on
# the CRuby tree — one surface, so this file is the same on both.
# Swapped in for runtime/active_storage_processor.rb by `project.rs`
# when the app declares variants; see that file for the other half.
require "vips"

# libvips 8.15+ skips BLOCKED classes in `vips_foreign_map`, so
# `vips_foreign_find_load` answers nil for a loader `block_untrusted` /
# `Vips.block` would refuse to run. 8.14 (Debian bookworm, Ubuntu jammy
# CI) still NAMES the blocked class — MagickFile for a BMP, SvgFile for
# an SVG — even after the operation itself raises "operation is blocked".
# A policy probe that asks find_load then sees a loader the process
# cannot run. Wrap the finder to the 8.15 contract.
#
# Reaching the C finder goes through `VipsExt.sp_vips_find_load`.
# Wrapping the Ruby method in place re-enters the wrapper on the
# spinel package and overflows. The package already binds that name;
# the gem does not, so the FFI stand-in below exists only there.
if !defined?(VipsExt)
  module VipsExt
    extend FFI::Library
    ffi_lib FFI.library_name("vips", 42)
    attach_function :sp_vips_find_load, :vips_foreign_find_load, [:string], :string
    attach_function :sp_vips_find_load_buffer, :vips_foreign_find_load_buffer, [:pointer, :size_t], :string
  end
end

module Vips
  # Class-name prefixes of loaders libvips tags UNTRUSTED. `vips -l`
  # prints the same list; a host that never built one simply never
  # returns that name from find_load. Prefix, not equality: find_load
  # answers the leaf (`VipsForeignLoadMagickFile`) and `Vips.block`
  # takes the parent (`VipsForeignLoadMagick`).
  UNTRUSTED_LOADER_PREFIXES = %w[
    VipsForeignLoadMagick
    VipsForeignLoadSvg
    VipsForeignLoadPdf
    VipsForeignLoadOpenslide
    VipsForeignLoadFits
    VipsForeignLoadMat
    VipsForeignLoadNifti
    VipsForeignLoadJp2k
    VipsForeignLoadJxl
    VipsForeignLoadCsv
    VipsForeignLoadRaw
    VipsForeignLoadVips
    VipsForeignLoadAnalyze
    VipsForeignLoadPpm
    VipsForeignLoadRad
    VipsForeignLoadOpenexr
    VipsForeignLoadDcraw
  ]

  def self.__rh_vips_loader_hidden?(name)
    return true if name.nil? || name == ""
    if Rails.application.vips_block_untrusted
      UNTRUSTED_LOADER_PREFIXES.each do |prefix|
        return true if name.start_with?(prefix)
      end
    end
    Rails.application.vips_blocked_operations.each do |op|
      return true if name == op || name.start_with?(op)
    end
    false
  end

  def self.vips_foreign_find_load(path)
    r = VipsExt.sp_vips_find_load(path)
    return nil if r == "" || __rh_vips_loader_hidden?(r)
    r
  end

  def self.vips_foreign_find_load_buffer(bytes, size)
    r = VipsExt.sp_vips_find_load_buffer(bytes, size)
    return nil if r == "" || __rh_vips_loader_hidden?(r)
    r
  end
end

module ActiveStorage
  class Processor
    # image_processing's vips pipeline for the transformations a
    # variation carries: `resize_to_limit` is `thumbnail(w, h, size:
    # :down)` — fit within, never enlarge — and `format` is the
    # encoder suffix. One libvips pipeline per output, which is also
    # the gem's shape (its lazy thumbnail is read once).
    def self.transform(data, content_type, variation)
      suffix = "." + variation.output_format(content_type)
      if variation.resize?
        Vips::Image.thumbnail_buffer(data, variation.width, height: variation.height, size: :down).write_to_buffer(suffix)
      else
        Vips::Image.new_from_buffer(data, "").write_to_buffer(suffix)
      end
    end
  end
end

# The app's loader policy (`config/initializers/vips.rb`: `Vips
# .block_untrusted(true)`, `Vips.block("VipsForeignLoadOpenslide",
# true)`), lifted at ingest onto the Application reopen and applied
# here, once, process-wide — which is what the initializer does.
Vips.block_untrusted(true) if Rails.application.vips_block_untrusted
Rails.application.vips_blocked_operations.each do |name|
  Vips.block(name, true)
end
