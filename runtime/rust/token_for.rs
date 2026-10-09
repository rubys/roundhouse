//! `ActiveRecord::TokenFor` for the emitted Rust project — hand-written
//! counterpart of `runtime/ruby/active_record/token_for.rb` (the wire
//! format is documented there; it is ActiveSupport's signed-GlobalID
//! envelope: url-safe padded base64, HMAC-SHA1, salt
//! "active_record/token_for", key = PBKDF2-HMAC-SHA256(secret, salt,
//! 1000, 64)).
//!
//! DEPENDENCY-FREE CRYPTO: the emitted Cargo template carries no
//! `sha2`/`hmac`/`sha1`, so SHA-1, SHA-256, HMAC and PBKDF2 are
//! implemented below and pinned by tests against RFC 3174 / FIPS 180 /
//! RFC 2202 / RFC 4231 / RFC 7914 vectors plus a Rails-measured token
//! (`token_matches_rails_measured_vector` reproduces a token Rails
//! 8.1.4 minted, bit for bit). Uses only
//! `base64` and `chrono`, both in the template.
//!
//! The secret is `SECRET_KEY_BASE` from the environment (empty when
//! unset, like `Rails.application.secret_key_base`), unless
//! `TokenFor::set_secret_key_base` overrides it.
//!
//! Every function mirrors the Ruby one: rejections answer `""` (for
//! `verified_data`) / `0` (for `data_id`); never an error.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use std::collections::HashMap;
use std::sync::Mutex;

const SALT: &str = "active_record/token_for";

// ---------------------------------------------------------------- SHA-1

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    for block in padded(data, true).chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

// -------------------------------------------------------------- SHA-256

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    for block in padded(data, true).chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip(v) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// Merkle–Damgård padding shared by SHA-1/SHA-256 (64-byte blocks,
/// big-endian bit length).
fn padded(data: &[u8], _be: bool) -> Vec<u8> {
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    m
}

// ----------------------------------------------------------------- HMAC

fn hmac(hash: fn(&[u8]) -> Vec<u8>, key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = if key.len() > 64 { hash(key) } else { key.to_vec() };
    k.resize(64, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&hash(&inner));
    hash(&outer)
}

fn h1(d: &[u8]) -> Vec<u8> {
    sha1(d).to_vec()
}
fn h256(d: &[u8]) -> Vec<u8> {
    sha256(d).to_vec()
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> Vec<u8> {
    hmac(h1, key, msg)
}
fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    hmac(h256, key, msg)
}

/// PBKDF2-HMAC-SHA256 (RFC 8018), `len` output bytes.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut block = 1u32;
    while out.len() < len {
        let mut s = salt.to_vec();
        s.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha256(password, &s);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for (a, b) in t.iter_mut().zip(&u) {
                *a ^= b;
            }
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(len);
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `ActiveSupport::SecurityUtils.secure_compare`: no early exit.
fn secure_compare(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ------------------------------------------------------ secret and keys

static SECRET_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);
static KEYS: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

/// `None` when no (or an empty) secret is configured: signing then fails
/// closed instead of deriving a key anyone can compute.
fn secret_key_base() -> Option<String> {
    if let Some(s) = SECRET_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return (!s.is_empty()).then_some(s);
    }
    std::env::var("SECRET_KEY_BASE").ok().filter(|s| !s.is_empty())
}

/// PBKDF2 is 2000 HMACs; cache per (salt, secret) like `derive_key`.
fn derive_key(secret: &str, salt: &str) -> Vec<u8> {
    let cache_key = format!("{salt}|{secret}");
    let mut g = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.entry(cache_key)
        .or_insert_with(|| pbkdf2_sha256(secret.as_bytes(), salt.as_bytes(), 1_000, 64))
        .clone()
}

// --------------------------------------------------------------- base64

/// `Base64.urlsafe_encode64` (padded).
fn b64_encode(s: &str) -> String {
    base64::engine::general_purpose::URL_SAFE.encode(s.as_bytes())
}

/// `Base64.urlsafe_decode64`: padding optional.
fn b64_decode(s: &str) -> Option<String> {
    let engine = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    String::from_utf8(engine.decode(s).ok()?).ok()
}

// ------------------------------------------------- envelope text scans

/// `iso8601_ms`: UTC, exactly three fractional digits.
fn iso8601_ms(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `MessageVerifier.extract`: the plain string field after `prefix`.
fn extract(env: &str, prefix: &str) -> String {
    let Some(at) = env.find(prefix) else { return String::new() };
    let rest = at + prefix.len();
    match env[rest..].find('"') {
        Some(close) => env[rest..rest + close].to_string(),
        None => String::new(),
    }
}

/// Where the JSON value starting at `start` ends (exclusive).
fn value_end(s: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut i = start;
    while i < s.len() {
        let c = s[i];
        if in_string {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_string = false;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
        } else if c == b'"' {
            in_string = true;
        } else if c == b'{' || c == b'[' {
            depth += 1;
        } else if c == b'}' || c == b']' {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        } else if c == b',' && depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `MessageVerifier.extract_raw`: a bare JSON value, "" when absent.
fn extract_raw(env: &str, prefix: &str) -> String {
    let Some(at) = env.find(prefix) else { return String::new() };
    let start = at + prefix.len();
    match value_end(env.as_bytes(), start) {
        Some(end) if end >= start => env[start..end].to_string(),
        _ => String::new(),
    }
}

/// `MessageVerifier.json_string`: a String as ActiveSupport's JSON
/// writes it (`<`, `>`, `&` escaped as `<` ...).
fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 32 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What `secure_password_data` accepts as the bcrypt digest: the model
/// field is a `String`, its reader an `Option<String>`.
pub trait IntoDigest {
    fn into_digest(self) -> Option<String>;
}
impl IntoDigest for String {
    fn into_digest(self) -> Option<String> { Some(self) }
}
impl IntoDigest for &str {
    fn into_digest(self) -> Option<String> { Some(self.to_string()) }
}
impl IntoDigest for Option<String> {
    fn into_digest(self) -> Option<String> { self }
}
impl IntoDigest for Option<&str> {
    fn into_digest(self) -> Option<String> { self.map(|s| s.to_string()) }
}

pub struct TokenFor;

impl TokenFor {
    /// Override the env-derived secret (tests / embedders).
    pub fn set_secret_key_base(secret: &str) {
        *SECRET_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(secret.to_string());
    }

    /// The signed token for `data_json` under `purpose` (already
    /// JSON-escaped), expiring `expires_in` seconds from now, or never
    /// for 0 (no `exp` key at all).
    pub fn generate(data_json: String, purpose: &str, expires_in: i64) -> String {
        let exp = if expires_in != 0 {
            let t = chrono::Utc::now() + chrono::Duration::seconds(expires_in);
            format!("\"{}\"", iso8601_ms(t))
        } else {
            String::new()
        };
        Self::sign(
            &secret_key_base().expect("SECRET_KEY_BASE must be set to sign ActiveRecord::TokenFor tokens"),
            data_json.as_str(),
            purpose,
            &exp,
        )
    }

    fn sign(secret: &str, data_json: &str, purpose: &str, exp: &str) -> String {
        let mut env = format!("{{\"_rails\":{{\"data\":{data_json}");
        if !exp.is_empty() {
            env.push_str(&format!(",\"exp\":{exp}"));
        }
        env.push_str(&format!(",\"pur\":\"{purpose}\"}}}}"));
        let payload = b64_encode(&env);
        let key = derive_key(secret, SALT);
        let digest = hex(&hmac_sha1(&key, payload.as_bytes()));
        format!("{payload}--{digest}")
    }

    /// The `data` JSON `token` carries, or "" for every rejection:
    /// tampered, other purpose, or expired.
    pub fn verified_data(token: &str, purpose: &str) -> String {
        match secret_key_base() {
            Some(secret) => Self::verify(&secret, token, purpose, chrono::Utc::now()),
            None => String::new(),
        }
    }

    fn verify(secret: &str, token: &str, purpose: &str, now: chrono::DateTime<chrono::Utc>) -> String {
        // The digest is hex (no '-'), the payload may hold '-': split
        // at the LAST separator. (Ruby splits at the first; they agree
        // for every token this module writes unless the payload itself
        // contains "--", which Ruby then rejects.)
        let Some(sep) = token.rfind("--") else { return String::new() };
        let (payload, supplied) = (&token[..sep], &token[sep + 2..]);
        let key = derive_key(secret, SALT);
        if !secure_compare(supplied, &hex(&hmac_sha1(&key, payload.as_bytes()))) {
            return String::new();
        }
        let Some(env) = b64_decode(payload) else { return String::new() };
        if extract(&env, "\"pur\":\"") != purpose {
            return String::new();
        }
        let exp = extract(&env, "\"exp\":\"");
        if !exp.is_empty() && exp <= iso8601_ms(now) {
            return String::new();
        }
        extract_raw(&env, "\"data\":")
    }

    /// has_secure_password's payload: `[id, password_salt&.last(10)]`.
    /// A bcrypt digest is `$2a$12$` + 22 salt chars + hash, so the last
    /// ten salt characters are bytes 19..29.
    pub fn secure_password_data(id: i64, digest: impl IntoDigest) -> String {
        let tail = digest
            .into_digest()
            .filter(|d| d.len() >= 29)
            .and_then(|d| d.get(19..29).map(|s| s.to_string()));
        Self::value_data(id, tail.as_deref())
    }

    /// `[id, "value"]` (nil being `null`) for a String value.
    pub fn value_data(id: i64, value: Option<&str>) -> String {
        let json = value.map(json_string).unwrap_or_else(|| "null".to_string());
        format!("[{id},{json}]")
    }

    /// `[id, 5]` for an Integer value.
    pub fn int_value_data(id: i64, value: Option<i64>) -> String {
        match value {
            Some(v) => format!("[{id},{v}]"),
            None => format!("[{id},null]"),
        }
    }

    /// `[id, true]` for a boolean value.
    pub fn bool_value_data(id: i64, value: Option<bool>) -> String {
        match value {
            Some(v) => format!("[{id},{v}]"),
            None => format!("[{id},null]"),
        }
    }

    /// A declaration without a block signs the id alone: `[id]`.
    pub fn id_data(id: i64) -> String {
        format!("[{id}]")
    }

    /// The record id at the head of a `[id, ...]` payload, or 0 (no
    /// row) for "" (Ruby: `data[1..].to_i`, i.e. leading digits).
    pub fn data_id(data: String) -> i64 {
        if data.len() < 2 {
            return 0;
        }
        let rest = data[1..].trim_start();
        let (sign, digits) = match rest.strip_prefix('-') {
            Some(d) => (-1, d),
            None => (1, rest),
        };
        let n: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
        sign * n.parse::<i64>().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha256_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // multi-block, million 'a'
        let m = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&m)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn hmac_vectors() {
        // RFC 2202 case 2
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        // RFC 2202 case 6: key longer than the block
        assert_eq!(
            hex(&hmac_sha1(&[0xaa; 80], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
        // RFC 4231 case 2
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn pbkdf2_vectors() {
        // RFC 7914 section 11: P="passwd", S="salt", c=1, dkLen=64
        assert_eq!(
            hex(&pbkdf2_sha256(b"passwd", b"salt", 1, 64)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        );
        // Well-known: password/salt, c=2, 32 bytes
        assert_eq!(
            hex(&pbkdf2_sha256(b"password", b"salt", 2, 32)),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }

    /// The token Rails 8.1.4 minted (see the header of
    /// runtime/ruby/active_record/token_for.rb: signature
    /// 5f96b355...51e4 for SECRET_KEY_BASE=test-secret), reproduced
    /// bit for bit; also re-derived with CRuby OpenSSL.
    #[test]
    fn token_matches_rails_measured_vector() {
        let p = "User\\npassword_reset\\n900";
        let exp = "\"2026-10-03T20:10:58.951Z\"";
        let tok = TokenFor::sign("test-secret", "[1,\"Iyo3zOdjGO\"]", p, exp);
        assert_eq!(
            tok,
            "eyJfcmFpbHMiOnsiZGF0YSI6WzEsIkl5bzN6T2RqR08iXSwiZXhwIjoiMjAyNi0xMC0wM1QyMDoxMDo1OC45NTFaIiwicHVyIjoiVXNlclxucGFzc3dvcmRfcmVzZXRcbjkwMCJ9fQ==--5f96b3553eb0ad2afc31948cd14f57df8af251e4"
        );
        let before = Utc.with_ymd_and_hms(2026, 10, 3, 20, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 10, 3, 20, 11, 0).unwrap();
        assert_eq!(TokenFor::verify("test-secret", &tok, p, before), "[1,\"Iyo3zOdjGO\"]");
        assert_eq!(TokenFor::verify("test-secret", &tok, p, after), "");
        // no-exp form, digest produced by CRuby OpenSSL
        let tok2 = TokenFor::sign("test-secret", "[1,\"Iyo3zOdjGO\"]", p, "");
        assert!(tok2.ends_with("--ed5c200a96200dda27da15398958a52223e50dfb"));
    }

    #[test]
    fn roundtrip_purpose_tamper_expiry() {
        TokenFor::set_secret_key_base("s3cret");
        let digest = "$2a$12$abcdefghijklmnopqrstuvXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0";
        let data = TokenFor::secure_password_data(7, digest.to_string());
        assert_eq!(data, "[7,\"uvXYZ01234\"]".replace("uvXYZ01234", &digest[19..29]));
        let p = "User\\npassword_reset\\n900";
        let tok = TokenFor::generate(data.clone(), p, 900);
        assert_eq!(TokenFor::verified_data(&tok, p), data);
        assert_eq!(TokenFor::data_id(data.clone()), 7);
        // wrong purpose, tampered payload/signature, garbage
        assert_eq!(TokenFor::verified_data(&tok, "User\\nother\\n900"), "");
        let mut bad = tok.clone();
        bad.push('0');
        assert_eq!(TokenFor::verified_data(&bad, p), "");
        let flipped = format!("A{}", &tok[1..]);
        assert_eq!(TokenFor::verified_data(&flipped, p), "");
        assert_eq!(TokenFor::verified_data("nonsense", p), "");
        assert_eq!(TokenFor::data_id(String::new()), 0);
        // other secret
        assert_eq!(TokenFor::verify("other", &tok, p, Utc::now()), "");
        // expiry: a negative lifetime is already past
        let past = TokenFor::generate(data.clone(), p, -5);
        assert_eq!(TokenFor::verified_data(&past, p), "");
        // and a 900s token is dead 901s later
        let later = Utc::now() + chrono::Duration::seconds(901);
        assert_eq!(TokenFor::verify("s3cret", &tok, p, later), "");
        // never-expiring: no exp key
        let forever = TokenFor::generate(TokenFor::id_data(3), p, 0);
        let env = b64_decode(forever.split("--").next().unwrap()).unwrap();
        assert!(!env.contains("\"exp\""));
        assert_eq!(TokenFor::verified_data(&forever, p), "[3]");
    }

    #[test]
    fn payload_shapes() {
        assert_eq!(TokenFor::value_data(1, None), "[1,null]");
        assert_eq!(TokenFor::value_data(1, Some("a<\"b")), "[1,\"a\\u003c\\\"b\"]");
        assert_eq!(TokenFor::int_value_data(2, Some(5)), "[2,5]");
        assert_eq!(TokenFor::bool_value_data(2, Some(true)), "[2,true]");
        assert_eq!(TokenFor::secure_password_data(4, None::<String>), "[4,null]");
        assert_eq!(TokenFor::secure_password_data(4, "short"), "[4,null]");
        assert_eq!(TokenFor::data_id("[42,\"x\"]".to_string()), 42);
        let t = Utc.with_ymd_and_hms(2026, 10, 3, 20, 10, 58).unwrap();
        assert_eq!(iso8601_ms(t), "2026-10-03T20:10:58.000Z");
    }

    // Call sites copied from models/user.rs (emitted Campfire).
    struct U { password_digest: String }
    impl U {
        fn id(&self) -> i64 { 1 }
        fn password_digest(&self) -> Option<String> { Some(self.password_digest.clone()) }
        fn password_reset_token(&self) -> String {
            TokenFor::generate(TokenFor::secure_password_data(self.id(), self.password_digest.clone()), "User\\npassword_reset\\n900", 900_i64)
        }
        fn check(&self, token: &str) -> bool {
            let data = TokenFor::verified_data(token, "User\\npassword_reset\\n900");
            let id: i64 = TokenFor::data_id(data.clone());
            let m = std::collections::HashMap::from([("id", TokenFor::data_id(data.clone()))]);
            let _ = m;
            id == self.id() && TokenFor::secure_password_data(self.id(), self.password_digest()) == data.clone()
        }
    }

    #[test]
    fn emitted_call_sites_typecheck_and_work() {
        TokenFor::set_secret_key_base("s3cret");
        let u = U { password_digest: "$2a$12$abcdefghijklmnopqrstuvXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0".into() };
        assert!(u.check(&u.password_reset_token()));
        let other = U { password_digest: "$2a$12$ZZZZZZZZZZZZZZZZZZZZZZXYZ0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ0".into() };
        assert!(!other.check(&u.password_reset_token()));
    }
}
