//! Vendored extras that spinel's own packages now cover reach the spinel
//! tree as written. An HTTP client over `net/https`, `resolv`, `ipaddr`
//! and `timeout` (lobsters' Sponge) compiles against spinel's bundled
//! packages, and an image compositor over `flatten` / `resize` /
//! `insert` (lobsters' StoryImage) against spinel-ruby-vips 0.3.0, which
//! the manifest pins. Neither is swapped for a raising façade.

use std::path::Path;

use roundhouse::ingest::ingest_app;
use roundhouse::project::{target_files, BuildTarget};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

const SPONGE: &str = r#"require "uri"
require "net/https"
require "resolv"
require "ipaddr"

class Sponge
  MAX_DNS_TIME = 5

  def fetch(url)
    uri = URI.parse(url)
    ips = []
    Timeout.timeout(MAX_DNS_TIME) do
      ips = Resolv.getaddresses(uri.host)
    end
    ip = IPAddr.new(ips.first)
    host = Net::HTTP.new(ip.to_s, uri.port)
    host.get(uri.path == "" ? "/" : uri.path)
  rescue Net::HTTPBadResponse
    nil
  end
end
"#;

const STORY_IMAGE: &str = r#"class StoryImage
  def add_logo(image_data, logo_path)
    logo = Vips::Image.new_from_file(logo_path)
    image = Vips::Image.new_from_buffer(image_data, "")
    image = image.flatten(background: [255, 255, 255]) if image.bands > 3
    logo = logo.resize((image.height / 5.0) / logo.height) if logo.height > image.height / 5
    image.insert(logo, 0, image.height - logo.height).write_to_buffer(".png")
  rescue Vips::Error
    nil
  end
end
"#;

fn spinel_files(test: &str) -> Vec<(String, String)> {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let dir = std::env::temp_dir().join(format!("roundhouse-extras-verbatim-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["app", "config", "db"] {
        copy_dir(&fixture.join(sub), &dir.join(sub));
    }
    std::fs::create_dir_all(dir.join("extras")).unwrap();
    std::fs::write(dir.join("extras/sponge.rb"), SPONGE).unwrap();
    std::fs::write(dir.join("app/models/story_image.rb"), STORY_IMAGE).unwrap();
    let app = ingest_app(&dir).expect("ingest");
    let files = target_files(&app, &dir, BuildTarget::Spinel).expect("spinel files");
    let _ = std::fs::remove_dir_all(&dir);
    files
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{path} missing")).1
}

#[test]
fn an_http_client_over_the_bundled_packages_is_emitted_as_written() {
    let files = spinel_files("http");
    let sponge = file(&files, "app/models/sponge.rb");
    assert!(sponge.contains("Resolv.getaddresses"), "{sponge}");
    assert!(sponge.contains("Net::HTTP.new"), "{sponge}");
    assert!(!sponge.contains("GemFacade.fail!"), "{sponge}");
}

#[test]
fn a_vips_compositor_is_emitted_as_written_against_the_pinned_package() {
    let files = spinel_files("vips");
    let story_image = file(&files, "app/models/story_image.rb");
    assert!(story_image.contains(".insert("), "{story_image}");
    assert!(!story_image.contains("GemFacade.fail!"), "{story_image}");
    let manifest = file(&files, "spin.toml");
    assert!(
        manifest.contains("ruby-vips = { git = \"https://github.com/roundhouse-rb/spinel-ruby-vips\", ref = \"v0.3.0\" }"),
        "{manifest}"
    );
}
