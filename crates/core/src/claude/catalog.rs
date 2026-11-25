//! The models and reasoning levels Claude Code offers this account.
//!
//! Claude Code caches the catalog its own `/model` picker uses at
//! `<claude home>/cache/model-catalog/<account>-cc.json`. Reading it keeps
//! the app's list exactly in step with Claude Code, with no model names in
//! this code. The file is Claude Code's internal cache, so it is read
//! tolerantly: anything unexpected is skipped, and without a usable file the
//! app falls back to the CLI's documented aliases.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::args::is_plain_token;

#[derive(Debug, Clone, PartialEq)]
pub struct EffortOption {
    /// The `--effort` value, e.g. `xhigh`.
    pub id: String,
    pub name: String,
    /// Claude Code's badge for the level, e.g. "Recommended".
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    /// The `--model` value, e.g. `claude-opus-5-5`.
    pub id: String,
    pub name: String,
    /// Reasoning levels the model takes; empty when it has none.
    pub efforts: Vec<EffortOption>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelCatalog {
    pub models: Vec<ModelInfo>,
}

impl ModelCatalog {
    pub fn model(&self, id: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|model| model.id == id)
    }

    /// The reasoning levels for `model`. With no model chosen, Claude's
    /// default model decides, so every level in the catalog is offered once,
    /// in catalog order.
    pub fn efforts_for(&self, model: Option<&str>) -> Vec<EffortOption> {
        match model {
            Some(id) => self
                .model(id)
                .map(|model| model.efforts.clone())
                .unwrap_or_default(),
            None => {
                let mut levels: Vec<EffortOption> = Vec::new();
                for level in self.models.iter().flat_map(|model| &model.efforts) {
                    if !levels.iter().any(|seen| seen.id == level.id) {
                        levels.push(level.clone());
                    }
                }
                levels
            }
        }
    }

    /// Used when Claude Code's catalog is unavailable: the CLI's documented
    /// aliases, which always resolve to the latest model of each family, and
    /// its documented `--effort` levels.
    pub fn fallback() -> Self {
        let effort = |id: &str, name: &str| EffortOption {
            id: id.into(),
            name: name.into(),
            note: None,
        };
        let efforts = vec![
            effort("low", "Low"),
            effort("medium", "Medium"),
            effort("high", "High"),
            effort("xhigh", "Extra"),
            effort("max", "Max"),
        ];
        let model = |id: &str, name: &str| ModelInfo {
            id: id.into(),
            name: name.into(),
            efforts: efforts.clone(),
        };
        Self {
            models: vec![
                model("opus", "Opus (latest)"),
                model("sonnet", "Sonnet (latest)"),
                model("haiku", "Haiku (latest)"),
                model("fable", "Fable (latest)"),
            ],
        }
    }
}

/// The newest cached catalog under `claude_home`, if one is readable.
pub fn load_catalog(claude_home: &Path) -> Option<ModelCatalog> {
    let path = newest_catalog_file(&claude_home.join("cache").join("model-catalog"))?;
    let value: Value = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    let models: Vec<ModelInfo> = value
        .pointer("/catalog/config/models")?
        .as_array()?
        .iter()
        .filter_map(parse_model)
        .collect();
    (!models.is_empty()).then_some(ModelCatalog { models })
}

fn newest_catalog_file(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.ends_with("-cc.json") && !name.ends_with(".badges.json")
        })
        .max_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())
        .map(|entry| entry.path())
}

fn parse_model(value: &Value) -> Option<ModelInfo> {
    let id = value.get("id")?.as_str().filter(|id| is_plain_token(id))?;
    let name = value.get("name").and_then(Value::as_str).unwrap_or(id);
    let efforts = value
        .pointer("/thinking/effort_options")
        .and_then(Value::as_array)
        .map(|options| options.iter().filter_map(parse_effort).collect())
        .unwrap_or_default();
    Some(ModelInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        efforts,
    })
}

fn parse_effort(value: &Value) -> Option<EffortOption> {
    let id = value.get("id")?.as_str().filter(|id| is_plain_token(id))?;
    Some(EffortOption {
        id: id.to_owned(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned(),
        note: value
            .pointer("/badge/message")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
