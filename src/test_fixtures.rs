//! Optional private replay data for ignored integration tests.
//! The repository contains no captured game geometry or runtime dumps.
use std::path::PathBuf;

pub fn bytes(name: &str) -> Vec<u8> {
    let directory = std::env::var_os("ER_CHARACTER_SCALE_FIXTURES")
        .expect("set ER_CHARACTER_SCALE_FIXTURES to your local replay fixture directory");
    std::fs::read(PathBuf::from(directory).join(name)).expect("private fixture unavailable")
}

pub fn text(name: &str) -> String {
    String::from_utf8(bytes(name)).expect("fixture must be UTF-8")
}
