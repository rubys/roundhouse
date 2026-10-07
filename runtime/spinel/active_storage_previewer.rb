# `ActiveStorage::Previewer.poster` over ffmpeg — the ruby family's
# reopen of the shared definition (runtime/ruby/active_storage.rb),
# which raises. Rails' nested `Previewer::VideoPreviewer` is the
# inheritance target apps subclass; drawing still goes through this
# class-side `poster` with the same ffmpeg command Rails uses.
# Campfire's Dockerfile installs ffmpeg for it, and so does the archive's.
#
# The `-vf` filter comes from `ActiveStorage.video_preview_vf_filter`
# (Rails' `config.active_storage.video_preview_arguments`), so an app
# that adds `gte(t,5)` (campfire) draws the same frame Rails would.
# Default is Rails' select/keyframe/scene filter.
#
# A tree without ffmpeg raises here, as Rails raises (`Errno::ENOENT`
# out of `IO.popen`): the honest answer is the missing program, not a
# blank poster. `system` rather than a pipe, and a temp file rather
# than stdout, because both ruby-family runtimes have `system` and
# `File.binread` and neither needs more.
module ActiveStorage
  class Previewer
    def self.poster(path)
      out = path + ".poster.png"
      filter = ActiveStorage.video_preview_vf_filter
      ok = system("ffmpeg", "-y", "-loglevel", "error", "-i", path, "-vf", filter, "-frames:v", "1", "-f", "image2", out)
      if !ok || !File.exist?(out)
        raise "ActiveStorage::Previewer: ffmpeg could not draw a poster for " + path +
              " — is ffmpeg installed? (campfire's Dockerfile installs it)"
      end
      png = File.binread(out)
      File.delete(out)
      png
    end
  end
end
