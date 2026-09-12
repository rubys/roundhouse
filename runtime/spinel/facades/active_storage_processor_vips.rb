# The image processor, over ruby-vips: `require "vips"` is the
# spinel-ruby-vips spin package on the spinel tree (a subset of the gem
# over the system libvips, headerless carried C) and the gem itself on
# the CRuby tree — one surface, so this file is the same on both.
# Swapped in for runtime/active_storage_processor.rb by `project.rs`
# when the app declares variants; see that file for the other half.
require "vips"

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
