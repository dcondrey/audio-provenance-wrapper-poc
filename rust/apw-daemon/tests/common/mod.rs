#![allow(dead_code)] // each test binary uses a subset

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

pub fn parity_json<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_slice(&std::fs::read(fixtures().join("parity").join(name)).unwrap()).unwrap()
}
