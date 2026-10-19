//! The complete integration suite for `bin/rh verify --all`.
//! The build script discovers every sibling Rust suite. Normal focused Cargo
//! commands keep their separate targets and do not compile these modules twice.
#![cfg(roundhouse_test_bundle)]

include!(concat!(env!("OUT_DIR"), "/integration_tests.rs"));
