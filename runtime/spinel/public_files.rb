# frozen_string_literal: true
# The files under public/ that Rails' ActionDispatch::Static serves ahead
# of the routes, by request path. FileHandler tries the path itself, then
# with `.html`, then its `index.html`, so `/about` is `public/about.html`
# and `/docs/` is `public/docs/index.html`.
#
# The file set is listed once, at the first request, rather than stat'ed
# per request: the stat was ~5% of a `/up` request on the bench box, paid
# by every route. A page the app writes later through the page cache
# (runtime/action_controller_page_caching.rb) is added as it is written
# and removed as it is expired. A path the set names is confirmed on disk
# before it is served, so a file another worker expired, or one expired
# while this one was being written, falls through to the routes instead of
# answering 404. When the page cache writes under public/, a miss also
# looks at the disk, which is how a page another worker cached is found.
module ActionController
  module PublicFiles
    def self.files
      if @files.nil?
        @files = {}
        Dir.glob("public/**/*", File::FNM_DOTMATCH).each do |path|
          ActionController::PublicFiles.add(path["public/".length, path.length]) if File.file?(path)
        end
      end
      @files
    end

    # The file a GET/HEAD for `path` is answered with, as a decoded path
    # under public/ (FileHandler unescapes the request path first, and the
    # page cache writes the decoded name), or "" for none.
    def self.lookup(path)
      decoded = ActionDispatch::Router.decode_capture(path)
      return "" if decoded.include?("..") || decoded.include?("\0")
      stem = decoded.chomp("/")
      found = ActionController::PublicFiles.present(decoded)
      found = ActionController::PublicFiles.present(stem + ".html") if found.empty? && !stem.empty?
      found = ActionController::PublicFiles.present(stem + "/index.html") if found.empty?
      found
    end

    # `file` when it is on disk and the set names it, or the page cache
    # writes under public/ (another worker's page is then found on disk);
    # else "".
    def self.present(file)
      files = ActionController::PublicFiles.files
      if files.key?(file)
        return file if File.file?("public" + file)
        files.delete(file)
        return ""
      end
      return "" unless ActionController::PublicFiles.watch_disk?
      return "" unless File.file?("public" + file)
      files[file] = true
      file
    end

    # Whether a page can appear under public/ while the app runs: the page
    # cache is on and writes there. Only then does a miss look at the disk,
    # so an app that does not cache into public/ stats nothing per route.
    def self.watch_disk?
      if @watch_disk.nil?
        public_dir = File.expand_path("public")
        cache_dir = File.expand_path(ActionController::Base.page_cache_directory)
        @watch_disk = ActionController::Base.perform_caching &&
          (cache_dir == public_dir || cache_dir.start_with?(public_dir + "/"))
      end
      @watch_disk
    end

    # Forget what the first lookup decided, after the settings change (the tests).
    def self.reset_watch!
      @watch_disk = nil
      nil
    end

    def self.add(rel)
      ActionController::PublicFiles.files["/" + rel] = true
      nil
    end

    def self.remove(rel)
      ActionController::PublicFiles.files.delete("/" + rel)
      nil
    end

    # `disk` relative to public/, or "" when it lies outside it.
    def self.relative(disk)
      root = File.join(File.expand_path("public"), "")
      return "" unless disk.start_with?(root)
      disk[root.length, disk.length - root.length]
    end
  end
end
