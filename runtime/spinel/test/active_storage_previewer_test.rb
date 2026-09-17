# Minitest-shaped, like hash_to_query_test.rb: a CRuby run of the ruby
# family's `ActiveStorage::Previewer.poster` reopen
# (runtime/active_storage_previewer.rb over the shared
# runtime/active_storage), the ffmpeg half of a video's `Preview`.
#
# The video is DRAWN here rather than kept as a fixture: ffmpeg's
# `lavfi` source makes a one-second red 32x24 clip in a few
# milliseconds, and a binary fixture in the runtime tree would have
# no reader. Skipped, not failed, where ffmpeg is absent — the
# previewer's own answer there is the raise the last test pins.
require "minitest/autorun"
require "tmpdir"
require_relative "test_helper"
require_relative "../runtime/active_storage_previewer"

class ActiveStoragePreviewerTest < Minitest::Test
  FFMPEG = system("ffmpeg", "-version", out: File::NULL, err: File::NULL)

  def with_clip
    path = File.join(Dir.tmpdir, "roundhouse-previewer-#{$$}.mp4")
    system("ffmpeg", "-y", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=red:s=32x24:d=1",
           "-pix_fmt", "yuv420p", path)
    yield path
  ensure
    File.delete(path) if File.exist?(path)
  end

  def test_poster_is_a_png_of_the_clips_first_frame
    skip "ffmpeg not installed" unless FFMPEG
    with_clip do |path|
      png = ActiveStorage::Previewer.poster(path)
      assert_equal "\x89PNG".b, png.byteslice(0, 4)
      # IHDR: width and height, big-endian, at bytes 16..23.
      assert_equal [32, 24], png.byteslice(16, 8).unpack("N2")
      refute File.exist?(path + ".poster.png"), "the temp poster is removed"
    end
  end

  def test_a_file_that_is_not_a_video_raises_rather_than_answering_bytes
    skip "ffmpeg not installed" unless FFMPEG
    path = File.join(Dir.tmpdir, "roundhouse-previewer-#{$$}.txt")
    File.write(path, "not a video")
    e = assert_raises(RuntimeError) { ActiveStorage::Previewer.poster(path) }
    assert_match(/could not draw a poster/, e.message)
  ensure
    File.delete(path) if File.exist?(path)
  end
end
