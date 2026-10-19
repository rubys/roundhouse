# Rails' signed-value vectors under the SPINEL-compiled runtime: the
# verifier, signed ids and cookie codec the binary actually runs, over
# sp_crypto's HMAC and PBKDF2 (runtime/spinel/message_digest.rb) rather
# than CRuby's OpenSSL, and the encrypted cookie jar over spinel's
# openssl package.
#
# The harness (tests/rails_compat_vectors_spinel.rs) copies beside this
# file the runtime sources it requires, and the case file the CRuby
# driver writes with `--cases`: tab-separated hex fields, op first,
# expected last, "-" for nil and "=" for the empty string. Only clock-stable cases are in it — this
# binary's verifier reads the real clock.
require "json"
require_relative "base64"
require_relative "message_digest"
require_relative "message_verifier"
require_relative "message_encryptor"
require_relative "signed_id"
require_relative "url"

# What the runtime reads off the app: the secret, set from the case file.
module Rails
  class App
    attr_accessor :secret_key_base
    def initialize
      @secret_key_base = ""
    end
  end

  def self.application
    @app ||= App.new
  end
end

# tep's url.rb names Tep.str_hash in a reader this driver never calls.
module Tep
  def self.str_hash
    Hash.new("")
  end
end

def unhex(s)
  return "" if s == "="
  out = +""
  i = 0
  while i + 1 < s.length
    out << s[i, 2].to_i(16).chr
    i += 2
  end
  out.force_encoding("UTF-8")
end

def hex(s)
  out = +""
  i = 0
  while i < s.bytesize
    out << format("%02x", s.getbyte(i))
    i += 1
  end
  out
end

MV = ActionController::MessageVerifier

lines = File.read(ARGV[0] || "cases.tsv").split("\n")
secret = unhex(lines[0].split("\t")[1].to_s)
Rails.application.secret_key_base = secret

passes = {}
totals = {}
shown = 0
i = 1
while i < lines.length
  f = lines[i].split("\t")
  op = unhex(f[0].to_s)
  want_raw = f[f.length - 1].to_s
  a = f.length > 2 ? unhex(f[1].to_s) : ""
  b = f.length > 3 ? unhex(f[2].to_s) : ""
  c = f.length > 4 ? unhex(f[3].to_s) : ""
  got = "-"
  if op == "kg"
    len = b.to_i
    key = len == 64 ? MV.derive_key(secret, a) : MessageDigest.pbkdf2_sha256(secret, a, MV::ITERATIONS, len)
    got = hex(key)
  elsif op == "unescape"
    got = Tep::Url.unescape(a)
  elsif op == "sc_verify"
    v = MV.verified(secret, MV::SIGNED_COOKIE_SALT, b, "cookie." + a, true)
    got = v == "" ? "-" : v
  elsif op == "sc_envelope"
    got = MV.envelope(secret, MV::SIGNED_COOKIE_SALT, b, "cookie." + a, c, true)
  elsif op == "sid_generate"
    got = ActiveRecord::SignedId.generate(a.to_i, b, 0)
  elsif op == "sid_verify"
    id = ActiveRecord::SignedId.verified_id(a, b)
    got = id == 0 ? "-" : id.to_s
  elsif op == "sgid_generate"
    got = MV.gid_envelope(secret, "signed_global_ids", a, b, c)
  elsif op == "sgid_verify"
    json = MV.verified_data_json(secret, "signed_global_ids", a, b, true)
    got = json == "" ? "-" : MV.json_value(json)
  elsif op == "as_generate"
    got = MV.gid_envelope(secret, "ActiveStorage", a, b, c)
  elsif op == "blob_verify"
    json = MV.verified_data_json(secret, "ActiveStorage", a, "blob_id", true)
    got = json == "" ? "-" : json
  elsif op == "json_encode"
    got = MV.json_string(a)
  elsif op == "json_decode"
    got = MV.json_value(a)
  elsif op == "as_data"
    json = MV.verified_data_json(secret, "ActiveStorage", a, b, true)
    got = json == "" ? "-" : json
  elsif op == "ec_verify"
    # The jar answers nil for a message that is not JSON (a Marshal one).
    json = ActionController::MessageEncryptor.decrypted_json(secret, b, "cookie." + a)
    got = json == "" ? "-" : json
    begin
      JSON.parse(json) if got != "-"
    rescue StandardError
      got = "-"
    end
  elsif op == "ec_cipher"
    got = MessageDigest.aes_256_gcm_encrypt(ActionController::MessageEncryptor.derive_key(secret), a, b)
  elsif op == "nul_key"
    # A secret whose derived key holds a zero byte: the digest must be
    # over all 64 bytes of it (a C-string decode used to cut it short).
    got = MV.digest_for(a, b, "payload", c == "SHA1")
  else
    got = "?"
  end
  want = want_raw == "-" ? "-" : unhex(want_raw)
  totals[op] = totals.fetch(op, 0) + 1
  if got.b == want.b
    passes[op] = passes.fetch(op, 0) + 1
  elsif shown < 40
    shown += 1
    puts "FAIL " + op + " " + a[0, 60].to_s
    puts "     want " + want[0, 120].to_s
    puts "     got  " + got[0, 120].to_s
  end
  i += 1
end
totals.each do |op, n|
  puts op + " " + passes.fetch(op, 0).to_s + "/" + n.to_s
end
puts "done"
