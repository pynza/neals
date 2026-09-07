use crate::daemon_client::with_daemon;
use crate::style;
use anyhow::{bail, Context, Result};
use neals_common::{runtime_dir, Registry, Request, Response, SYSTEM_DAEMON_SOCKET};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

pub fn run(project: &str, update: bool, hard: bool, yes: bool) -> Result<ExitCode> {
    let project = resolve_registered(project)?;

    if hard {
        print_hard_warning(&project.name, &project.path);
        if !crate::confirm("Continue?", yes)? {
            style::print_dim("aborted");
            return Ok(ExitCode::SUCCESS);
        }
    }

    stop_if_running(&project.name)?;

    if hard {
        wipe_managed_state(&project.name, &project.path)?;
    }

    if update {
        update_inputs(&project.path)?;
    }

    invalidate_and_evaluate(&project.path)?;

    style::print_ok(&format!(
        "refreshed `{name}` — run `neals up {name}` when ready",
        name = project.name
    ));
    Ok(ExitCode::SUCCESS)
}

fn resolve_registered(name: &str) -> Result<neals_common::Project> {
    let registry = Registry::load()?;
    let project = registry
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("project `{name}` is not registered"))?
        .clone();
    if !project.path.is_dir() {
        bail!(
            "project `{name}` path is missing: {}",
            project.path.display()
        );
    }
    if !is_safe_project_name(&project.name) {
        bail!("refusing unsafe project name `{}`", project.name);
    }
    Ok(project)
}

fn is_safe_project_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.contains('\0')
}

fn stop_if_running(project: &str) -> Result<()> {
    let running = match with_daemon(Request::Status)? {
        Response::Status { projects } => projects.iter().any(|p| p.name == project),
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected daemon response: {other:?}"),
    };
    if running {
        crate::cmd_down(project)?;
    }
    Ok(())
}

fn print_hard_warning(name: &str, project_path: &Path) {
    let targets = managed_wipe_targets(name, project_path);
    style::print_warn(&format!(
        "this will remove managed environment state for `{name}`"
    ));
    eprintln!();
    eprintln!("The following will be removed (if present):");
    for path in &targets {
        eprintln!("  {}", path.display());
    }
    eprintln!();
    eprintln!("Project source, devenv.lock, .env and external volumes will NOT be removed.");
    eprintln!("Note: databases under .devenv are wiped too.");
    eprintln!();
}

fn managed_wipe_targets(name: &str, project_path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        project_path.join(".devenv"),
        project_path.join(".neals"),
    ];
    for root in managed_runtime_roots() {
        paths.push(root.join(name));
    }
    paths
}

fn managed_runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(r) = runtime_dir() {
        roots.push(r);
    }
    let system = PathBuf::from("/run/neals");
    if Path::new(SYSTEM_DAEMON_SOCKET).exists() || system.is_dir() {
        if !roots.iter().any(|r| r == &system) {
            roots.push(system);
        }
    }
    roots
}

fn wipe_managed_state(name: &str, project_path: &Path) -> Result<()> {
    style::print_dim("removing managed environment state…");
    for path in managed_wipe_targets(name, project_path) {
        remove_managed_path(&path, name, project_path)?;
    }
    Ok(())
}

fn remove_managed_path(path: &Path, project: &str, project_root: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !is_allowed_wipe_path(path, project, project_root)? {
        bail!(
            "refusing to remove path that is not managed state: {}",
            path.display()
        );
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        style::print_dim(&format!("removed {}", path.display()));
    } else if path.is_file() || path.symlink_metadata().is_ok() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        style::print_dim(&format!("removed {}", path.display()));
    }
    Ok(())
}

fn is_allowed_wipe_path(path: &Path, project: &str, project_root: &Path) -> Result<bool> {
    let canon_root = fs::canonicalize(project_root)
        .with_context(|| format!("failed to resolve {}", project_root.display()))?;

    for leaf in [".devenv", ".neals"] {
        let expected = project_root.join(leaf);
        if paths_equal_loose(path, &expected) {
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    let parent_canon = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
                    if parent_canon != canon_root {
                        return Ok(false);
                    }
                }
            }
            return Ok(true);
        }
    }

    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Ok(false);
    };
    if name != project {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    Ok(managed_runtime_roots().iter().any(|root| parent == root))
}

fn paths_equal_loose(a: &Path, b: &Path) -> bool {
    a == b
}

fn update_inputs(project_dir: &Path) -> Result<()> {
    style::print_dim("Updating environment inputs (devenv.lock will be modified)…");
    let status = Command::new("devenv")
        .arg("update")
        .current_dir(project_dir)
        .stdin(Stdio::null())
        .status()
        .context("failed to run `devenv update` (is devenv on PATH?)")?;
    if !status.success() {
        bail!("`devenv update` failed with {status}");
    }
    style::print_ok("inputs updated");
    Ok(())
}

fn invalidate_and_evaluate(project_dir: &Path) -> Result<()> {
    style::print_dim("Re-evaluating environment…");
    let output = Command::new("devenv")
        .args(["--refresh-eval-cache", "eval", "devenv.runtime"])
        .current_dir(project_dir)
        .stdin(Stdio::null())
        .output()
        .context("failed to run `devenv eval` (is devenv on PATH?)")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        bail!(
            "environment evaluation failed{}",
            if err.is_empty() {
                String::new()
            } else {
                format!(": {err}")
            }
        );
    }
    style::print_ok("environment evaluated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neals-refresh-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn safe_project_names() {
        assert!(is_safe_project_name("demo"));
        assert!(is_safe_project_name("my-app"));
        assert!(!is_safe_project_name("../etc"));
        assert!(!is_safe_project_name("a/b"));
        assert!(!is_safe_project_name(""));
        assert!(!is_safe_project_name("."));
    }

    #[test]
    fn wipe_targets_include_devenv_neals_and_runtime() {
        let root = PathBuf::from("/tmp/proj");
        let targets = managed_wipe_targets("demo", &root);
        assert!(targets.iter().any(|p| p.ends_with(".devenv")));
        assert!(targets.iter().any(|p| p.ends_with(".neals")));
        assert!(targets.iter().any(|p| p.ends_with("demo")));
    }

    #[test]
    fn allows_devenv_under_project_root() {
        let root = temp_root("allow");
        fs::create_dir_all(&root).unwrap();
        let devenv = root.join(".devenv");
        fs::create_dir_all(&devenv).unwrap();
        assert!(is_allowed_wipe_path(&devenv, "demo", &root).unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_unrelated_path() {
        let root = temp_root("refuse");
        fs::create_dir_all(&root).unwrap();
        let other = root.join("src");
        fs::create_dir_all(&other).unwrap();
        assert!(!is_allowed_wipe_path(&other, "demo", &root).unwrap());
        let _ = fs::remove_dir_all(&root);
    }
}
