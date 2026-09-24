# StoryImage façade — lobsters builds a story's social-card image by
# fetching the linked page (through Sponge, itself a raising façade on
# this tree), finding its og:image, and compositing the site logo onto
# it with ruby-vips (`flatten`, `resize`, `insert`). The spinel-ruby-vips
# package carries decode/encode/thumbnail but not those three operations,
# so the composite cannot compile. The scaffold base ships this stand-in
# at the same emit path, leaving the require graph untouched; the CRuby
# tree restores the verbatim emit (see
# emit::ruby::library::restore_extras_facades). The cache-path half is
# REAL — `path` and `exists?` are what the image controller serves from
# — and `generate` raises, which is where the Sponge fetch already
# stopped it. Real fix: flatten/resize/insert in spinel-ruby-vips, then
# drop this row.
require_relative "../../runtime/gem_facades"

class StoryImage
  CACHE_DIR = Rails.public_path.join("story_image").freeze

  def initialize(short_id_or_story)
    @short_id = if short_id_or_story.is_a?(Story)
      short_id_or_story.short_id
    else
      short_id_or_story
    end
  end

  def path
    CACHE_DIR.join("#{File.basename(@short_id)}.png")
  end

  def exists?
    path.exist?
  end

  def generate(url)
    GemFacade.fail!("StoryImage#generate")
    nil
  end
end
