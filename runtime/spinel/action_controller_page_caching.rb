# frozen_string_literal: true
# actionpack-page_caching 1.2.5 (`ActionController::Caching::Pages`), for
# the ruby family and spinel: a cached page is the response body written
# under `page_cache_directory`, for a web server in front of the app to
# answer the next request with.
#
# - `caches_page :a, :b, if: …` is lowered to the `after_action` the gem's
#   macro registers, calling `cache_page(nil, nil, gzip)` (src/lower);
# - nothing is written or expired unless `perform_caching`;
# - the instance form writes only a GET or HEAD answered 200, at the
#   request path, with the response's format as the extension;
# - a path that would leave the directory once expanded is not written.
#
# `page_cache_directory` is the app's `config.action_controller.
# page_cache_directory` for the environment (the app's `public/` unless
# assigned), inherited through the controller hierarchy like the gem's
# class_attribute. A Proc or Symbol directory is refused at ingest.
#
# NOT HERE: the Hash forms of `cache_page`/`expire_page` (`action:`
# options through `url_for`); the String and request-path forms are the
# gem's own and the ones lowered.
module ActionController
  class PageCache
    def initialize(cache_directory, default_extension)
      @cache_directory = cache_directory
      @default_extension = default_extension
    end

    def expire(path)
      ActiveSupport::Notifications.instrument("expire_page.action_controller", { path: path }) do
        delete(cache_path(path, nil))
        0
      end
      nil
    end

    def cache(content, path, extension, gzip)
      ActiveSupport::Notifications.instrument("write_page.action_controller", { path: path }) do
        write(content, cache_path(path, extension), gzip)
        0
      end
      nil
    end

    def cache_file(path, extension)
      name = if path.empty? || path.delete("/").empty?
        "/index"
      else
        URI::DEFAULT_PARSER.unescape(path.chomp("/"))
      end
      return name unless File.extname(name).empty?
      name + (extension.nil? ? @default_extension : extension)
    end

    # Nil when the expanded path is outside the directory (`/../x`), which the gem skips.
    def cache_path(path, extension)
      directory = File.expand_path(@cache_directory)
      normalized = File.expand_path(File.join(directory, cache_file(path, extension)))
      return nil unless normalized.start_with?(File.join(directory, ""))
      normalized
    end

    def delete(path)
      return nil if path.nil?
      File.delete(path) if File.exist?(path)
      File.delete(path + ".gz") if File.exist?(path + ".gz")
      rel = ActionController::PublicFiles.relative(path)
      ActionController::PublicFiles.remove(rel) unless rel.empty?
      nil
    end

    def write(content, path, gzip)
      return nil if path.nil?
      FileUtils.makedirs(File.dirname(path))
      File.open(path, "wb+") { |f| f.write(content) }
      unless gzip.nil?
        Zlib::GzipWriter.open(path + ".gz", gzip) { |f| f.write(content) }
      end
      rel = ActionController::PublicFiles.relative(path)
      ActionController::PublicFiles.add(rel) unless rel.empty?
      nil
    end
  end

  class Base
    def self.page_cache_directory
      return @page_cache_directory if @page_cache_directory_assigned
      return Rails.application.action_controller_page_cache_directory if self == ActionController::Base
      superclass.page_cache_directory
    end

    def self.page_cache_directory=(directory)
      @page_cache_directory = directory
      @page_cache_directory_assigned = true
    end

    def self.page_cache_compression
      return @page_cache_compression if @page_cache_compression_assigned
      return nil if self == ActionController::Base
      superclass.page_cache_compression
    end

    def self.page_cache_compression=(level)
      @page_cache_compression = level
      @page_cache_compression_assigned = true
    end

    def self.page_cache
      ActionController::PageCache.new(page_cache_directory, ".html")
    end

    def self.expire_page(path)
      page_cache.expire(path) if perform_caching
      nil
    end

    # `gzip` is the gem's level: an Integer, or nil/false for none.
    def self.cache_page(content, path, extension = nil, gzip = Zlib::BEST_COMPRESSION)
      level = gzip == false ? nil : gzip
      page_cache.cache(content, path, extension, level) if perform_caching
      nil
    end

    def page_cache_directory
      self.class.page_cache_directory
    end

    def caching_allowed?
      (request.get? || request.head?) && response.status == 200
    end

    def expire_page(path)
      self.class.page_cache.expire(path) if self.class.perform_caching
      nil
    end

    def cache_page(content = nil, path = nil, gzip = Zlib::BEST_COMPRESSION)
      return nil unless self.class.perform_caching && caching_allowed?
      media = response.media_type
      symbol = media.empty? ? nil : Mime::Type.lookup(media).symbol
      extension = symbol.nil? ? nil : "." + symbol.to_s
      level = gzip == false ? nil : gzip
      self.class.page_cache.cache(content.nil? ? response.body : content, path.nil? ? request.path : path, extension, level)
      nil
    end
  end
end
