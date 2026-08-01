use crate::xdg::ensure_dir;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    pub projects: Vec<Project>,
}

impl Registry {
    pub fn load() -> Result<Self> {
        Self::load_from(&crate::xdg::projects_file()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let registry: Self = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(registry)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::xdg::projects_file()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let data = serde_json::to_string_pretty(self).context("failed to serialize registry")?;
        let file_name = path
            .file_name()
            .context("registry path has no file name")?
            .to_string_lossy();
        let tmp_path = path.with_file_name(format!("{file_name}.tmp"));
        fs::write(&tmp_path, &data)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        fs::rename(&tmp_path, path).with_context(|| {
            format!(
                "failed to replace {} with {}",
                path.display(),
                tmp_path.display()
            )
        })?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn add(&mut self, project: Project) -> Result<()> {
        if self.get(&project.name).is_some() {
            bail!("project `{}` is already registered", project.name);
        }
        self.projects.push(project);
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<Project> {
        let idx = self
            .projects
            .iter()
            .position(|p| p.name == name)
            .with_context(|| format!("project `{name}` is not registered"))?;
        Ok(self.projects.remove(idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file_returns_empty() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-registry-missing-{}",
            std::process::id()
        ));
        let path = tmp.join("projects.json");
        let registry = Registry::load_from(&path).unwrap();
        assert!(registry.projects.is_empty());
    }

    #[test]
    fn round_trip_save_and_load() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-registry-roundtrip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = tmp.join("neals").join("projects.json");
        let mut registry = Registry::default();
        registry
            .add(Project {
                name: "ferrari".into(),
                path: PathBuf::from("/home/dev/ferrari"),
            })
            .unwrap();
        registry.save_to(&path).unwrap();

        let loaded = Registry::load_from(&path).unwrap();
        assert_eq!(loaded, registry);
        assert_eq!(
            loaded.get("ferrari").unwrap().path,
            PathBuf::from("/home/dev/ferrari")
        );
        assert!(!path.with_file_name("projects.json.tmp").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_rejects_duplicate_name() {
        let mut registry = Registry::default();
        registry
            .add(Project {
                name: "a".into(),
                path: PathBuf::from("/a"),
            })
            .unwrap();
        let err = registry
            .add(Project {
                name: "a".into(),
                path: PathBuf::from("/b"),
            })
            .unwrap_err();
        assert!(err.to_string().contains("already registered"));
    }

    #[test]
    fn remove_existing_project() {
        let mut registry = Registry::default();
        registry
            .add(Project {
                name: "a".into(),
                path: PathBuf::from("/a"),
            })
            .unwrap();
        let removed = registry.remove("a").unwrap();
        assert_eq!(removed.name, "a");
        assert!(registry.projects.is_empty());
    }

    #[test]
    fn remove_missing_project_errors() {
        let mut registry = Registry::default();
        let err = registry.remove("missing").unwrap_err();
        assert!(err.to_string().contains("not registered"));
    }
}
