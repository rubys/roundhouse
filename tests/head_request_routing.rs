//! Emitted-program regression for HEAD routing (kept out of
//! tests/emit_and_run.rs so concurrent appends there do not conflict).
//! Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// No `rails new` app declares a HEAD route, so `HEAD /` — what an
/// uptime monitor or a deploy proxy sends — matched nothing and 404'd.
/// Rails routes it as the GET it shadows; a route declared for HEAD
/// itself still wins, and a `via: :all` route is not taken ahead of an
/// earlier GET route.
#[test]
fn head_routes_as_get_unless_a_head_route_is_declared() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  match \"probe\" => \"articles#show\", via: :head, defaults: { id: \"1\" }\n  get \"probe\" => \"articles#index\"\n",
        )
        .run_ruby(r#"
def req(verb, path)
  out = StringIO.new
  Main.run({ "REQUEST_METHOD" => verb, "PATH_INFO" => path, "HTTP_ACCEPT" => "text/html" }, StringIO.new(""), out)
  out.string
end
root = req("HEAD", "/")
raise "HEAD /:\n#{root}" unless root.start_with?("Status: 200")
table = Main.route_table
m = ActionDispatch::Router.match("HEAD", "/probe", table)
raise "HEAD /probe: #{m&.action.inspect}, want the declared HEAD route" unless m && m.action.to_s == "show"
m = ActionDispatch::Router.match("GET", "/probe", table)
raise "GET /probe: #{m&.action.inspect}, want the GET route" unless m && m.action.to_s == "index"
puts "head"
"#)
        .assert_passes();
}
