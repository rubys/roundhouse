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
puts JSON.generate(rows)
