use crate::style;
use anyhow::{bail, Context, Result};
use neals_common::{has_neals_config, inject_neals_stanza, resolve_project_name};
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run() -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    init_in(&cwd)
}

fn init_in(dir: &Path) -> Result<()> {
    let devenv_nix = dir.join("devenv.nix");
    if !devenv_nix.is_file() {
        style::eprint_dim("no devenv.nix — running `devenv init`");
        let status = Command::new("devenv")
            .arg("init")
            .current_dir(dir)
            .stdin(Stdio::null())
            .status()
            .context("failed to run `devenv init` (is devenv on PATH?)")?;
        if !status.success() {
            bail!("`devenv init` failed with {status}");
        }
        if !devenv_nix.is_file() {
            bail!("`devenv init` did not create {}", devenv_nix.display());
        }
        style::print_ok("ran `devenv init`");
    }

    let src = fs::read_to_string(&devenv_nix)
        .with_context(|| format!("failed to read {}", devenv_nix.display()))?;
    if has_neals_config(&src) {
        style::print_ok("devenv.nix already has a neals block");
        return Ok(());
    }

    let name = resolve_project_name(dir)?.as_str().to_string();
    fs::write(&devenv_nix, inject_neals_stanza(&src, &name)?)
        .with_context(|| format!("failed to write {}", devenv_nix.display()))?;
    style::print_ok(&format!(
        "added neals block (`neals.name = \"{name}\"`) — `neals register` when ready"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_block_is_noop() {
        let dir = std::env::temp_dir().join(format!(
            "neals-init-noop-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devenv.nix");
        let src = r#"{ neals = { name = "keep"; services = { }; }; }"#;
        fs::write(&path, src).unwrap();
        init_in(&dir).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), src);
        let _ = fs::remove_dir_all(&dir);
    }
}
