//! JSON-file persistence for threads and the project list.
//!
//! Every write is atomic (temp file, fsync, rename), so a crash leaves the old
//! file or the new one, never half of one. Reads never fail the app: a file
//! that does not parse is skipped with a warning. On Unix the data directory
//! and files are private to the user, because threads hold conversation
//! history and tool output.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::thread::{Thread, ThreadId};

/// `<data dir>/agent-desk`, e.g. `~/.local/share/agent-desk` on Linux.
pub fn default_root() -> PathBuf {
    dirs::data_dir()
        .expect("the OS reports no user data directory")
        .join("agent-desk")
}

pub struct ThreadStore {
    root: PathBuf,
}

impl ThreadStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Writes `<root>/threads/<id>.json`, replacing any earlier save.
    pub fn save(&self, thread: &Thread) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(thread).map_err(io::Error::other)?;
        write_atomic(&self.thread_path(&thread.id)?, &json)
    }

    /// Every readable thread. Unreadable, corrupt or misnamed files are skipped.
    pub fn load_all(&self) -> Vec<Thread> {
        let entries = match fs::read_dir(self.threads_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                log::warn!("reading {}: {error}", self.threads_dir().display());
                return Vec::new();
            }
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| match load_thread(&path) {
                Ok(thread) => Some(thread),
                Err(error) => {
                    log::warn!("skipping thread file {}: {error}", path.display());
                    None
                }
            })
            .collect()
    }

    pub fn delete(&self, id: &ThreadId) -> io::Result<()> {
        fs::remove_file(self.thread_path(id)?)
    }

    fn threads_dir(&self) -> PathBuf {
        self.root.join("threads")
    }

    fn thread_path(&self, id: &ThreadId) -> io::Result<PathBuf> {
        if !is_safe_file_stem(&id.0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid thread id {:?}", id.0),
            ));
        }
        Ok(self.threads_dir().join(format!("{}.json", id.0)))
    }
}

fn load_thread(path: &Path) -> io::Result<Thread> {
    let thread: Thread = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
    // The id names the file it is saved to, so a mismatch would make the next
    // save write somewhere else and leave a duplicate behind.
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(thread.id.0.as_str()) {
        return Err(io::Error::other(format!(
            "file name does not match thread id {:?}",
            thread.id.0
        )));
    }
    Ok(thread)
}

/// Thread ids are UUIDs; anything else could name a path outside the store.
fn is_safe_file_stem(stem: &str) -> bool {
    !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub path: PathBuf,
    pub name: String,
    pub is_git: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    NotADirectory(PathBuf),
    AlreadyAdded(PathBuf),
    Io(String),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotADirectory(path) => write!(f, "{} is not a directory", path.display()),
            ProjectError::AlreadyAdded(path) => write!(f, "{} is already added", path.display()),
            ProjectError::Io(error) => write!(f, "could not save projects: {error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

pub struct ProjectList {
    root: PathBuf,
    projects: Vec<Project>,
}

impl ProjectList {
    /// Loads `<root>/projects.json`. A missing or corrupt file gives an empty list.
    pub fn load(root: PathBuf) -> Self {
        let path = root.join("projects.json");
        let projects = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::warn!("ignoring corrupt {}: {error}", path.display());
                Vec::new()
            }),
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    log::warn!("reading {}: {error}", path.display());
                }
                Vec::new()
            }
        };
        Self { root, projects }
    }

    pub fn add(&mut self, path: &Path) -> Result<Project, ProjectError> {
        if !path.is_dir() {
            return Err(ProjectError::NotADirectory(path.to_path_buf()));
        }
        let path = dunce::canonicalize(path).map_err(|e| ProjectError::Io(e.to_string()))?;
        if self.projects.iter().any(|project| project.path == path) {
            return Err(ProjectError::AlreadyAdded(path));
        }
        let project = Project {
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            // `.git` is a file in a worktree, so existence is the test, not is_dir.
            is_git: path.join(".git").exists(),
            path,
        };

        let mut projects = self.projects.clone();
        projects.push(project.clone());
        self.save(&projects)
            .map_err(|e| ProjectError::Io(e.to_string()))?;
        self.projects = projects;
        Ok(project)
    }

    /// Removes the project at `path`. The folder itself may already be gone.
    pub fn remove(&mut self, path: &Path) -> io::Result<()> {
        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let projects: Vec<Project> = self
            .projects
            .iter()
            .filter(|project| project.path != canonical && project.path != path)
            .cloned()
            .collect();
        self.save(&projects)?;
        self.projects = projects;
        Ok(())
    }

    pub fn list(&self) -> &[Project] {
        &self.projects
    }

    fn save(&self, projects: &[Project]) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(projects).map_err(io::Error::other)?;
        write_atomic(&self.root.join("projects.json"), &json)
    }
}

/// Writes `bytes` to `<path>.tmp`, flushes it to disk, then renames it over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);

    let mut file = private_file_options().open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

fn create_private_dir_all(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

fn private_file_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}
