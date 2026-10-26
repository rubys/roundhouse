//! An app's headers reach tep's case-sensitive Hash under tep's spelling,
//! so a response the app already gzipped (campfire's cached sidebar, whose
//! headers a cache handed back downcased) is not gzipped a second time.

use std::process::Command;

const SCRIPT: &str = r#"
require File.expand_path("runtime/spinel/tep/tep_core", Dir.pwd)
Req = Struct.new(:verb, :req_headers)
Res = Struct.new(:streaming, :upgrading_ws, :file_path, :status, :body, :headers)

html = (1..200).map { |i| "<p>#{i * 7919 % 10007}</p>" }.join
app_headers = { "content-encoding" => "gzip", "vary" => "Accept, Accept-Encoding", "etag" => "W/\"x\"" }
res = Res.new(false, false, "", 200, Zlib.gzip(html), Tep.str_hash)
res.headers["Content-Type"] = "text/html; charset=utf-8"
app_headers.each { |k, v| res.headers[Tep.header_name(k)] = v }
Tep.maybe_gzip!(Req.new("GET", { "accept-encoding" => "gzip" }), res)

encodings = res.headers.select { |k, _| k.downcase == "content-encoding" }
raise "one Content-Encoding: #{res.headers.inspect}" unless encodings == { "Content-Encoding" => "gzip" }
raise "gzipped once" unless Zlib.gunzip(res.body) == html
raise "an unmanaged header keeps its name" unless res.headers["etag"] == "W/\"x\""
puts "ALL OK"
"#;

#[test]
fn an_app_gzipped_response_is_not_gzipped_again() {
    let output = Command::new("ruby")
        .args(["-e", SCRIPT])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("ruby available");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("ALL OK"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}
