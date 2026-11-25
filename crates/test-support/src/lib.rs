//! Helpers for tests that drive real child processes. Test-only: no crate
//! depends on this outside `[dev-dependencies]`.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
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

/// Serializes tests that write an executable and then run one. Without it, a
/// fork on another thread can inherit the open write handle and make exec
/// fail with ETXTBSY ("Text file busy") on Linux.
pub fn process_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Writes a small Claude Code model catalog under `claude_home`, shaped like
/// `cache/model-catalog/*-cc.json`: two models with reasoning levels (the
/// first with badges) and one without reasoning.
pub fn write_model_catalog(claude_home: &std::path::Path) {
    use serde_json::json;
    let effort = |id: &str, name: &str| json!({"id": id, "name": name});
    let catalog = json!({
        "version": 2,
        "fetchedAt": 1,
        "staleAt": 2,
        "catalog": {
            "surface": "cc",
            "config": {
                "id": "cc",
                "models": [
                    {"id": "claude-opus-5-5", "name": "Opus 5.5", "thinking": {"type": "effort", "effort_options": [
                        effort("low", "Low"),
                        {"id": "medium", "name": "Medium", "badge": {"message": "Recommended", "variant": "neutral"}},
                        effort("high", "High"),
                        effort("xhigh", "Extra"),
                        {"id": "max", "name": "Max", "badge": {"message": "3.5× or more usage", "variant": "warning"}},
                    ]}},
                    {"id": "claude-sonnet-5-5", "name": "Sonnet 5.5", "thinking": {"type": "effort", "effort_options": [
                        effort("low", "Low"), effort("high", "High"),
                    ]}},
                    {"id": "claude-haiku-4-5-20251001", "name": "Haiku 4.5", "thinking": {"type": "none"}},
                ],
            },
        },
    });
    let dir = claude_home.join("cache").join("model-catalog");
    std::fs::create_dir_all(&dir).expect("create catalog dir");
    std::fs::write(dir.join("account-cc.json"), catalog.to_string()).expect("write catalog");
    std::fs::write(dir.join("account-cc.badges.json"), "{}").expect("write badges");
}
