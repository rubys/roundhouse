//! ActionDispatch::Session — per-app session store. Empty by default
//! (real-blog uses no session keys); HWIA-shape shim methods route
//! through an internal HashMap so apps that introduce session keys
//! can grow the surface without a runtime rewrite.
//!
//! Hand-written for rust2 Phase 3 (sibling of
//! `runtime/ruby/action_dispatch/session.rb`). The transpile pipeline
//! produces broken Rust for this file's shim methods; hand-writing
//! avoids fighting those emit bugs.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Session {
    data: HashMap<String, String>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_persisted(other: Option<&HashMap<String, String>>) -> Self {
        let mut session = Self::default();
        if let Some(map) = other {
            for (k, v) in map {
                session.data.insert(k.clone(), v.clone());
            }
        }
        session
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }

    pub fn set(&mut self, key: &str, value: String) {
        self.data.insert(key.to_string(), value);
    }

    pub fn fetch(&self, key: &str, default: Option<String>) -> Option<String> {
        self.get(key).or(default)
    }

    pub fn key(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.key(key)
    }

    pub fn include(&self, key: &str) -> bool {
        self.key(key)
    }

    pub fn delete(&mut self, key: &str) -> Option<String> {
        self.data.remove(key)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn to_h(&self) -> HashMap<String, String> {
        self.data.clone()
    }
}
