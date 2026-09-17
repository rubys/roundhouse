# `ActiveStorage::Previewer.poster` over ffmpeg — the ruby family's
# reopen of the shared definition (runtime/ruby/active_storage.rb),
# which raises. Rails' `Previewer::VideoPreviewer` draws the poster
# with this exact command; campfire's own Dockerfile installs ffmpeg
# for it, and so does the archive's.
#
# The filter is Rails' verbatim (actionstorage's `video_preview.rb`):
# the first frame, or the first keyframe, or the first scene change
# past 1.5%, whichever comes first — so a video that opens black gets
# the frame a viewer would recognise, and the frame this draws is the
# one a Rails process would have drawn for the same file.
#
# A tree without ffmpeg raises here, as Rails raises (`Errno::ENOENT`
# out of `IO.popen`): the honest answer is the missing program, not a
# blank poster. `system` rather than a pipe, and a temp file rather
# than stdout, because both ruby-family runtimes have `system` and
# `File.binread` and neither needs more.
module ActiveStorage
  class Previewer
    FILTER = "select=eq(n\\,0)+eq(key\\,1)+gt(scene\\,0.015),loop=loop=-1:size=2,trim=start_frame=1"

    def self.poster(path)
      out = path + ".poster.png"
      ok = system("ffmpeg", "-y", "-loglevel", "error", "-i", path, "-vf", FILTER, "-frames:v", "1", "-f", "image2", out)
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
