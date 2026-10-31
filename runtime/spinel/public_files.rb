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
# answering 404; a page another worker cached is rendered here again.
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

    # The URL path of the file a GET/HEAD for `path` is answered with, or "" for none.
    def self.lookup(path)
      return "" if path.include?("..")
      stem = path.chomp("/")
      found = ActionController::PublicFiles.present(path)
      found = ActionController::PublicFiles.present(stem + ".html") if found.empty? && !stem.empty?
      found = ActionController::PublicFiles.present(stem + "/index.html") if found.empty?
      found
    end

    # `file` when the set names it and it is still on disk, else "".
    def self.present(file)
      files = ActionController::PublicFiles.files
      return "" unless files.key?(file)
      return file if File.file?("public" + file)
      files.delete(file)
      ""
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
