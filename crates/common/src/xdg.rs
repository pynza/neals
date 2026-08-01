use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

fn home_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home))
}

fn config_dir_with(xdg_config_home: Option<OsString>, home: PathBuf) -> PathBuf {
    match xdg_config_home {
        Some(xdg) => PathBuf::from(xdg).join("neals"),
        None => home.join(".config").join("neals"),
    }
}

fn state_dir_with(xdg_state_home: Option<OsString>, home: PathBuf) -> PathBuf {
    match xdg_state_home {
        Some(xdg) => PathBuf::from(xdg).join("neals"),
        None => home.join(".local").join("state").join("neals"),
    }
}

fn runtime_dir_with(xdg_runtime_dir: Option<OsString>) -> PathBuf {
    match xdg_runtime_dir {
        Some(xdg) => PathBuf::from(xdg).join("neals"),
        None => PathBuf::from("/tmp/neals"),
    }
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(config_dir_with(std::env::var_os("XDG_CONFIG_HOME"), home_dir()?))
}

pub fn state_dir() -> Result<PathBuf> {
    Ok(state_dir_with(std::env::var_os("XDG_STATE_HOME"), home_dir()?))
}

pub fn runtime_dir() -> Result<PathBuf> {
    Ok(runtime_dir_with(std::env::var_os("XDG_RUNTIME_DIR")))
}

pub fn projects_file() -> Result<PathBuf> {
    Ok(config_dir()?.join("projects.json"))
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_prefers_xdg() {
        let got = config_dir_with(Some(OsString::from("/tmp/foo")), PathBuf::from("/home/x"));
        assert_eq!(got, PathBuf::from("/tmp/foo/neals"));
    }

    #[test]
    fn config_dir_falls_back_to_home() {
        let got = config_dir_with(None, PathBuf::from("/home/x"));
        assert_eq!(got, PathBuf::from("/home/x/.config/neals"));
    }

    #[test]
    fn state_dir_prefers_xdg() {
        let got = state_dir_with(Some(OsString::from("/tmp/state")), PathBuf::from("/home/x"));
        assert_eq!(got, PathBuf::from("/tmp/state/neals"));
    }

    #[test]
    fn runtime_dir_prefers_xdg() {
        let got = runtime_dir_with(Some(OsString::from("/run/user/1000")));
        assert_eq!(got, PathBuf::from("/run/user/1000/neals"));
    }

    #[test]
    fn runtime_dir_falls_back_to_tmp() {
        assert_eq!(runtime_dir_with(None), PathBuf::from("/tmp/neals"));
    }

    #[test]
    fn ensure_dir_creates_nested_path() {
        let tmp = std::env::temp_dir().join(format!("neals-ensure-{}", std::process::id()));
        let nested = tmp.join("a").join("b");
        ensure_dir(&nested).unwrap();
        assert!(nested.is_dir());
        let _ = fs::remove_dir_all(&tmp);
    }
}
