//! One `Vary: Accept` contract for the interpreted and native lanes
//! (`emit_and_run/vary_accept.rs`, `spinel_toolchain/vary_accept.rs`).
//!
//! Rails 7.1+ answers a render with `Vary: Accept` when the response
//! format came from the Accept header: no `format` param (path
//! extension, `?format=`, route default), and an Accept that is not a
//! browser's list with `*/*` beside other types, or an XHR. A redirect
//! or a `head` does not render and carries none. Each case below is
//! what Rails 8.1.4 answered.

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/vary-probe/away\", to: \"vary_probes#away\"\n  get \"/vary-probe/bare\", to: \"vary_probes#bare\"\n",
        )
        .write(
            "app/controllers/vary_probes_controller.rb",
            "class VaryProbesController < ApplicationController\n  def away\n    redirect_to \"/articles\"\n  end\n\n  def bare\n    head :ok\n  end\nend\n",
        )
}

pub const BROWSER_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// `(path, request headers, whether the response carries Vary: Accept)`.
pub const CASES: &[(&str, &[(&str, &str)], bool)] = &[
    ("/articles", &[("Accept", "*/*")], true),
    ("/articles", &[("Accept", "text/html")], true),
    ("/articles", &[("Accept", BROWSER_ACCEPT)], false),
    ("/articles", &[], false),
    ("/articles.json", &[("Accept", "application/json")], false),
    ("/articles?format=html", &[("Accept", "text/html")], false),
    ("/articles", &[("X-Requested-With", "XMLHttpRequest"), ("Accept", "text/html, */*")], true),
    ("/vary-probe/away", &[("Accept", "text/html")], false),
    ("/vary-probe/bare", &[("Accept", "text/html")], false),
];

/// The Ruby lane: the same cases through `Main.run_rack`.
pub fn assertions() -> String {
    let mut script = String::from(
        "def vary(path, headers)\n  p, q = path.split(\"?\", 2)\n  env = { \"REQUEST_METHOD\" => \"GET\", \"PATH_INFO\" => p, \"QUERY_STRING\" => q.to_s,\n          \"HTTP_HOST\" => \"localhost\", \"rack.input\" => StringIO.new(\"\") }\n  headers.each { |k, v| env[\"HTTP_\" + k.upcase.tr(\"-\", \"_\")] = v }\n  status, h, _ = Main.run_rack(env)\n  raise \"#{path} status #{status}\" unless status < 400\n  h[\"vary\"]\nend\n",
    );
    for (path, headers, varies) in CASES {
        let pairs: Vec<String> = headers.iter().map(|(k, v)| format!("{k:?} => {v:?}")).collect();
        let expected = if *varies { "\"Accept\"" } else { "nil" };
        script.push_str(&format!(
            "got = vary({path:?}, {{ {} }})\nraise \"{path} {}: vary #{{got.inspect}}\" unless got == {expected}\n",
            pairs.join(", "),
            pairs.join(", ").replace('"', "'"),
        ));
    }
    script.push_str("puts \"vary_accept contract passed\"\n");
    script
}
