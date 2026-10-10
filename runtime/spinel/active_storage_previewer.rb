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

    # Rails' `ActiveStorage::Analyzer::VideoAnalyzer`, its width and
    # height: ffprobe on the first video stream. campfire posts a video
    # with a poster only when its size is known (`too_many_pixels_to_
    # preview?` reads both), so a video with no dimensions was a video
    # with no poster. The bytes go to a file under the storage root
    # because ffprobe reads a path, and the answer comes back through
    # `-o`, for the same `system`-only reason as `poster`. No ffprobe, or
    # a stream it cannot read, answers `[0, 0]` — the metadata Rails
    # leaves empty when its analyzer fails. Rotation (Rails swaps width
    # and height for a 90/270 degree stream) is not read.
    PROBES = [0]

    def self.video_dimensions(data)
      PROBES[0] = PROBES[0] + 1
      service = ActiveStorage::Blob.service
      service.ensure_dir(service.root)
      path = service.root + "/.probe-" + Process.pid.to_s + "-" + PROBES[0].to_s
      out = path + ".txt"
      File.binwrite(path, data)
      ok = system("ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height",
                  "-of", "csv=s=x:p=0", "-o", out, path)
      dims = [0, 0]
      if ok && File.exist?(out)
        parts = File.read(out).strip.split("x")
        dims = [parts[0].to_i, parts[1].to_i] if parts.length >= 2
      end
      File.delete(out) if File.exist?(out)
      File.delete(path) if File.exist?(path)
      dims
    end
  end
end
