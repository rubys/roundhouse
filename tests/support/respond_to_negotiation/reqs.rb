# [name, path, accept, extra_env]
TURBO = "text/vnd.turbo-stream.html, text/html, application/xhtml+xml"
REQS = []
add = ->(name, path, accept = nil, env = {}) { REQS << [name, path, accept, env] }
ACTS_HTML = %w[html_helper html_helper_redirect html_cond_redirect html_cond_head html_unless_render html_early_return html_rescue html_status]
ACTS_HTML.each do |a|
  [["html", "text/html"], ["turbo", TURBO], ["json", "application/json"], ["none", nil]].each do |l, acc|
    add.("#{a} #{l}", "/probes/#{a}", acc)
    add.("#{a} r #{l}", "/probes/#{a}?r=1", acc)
  end
end
%w[pre_assign seq_if top_if wants_param].each do |a|
  [["html", "text/html"], ["turbo", TURBO], ["json", "application/json"], ["none", nil]].each do |l, acc|
    add.("#{a} #{l}", "/probes/#{a}", acc)
    add.("#{a} ok #{l}", "/probes/#{a}?ok=1", acc)
  end
end
%w[multi_turbo].each { |a| add.("#{a} turbo", "/probes/#{a}", TURBO); add.("#{a} html", "/probes/#{a}", "text/html") }
# ?format= handling
%w[three json_only html_only plain_two].each do |a|
  ["json", "html", "turbo_stream", "bogus", "JSON", "", "xml", "js", "text", "all"].each do |f|
    [["none", nil], ["turbo", TURBO], ["json", "application/json"]].each do |l, acc|
      add.("#{a} ?format=#{f} #{l}", "/probes/#{a}?format=#{f}", acc)
    end
  end
  add.("#{a} ?format[]=json", "/probes/#{a}?format[]=json", "text/html")
  add.("#{a} .json?format=turbo_stream", "/probes/#{a}.json?format=turbo_stream", "text/html")
  add.("#{a} .turbo_stream?format=json", "/probes/#{a}.turbo_stream?format=json", "text/html")
  add.("#{a} ?format=json&format=html", "/probes/#{a}?format=json&format=html", "text/html")
end
# Accept ranking
ACC = {
  "tie"        => "text/html;q=0.5, text/vnd.turbo-stream.html;q=0.5",
  "tie_rev"    => "text/vnd.turbo-stream.html;q=0.5, text/html;q=0.5",
  "dup_q"      => "text/vnd.turbo-stream.html;q=0.3, text/html;q=0.5, text/vnd.turbo-stream.html;q=0.9",
  "dup_q2"     => "text/html;q=0.9, application/json;q=0.5, text/html;q=0.1, application/json;q=0.95",
  "dup_tie"    => "text/html;q=0.5, application/json;q=0.5, text/html;q=0.5",
  "star_text"  => "text/*;q=0.5, application/json",
  "star_text2" => "text/html;q=0.1, text/*",
  "star_text3" => "application/json;q=0.2, text/*;q=0.9",
  "star_app"   => "application/*;q=0.5, text/vnd.turbo-stream.html;q=0.4",
  "star_app2"  => "application/*, text/html;q=0.1",
  "star_text_json" => "text/*, application/json",
  "zero_both"  => "text/vnd.turbo-stream.html;q=0, text/html;q=0",
  "zero_star"  => "*/*;q=0",
  "zero_one"   => "application/json;q=0, text/html;q=0.1",
  "star_low"   => "*/*;q=0.1",
  "star_mid"   => "text/html;q=0.1,*/*;q=0.5",
  "star_x"     => "application/json;q=0.1, */*;q=0.9",
  "star_png"   => "image/png, */*;q=0.5",
  "wild_same"  => "text/*, text/html;q=0.1",
  "turbo_hi"   => "text/html;q=0.5, text/vnd.turbo-stream.html;q=0.5, application/json;q=0.5",
  "unknown_mix"=> "foo/bar, application/json;q=0.2, baz/qux;q=0.9",
  "nl"         => "text/html,\n*/*",
  "xml_pair"   => "text/xml, application/xml;q=0.9, text/html;q=0.1",
  "js_json"    => "text/javascript, application/json;q=0.5",
  "text_plain" => "text/plain, text/html;q=0.5",
  "bigq"       => "application/json;q=2, text/html",
  "big"        => ("text/*," * 9000) + "application/json",
  "big2"       => ("application/*;q=0.3," * 3000) + "text/vnd.turbo-stream.html;q=0.5",
}
%w[three plain_two].each do |a|
  ACC.each { |l, acc| add.("#{a} acc #{l}", "/probes/#{a}", acc) }
end
# --- numbered/it params, arm guards, branches without arms, path extensions ---
ACC3 = [["html", "text/html"], ["turbo", TURBO], ["json", "application/json"], ["none", nil]]
%w[send_arm rescue_render shadow_local numbered it_param].each do |a|
  ACC3.each { |l, acc| add.("#{a} #{l}", "/probes/#{a}", acc) }
end
%w[branch_diff top_branch_diff if_noelse seq_if_noelse else_other_stmt].each do |a|
  ACC3.each do |l, acc|
    add.("#{a} #{l}", "/probes/#{a}", acc)
    add.("#{a} ok #{l}", "/probes/#{a}?ok=1", acc)
  end
end
%w[three plain_two].each do |a|
  %w[json html turbo_stream bogus JSON js all text none].each do |ext|
    ACC3.each { |l, acc| add.("#{a} .#{ext} #{l}", "/probes/#{a}.#{ext}", acc) }
  end
  %w[none NONE].each { |f| ACC3.each { |l, acc| add.("#{a} ?format=#{f} #{l}", "/probes/#{a}?format=#{f}", acc) } }
  ACC3.each { |l, acc| add.("#{a} ?format= #{l}", "/probes/#{a}?format=", acc) }
end

%w[any_two nested_named nested_rest nested_it].each do |a|
  ACC3.each { |l, acc| add.("#{a} #{l}", "/probes/#{a}", acc) }
end
