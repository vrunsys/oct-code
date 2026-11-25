//! Shared helpers for tests that drive a real child process.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Path to the `fake-claude` binary, built once per test process.
pub fn fake_claude_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        escargot::CargoBuild::new()
            .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .package("fake-claude")
            .bin("fake-claude")
            .run()
            .expect("build fake-claude")
            .path()
            .to_path_buf()
    })
    .clone()
}

/// Runs `future` to completion, failing the test if it takes over 10 seconds.
pub fn block_on_timeout<T>(future: impl Future<Output = T>) -> T {
    smol::block_on(smol::future::or(future, async {
        smol::Timer::after(Duration::from_secs(10)).await;
        panic!("timed out after 10s");
    }))
}
