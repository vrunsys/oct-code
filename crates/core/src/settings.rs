//! User preferences, saved as `<root>/settings.json`.
//!
//! Loading never fails: a missing or unreadable file gives the defaults.
//! Saving uses the store's atomic, private write.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store::write_atomic;

const FILE_NAME: &str = "settings.json";

/// Light or dark, or follow the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Settings {
    pub appearance: Appearance,
}

impl Settings {
    pub fn load(root: &Path) -> Self {
        let path = root.join(FILE_NAME);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::warn!("ignoring unreadable {}: {error}", path.display());
                Self::default()
            }),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::warn!("reading {}: {error}", path.display());
                }
                Self::default()
            }
        }
    }

    pub fn save(&self, root: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(&root.join(FILE_NAME), &json)
    }
}
