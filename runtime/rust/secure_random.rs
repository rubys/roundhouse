//! `SecureRandom` for the Rust target — hand-written counterpart of
//! Ruby's `SecureRandom` plus the `lower::mocha` stub slot
//! (`runtime/spinel/secure_random_stub.rb`).
//!
//! Only the surface the emitted code calls: `alphanumeric`, `uuid`,
//! `stub_alphanumeric`, `stub_uuid` (and `clear_stubs`).
//!
//! Randomness: `/dev/urandom`, falling back (non-unix, read failure) to
//! std's `RandomState` keyed hashing mixed with the clock. No new crates.
//!
//! Stubs are THREAD-LOCAL: `cargo test` runs each test on its own thread,
//! so one test's `stub_uuid` can never leak into a sibling, and a fresh
//! test thread starts unstubbed (the Ruby side clears in the helper's
//! setup; here that is automatic). Consequence: a stub is only seen by
//! code running on the thread that installed it. Like the Ruby stub, a
//! stub answers EVERY call until cleared (not one-shot).

use std::cell::RefCell;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Read;

const ALPHANUMERIC: &[u8; 62] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

thread_local! {
    static STUB_ALPHANUMERIC: RefCell<Option<String>> = const { RefCell::new(None) };
    static STUB_UUID: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub struct SecureRandom;

fn fallback_bytes(buf: &mut [u8]) {
    let mut counter: u64 = 0;
    for chunk in buf.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(counter);
        counter += 1;
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.write_u128(d.as_nanos());
        }
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !ok {
        fallback_bytes(&mut buf);
    }
    buf
}

impl SecureRandom {
    /// `SecureRandom.alphanumeric(n)`: `n` chars of `[A-Za-z0-9]`,
    /// rejection-sampled (bytes >= 248 are dropped) so the distribution
    /// is uniform. `n <= 0` yields `""`. A stub, when set, wins.
    pub fn alphanumeric(n: i64) -> String {
        if let Some(v) = STUB_ALPHANUMERIC.with(|s| s.borrow().clone()) {
            return v;
        }
        let n = n.max(0) as usize;
        let mut out = String::with_capacity(n);
        while out.len() < n {
            let want = n - out.len();
            for b in random_bytes(want * 2 + 8) {
                if out.len() >= n {
                    break;
                }
                if b < 248 {
                    out.push(ALPHANUMERIC[(b % 62) as usize] as char);
                }
            }
        }
        out
    }

    /// `SecureRandom.uuid`: RFC 9562 v4, lowercase, hyphenated. A stub,
    /// when set, wins.
    pub fn uuid() -> String {
        if let Some(v) = STUB_UUID.with(|s| s.borrow().clone()) {
            return v;
        }
        let mut b = random_bytes(16);
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h: String = b.iter().map(|x| format!("{:02x}", x)).collect();
        format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
    }

    /// `SecureRandom.stub_alphanumeric(v)`: every later `alphanumeric(_)`
    /// on this thread answers `v` (whatever `n` is) until cleared.
    pub fn stub_alphanumeric(value: impl Into<String>) {
        let v = value.into();
        STUB_ALPHANUMERIC.with(|s| *s.borrow_mut() = Some(v));
    }

    /// `SecureRandom.stub_uuid(v)`: likewise for `uuid()`.
    pub fn stub_uuid(value: impl Into<String>) {
        let v = value.into();
        STUB_UUID.with(|s| *s.borrow_mut() = Some(v));
    }

    /// Reset both stubs (this thread). Ruby: `clear_secure_random_stubs`.
    pub fn clear_stubs() {
        STUB_ALPHANUMERIC.with(|s| *s.borrow_mut() = None);
        STUB_UUID.with(|s| *s.borrow_mut() = None);
    }

    /// Ruby-name alias of [`SecureRandom::clear_stubs`].
    pub fn clear_secure_random_stubs() {
        Self::clear_stubs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphanumeric_length_and_charset() {
        for n in [0_i64, 1, 12, 24, 100] {
            let s = SecureRandom::alphanumeric(n);
            assert_eq!(s.len(), n as usize);
            assert!(s.bytes().all(|c| c.is_ascii_alphanumeric()));
        }
        assert_eq!(SecureRandom::alphanumeric(-3), "");
        assert_ne!(SecureRandom::alphanumeric(24), SecureRandom::alphanumeric(24));
    }

    #[test]
    fn uuid_is_v4_shaped() {
        let u = SecureRandom::uuid();
        assert_eq!(u.len(), 36);
        let p: Vec<&str> = u.split('-').collect();
        assert_eq!(p.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert!(p[2].starts_with('4'));
        assert!("89ab".contains(p[3].chars().next().unwrap()));
        assert!(u.chars().all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_uppercase()));
        assert_ne!(u, SecureRandom::uuid());
    }

    #[test]
    fn stubs_answer_until_cleared() {
        SecureRandom::stub_alphanumeric("tok");
        SecureRandom::stub_uuid(String::from("u-1"));
        assert_eq!(SecureRandom::alphanumeric(12), "tok");
        assert_eq!(SecureRandom::alphanumeric(24), "tok");
        assert_eq!(SecureRandom::uuid(), "u-1");
        SecureRandom::stub_alphanumeric("tok2");
        assert_eq!(SecureRandom::alphanumeric(1), "tok2");
        SecureRandom::clear_stubs();
        assert_eq!(SecureRandom::alphanumeric(12).len(), 12);
        assert_eq!(SecureRandom::uuid().len(), 36);
    }

    #[test]
    fn stubs_are_thread_local() {
        SecureRandom::stub_uuid("mine");
        let other = std::thread::spawn(SecureRandom::uuid).join().unwrap();
        assert_ne!(other, "mine");
        assert_eq!(SecureRandom::uuid(), "mine");
        SecureRandom::clear_secure_random_stubs();
    }

    #[test]
    fn fallback_fills() {
        let mut b = [0u8; 21];
        fallback_bytes(&mut b);
        assert!(b.iter().any(|x| *x != 0));
    }
}
