class FormatRouteProbe
  def self.paths
    [
      pwa_manifest_path,
      pwa_manifest_path(format: :json),
      pwa_manifest_path(format: "json"),
      pwa_manifest_path(format: :xml),
      plain_path(format: :json),
      string_manifest_path(format: :json),
      string_manifest_path(format: :xml),
      scoped_path(7, format: :json),
      scoped_path(7, format: :xml),
      scoped_path(7, format: :json, page: 2),
      scoped_path(7, format: :xml, page: 2)
    ]
  end
end
