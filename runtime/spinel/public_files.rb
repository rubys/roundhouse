# frozen_string_literal: true
# The files under public/ that Rails' ActionDispatch::Static serves ahead
# of the routes, by request path. FileHandler tries the path itself, then
# with `.html`, then its `index.html`, so `/about` is `public/about.html`
# and `/docs/` is `public/docs/index.html`.
#
# Listed once, at the first request, rather than stat'ed per request:
# the stat was ~5% of a `/up` request on the bench box, paid by every
# route. A page the app writes later through the page cache
# (runtime/action_controller_page_caching.rb) is added here as it is
# written and removed as it is expired, which is the one way a running
# app puts a file under public/.
module ActionController
  module PublicFiles
    def self.table
      if @table.nil?
        @table = {}
        Dir.glob("public/**/*", File::FNM_DOTMATCH).each do |path|
          ActionController::PublicFiles.add(path["public/".length, path.length]) if File.file?(path)
        end
      end
      @table
    end

    # The URL path of the file a GET/HEAD for `path` is answered with, or "" for none.
    def self.lookup(path)
      return "" if path.include?("..")
      ActionController::PublicFiles.table.fetch(path, "")
    end

    def self.add(rel)
      file = "/" + rel
      table = ActionController::PublicFiles.table
      table[file] = file
      if file.end_with?("/index.html")
        dir = file[0, file.length - "index.html".length]
        table[dir] = file unless table.key?(dir)
        stem = dir.chomp("/")
        table[stem] = file unless stem.empty? || table.key?(stem)
      elsif file.end_with?(".html")
        stem = file[0, file.length - ".html".length]
        table[stem] = file unless table.key?(stem)
      end
      nil
    end

    def self.remove(rel)
      file = "/" + rel
      table = ActionController::PublicFiles.table
      table.delete_if { |_, v| v == file }
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
