//! `OpenSSL::Random.random_bytes` — the openssl library's on the ruby
//! family, spinel's `packages/openssl` on spinel — must use entropy, even
//! after srand.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::{Command, Output};

// ruby/ruby-bench d771f81f, benchmarks/lobsters/extras/utils.rb:2-15.
// Short IDs and session/reset tokens consume this one-byte rejection loop.
const UTILS: &str = r#"class Utils
  def self.random_str(len)
    str = ""
    while str.length < len
      chr = OpenSSL::Random.random_bytes(1)
      ord = chr.unpack1('C')
      if (ord >= 48 && ord <= 57) || (ord >= 65 && ord <= 90) || (ord >= 97 && ord <= 122)
        str += chr
      end
    end
    return str
  end
end
"#;

const CONTRACT: &str = r#"require_relative "app/models/utils"
require_relative "runtime/secure_random_stub"

def assert_binary_bytes(bytes, length)
  raise "wrong byte length for #{length}: #{bytes.bytesize}" unless bytes.bytesize == length
  raise "wrong encoding for #{length}: #{bytes.encoding.name}" unless bytes.encoding.name == "ASCII-8BIT"
  raise "invalid binary encoding for #{length}" unless bytes.valid_encoding?
end

# The test reopen stubs renderers, not the package's random_bytes primitive.
SecureRandom.stub_alphanumeric("fixed")
SecureRandom.stub_uuid("fixed")

srand(12345)
first = OpenSSL::Random.random_bytes(32)
srand(12345)
second = OpenSSL::Random.random_bytes(32)
raise "random bytes repeated after srand" if first == second

# Cover empty, single-draw, and multi-draw results, including both sides of
# successive native chunk boundaries, and a draw nested in another call.
[0, 1, 16, 32, 255, 256, 257, 511, 512, 513, 1024].each do |length|
  assert_binary_bytes(OpenSSL::Random.random_bytes(length), length)
end

assert_binary_bytes(OpenSSL::Random.random_bytes(OpenSSL::Random.random_bytes(1).bytesize + 256), 257)

begin
  OpenSSL::Random.random_bytes(-1)
  raise "negative length was accepted"
rescue ArgumentError
end

[6, 32, 60].each do |length|
  token = Utils.random_str(length)
  raise "wrong token length" unless token.bytesize == length
  token.each_byte do |byte|
    unless (byte >= 48 && byte <= 57) || (byte >= 65 && byte <= 90) || (byte >= 97 && byte <= 122)
      raise "non-alphanumeric token byte"
    end
  end
end

puts first.unpack1("H*")
"#;

fn utils_tree() -> emit_and_run::Emitted {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("extras/utils.rb", UTILS)
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("contract.rb"), CONTRACT).expect("write consumer");
    emitted
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_twice(mut command: Command) {
    let first = command.output().expect("run consumer");
    let second = command.output().expect("run consumer again");
    for output in [&first, &second] {
        assert_success(output);
        let hex = String::from_utf8_lossy(&output.stdout);
        assert_eq!(hex.trim().len(), 64, "{hex}");
        assert!(hex.trim().bytes().all(|b| b.is_ascii_hexdigit()), "{hex}");
    }
    assert_ne!(
        first.stdout, second.stdout,
        "random bytes repeated across seeded processes"
    );
}

#[test]
fn openssl_random_uses_entropy_and_preserves_binary_strings() {
    let emitted = utils_tree();
    let aliases = r#"
RandomBytes = OpenSSL::Random
[0, 1, 16, 32, 255, 256, 257, 511, 512, 513, 1024].each do |length|
  assert_binary_bytes(RandomBytes.random_bytes(length), length)
end
begin
  RandomBytes.random_bytes(-1)
  raise "negative length was accepted through a consumer alias"
rescue ArgumentError
end
"#;
    std::fs::write(emitted.join("contract.rb"), format!("{CONTRACT}{aliases}"))
        .expect("write Ruby consumer alias");
    let mut command = Command::new("ruby");
    command.arg("contract.rb").current_dir(&emitted);
    run_twice(command);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn openssl_random_uses_entropy_in_a_native_binary() {
    let emitted = utils_tree();
    let compiler = std::env::var("SPINEL").unwrap_or_else(|_| "spinel".into());
    let compiled = Command::new(compiler)
        .args(["--jobs=1", "--rbs", ".", "contract.rb", "-o", "contract"])
        .current_dir(&emitted)
        .output()
        .expect("compile consumer with Spinel");
    assert_success(&compiled);
    let mut command = Command::new(emitted.join("contract"));
    command.current_dir(&emitted);
    run_twice(command);
}
