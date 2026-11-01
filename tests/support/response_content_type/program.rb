# frozen_string_literal: true
require "json"
rows = []
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/json", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["json", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/plain", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["plain", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/html", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["html", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/vendor_json", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["vendor_json", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/json_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["json_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/head", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["head", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/head_csv", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["head_csv", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/send_csv", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["send_csv", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/send_binary", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["send_binary", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/redirect", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["redirect", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/plain_csv", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["plain_csv", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/plain_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["plain_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/head_json", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["head_json", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/head_no_content", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["head_no_content", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/send_json", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["send_json", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_symbol_type", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_symbol_type", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_trailing_parameter", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_trailing_parameter", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_quoted", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_quoted", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_trailing_space", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_trailing_space", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_uppercase_value", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_uppercase_value", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_uppercase_key", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_uppercase_key", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_no_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_no_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_empty_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_empty_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_other_parameter", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_other_parameter", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_duplicate_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_duplicate_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/inherited_boundary_before", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["inherited_boundary_before", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_quoted_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_quoted_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_trailing_parameter", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_trailing_parameter", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_uppercase_value", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_uppercase_value", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_uppercase_key", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_uppercase_key", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_empty_charset", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_empty_charset", status, headers["content-type"]]
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/new_other_parameter", "QUERY_STRING" => "", "HTTP_HOST" => "example.org", "rack.input" => StringIO.new(""))
rows << ["new_other_parameter", status, headers["content-type"]]
puts JSON.generate(rows)
