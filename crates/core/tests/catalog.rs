//! The model catalog Claude Code caches for its model picker.

use std::fs;

use agent_desk_core::claude::catalog::{ModelCatalog, load_catalog};
use agent_desk_test_support::write_model_catalog;

#[test]
fn reads_models_and_their_reasoning_levels() {
    let home = tempfile::tempdir().unwrap();
    write_model_catalog(home.path());
    let catalog = load_catalog(home.path()).expect("a catalog");

    let ids: Vec<&str> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-4-5-20251001"
        ]
    );
    let opus = catalog.model("claude-opus-5-5").unwrap();
    assert_eq!(opus.name, "Opus 5.5");
    let levels: Vec<&str> = opus.efforts.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(levels, ["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(opus.efforts[1].note.as_deref(), Some("Recommended"));
    assert_eq!(opus.efforts[4].note.as_deref(), Some("3.5× or more usage"));
}

#[test]
fn a_model_without_reasoning_has_no_levels() {
    let home = tempfile::tempdir().unwrap();
    write_model_catalog(home.path());
    let catalog = load_catalog(home.path()).unwrap();
    assert!(
        catalog
            .model("claude-haiku-4-5-20251001")
            .unwrap()
            .efforts
            .is_empty()
    );
}

#[test]
fn ids_that_could_become_flags_are_dropped() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("cache/model-catalog");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("x-cc.json"),
        r#"{"catalog":{"config":{"models":[
            {"id":"--dangerously-skip-permissions","name":"Evil"},
            {"id":"claude-opus-5-5","name":"Opus 5.5","thinking":{"type":"effort","effort_options":[{"id":"--x","name":"Bad"},{"id":"low","name":"Low"}]}}
        ]}}}"#,
    )
    .unwrap();
    let catalog = load_catalog(home.path()).unwrap();
    assert_eq!(catalog.models.len(), 1);
    let levels: Vec<&str> = catalog.models[0]
        .efforts
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(levels, ["low"]);
}

#[test]
fn missing_or_unreadable_catalog_is_none() {
    let home = tempfile::tempdir().unwrap();
    assert!(load_catalog(home.path()).is_none());
    let dir = home.path().join("cache/model-catalog");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("x-cc.json"), "{not json").unwrap();
    assert!(load_catalog(home.path()).is_none());
}

#[test]
fn fallback_offers_only_the_clis_aliases() {
    let fallback = ModelCatalog::fallback();
    let ids: Vec<&str> = fallback.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["opus", "sonnet", "haiku", "fable"]);
    assert!(fallback.models.iter().all(|m| !m.efforts.is_empty()));
}

#[test]
fn levels_for_a_model_or_for_the_default_model() {
    let home = tempfile::tempdir().unwrap();
    write_model_catalog(home.path());
    let catalog = load_catalog(home.path()).unwrap();
    let ids = |model: Option<&str>| -> Vec<String> {
        catalog
            .efforts_for(model)
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(ids(Some("claude-sonnet-5-5")), ["low", "high"]);
    assert!(ids(Some("claude-haiku-4-5-20251001")).is_empty());
    assert!(ids(Some("not-in-catalog")).is_empty());
    // With no model chosen, Claude's default decides: every level, once, in catalog order.
    assert_eq!(ids(None), ["low", "medium", "high", "xhigh", "max"]);
}
