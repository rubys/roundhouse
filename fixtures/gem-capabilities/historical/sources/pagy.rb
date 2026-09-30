class SurveyProbe
  include Pagy::Method

  def last_page
    pagination, records = pagy(:offset, [11, 22, 33, 44, 55], limit: 2, page: 3, request: {base_url: "https://example.invalid", path: "/items", params: {}})
    [records, pagination.from, pagination.to, pagination.pages, pagination.next]
  end

  def overflow
    pagination, records = pagy(:offset, [11, 22, 33, 44, 55], limit: 2, page: 4, request: {base_url: "https://example.invalid", path: "/items", params: {}})
    records
  end
end
