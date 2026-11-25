use std::fs;

use agent_desk_core::settings::{Appearance, Settings};

#[test]
fn saved_appearance_survives_reload() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.appearance = Appearance::Dark;
    settings.save(root.path()).unwrap();
    assert_eq!(Settings::load(root.path()).appearance, Appearance::Dark);
}

#[test]
fn missing_file_gives_defaults() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(Settings::load(root.path()).appearance, Appearance::System);
}

#[test]
fn corrupt_file_gives_defaults() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("settings.json"),
        r#"{"appearance": "sepia"}"#,
    )
    .unwrap();
    assert_eq!(Settings::load(root.path()).appearance, Appearance::System);
}

#[cfg(unix)]
#[test]
fn settings_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    Settings::default().save(root.path()).unwrap();
    let mode = fs::metadata(root.path().join("settings.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
