use crate::daemon_client::with_daemon;
use crate::style;
use anyhow::{bail, Context, Result};
use neals_common::{ensure_dir, state_dir, Request, Response};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

const WATCH_TICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    Bash,
    Zsh,
    Other,
}

pub fn enter_project_shell(project: &str, path: &Path) -> Result<ExitCode> {
    let netns_pid = require_netns_pid(project)?;
    let kind = detect_shell();
    let shell_path = shell_executable();

    match kind {
        ShellKind::Bash => {
            let rc = write_bash_rc(project)?;
            run_watched_shell(
                path,
                project,
                netns_pid,
                &[
                    "--quiet",
                    "shell",
                    &shell_path,
                    "-i",
                    "--rcfile",
                    rc.to_str().context("rc path not utf-8")?,
                ],
                &[],
            )
        }
        ShellKind::Zsh => {
            let zdot = write_zsh_dir(project)?;
            run_watched_shell(
                path,
                project,
                netns_pid,
                &["--quiet", "shell", &shell_path, "-i"],
                &[("ZDOTDIR", zdot.as_os_str())],
            )
        }
        ShellKind::Other => {
            style::print_warn(&format!(
                "branded prompt not configured for `{}`",
                shell_path
            ));
            run_watched_shell(
                path,
                project,
                netns_pid,
                &["--quiet", "shell", &shell_path, "-i"],
                &[],
            )
        }
    }
}

pub fn run_project_exec(project: &str, path: &Path, command: &[String]) -> Result<ExitCode> {
    if command.is_empty() {
        bail!("no command provided");
    }
    let netns_pid = require_netns_pid(project)?;
    let script = command
        .iter()
        .map(|arg| neals_common::shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let status = nsenter_devenv_stdin(
        path,
        project,
        netns_pid,
        &["--quiet", "shell", "bash"],
        &format!("{script}\n"),
    )?;
    Ok(exit_code_from_status(status))
}

fn require_netns_pid(project: &str) -> Result<u32> {
    match with_daemon(Request::Status)? {
        Response::Status { projects } => {
            let Some(p) = projects.iter().find(|p| p.name == project) else {
                bail!("project `{project}` is not running; start it with `neals up {project}`");
            };
            if p.netns_pid == 0 {
                bail!("project `{project}` has no netns pid (daemon too old?)");
            }
            Ok(p.netns_pid)
        }
        Response::Error { message } => bail!(message),
        _ => bail!("unexpected daemon response for Status"),
    }
}

fn detect_shell() -> ShellKind {
    let name = shell_executable();
    let base = Path::new(&name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("bash");
    match base {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        _ => ShellKind::Other,
    }
}

fn shell_executable() -> String {
    env::var("SHELL").unwrap_or_else(|_| "bash".into())
}

fn write_bash_rc(project: &str) -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("bashrc-{project}"));
    let home = env::var("HOME").unwrap_or_default();
    let content = format!(
        r#"if [ -f "{home}/.bashrc" ]; then
  . "{home}/.bashrc"
fi
export NEALS_PROJECT="{project}"
if [ -t 1 ] && [ -z "${{NO_COLOR:-}}" ]; then
  PS1='\[\e[35m\]neals:{project}\[\e[0m\] \w \$ '
else
  PS1='neals:{project} \w \$ '
fi
if [ -t 1 ]; then
  clear 2>/dev/null || printf '\033[H\033[2J'
fi
"#
    );
    fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn write_zsh_dir(project: &str) -> Result<PathBuf> {
    let dir = state_dir()?.join(format!("zsh-{project}"));
    ensure_dir(&dir)?;
    let home = env::var("HOME").unwrap_or_default();
    let content = format!(
        r#"if [ -f "{home}/.zshrc" ]; then
  source "{home}/.zshrc"
fi
export NEALS_PROJECT="{project}"
if [[ -o interactive ]] && [[ -z "${{NO_COLOR:-}}" ]]; then
  PROMPT="%F{{magenta}}neals:{project}%f %~ %# "
else
  PROMPT="neals:{project} %~ %# "
fi
if [[ -t 1 ]]; then
  clear 2>/dev/null || printf '\033[H\033[2J'
fi
"#
    );
    let zshrc = dir.join(".zshrc");
    fs::write(&zshrc, content).with_context(|| format!("failed to write {}", zshrc.display()))?;
    Ok(dir)
}

fn run_watched_shell(
    dir: &Path,
    project: &str,
    netns_pid: u32,
    devenv_args: &[&str],
    extra_env: &[(&str, &std::ffi::OsStr)],
) -> Result<ExitCode> {
    let mut child = spawn_nsenter_devenv(dir, project, netns_pid, devenv_args, extra_env)?;

    loop {
        match child.try_wait().context("waiting for project shell")? {
            Some(status) => return Ok(exit_code_from_status(status)),
            None => {
                if !project_session_alive(project, netns_pid) {
                    let _ = child.kill();
                    let _ = child.wait();
                    style::eprint_dim(&format!(
                        "`{project}` stopped — left the project shell"
                    ));
                    return Ok(ExitCode::SUCCESS);
                }
                thread::sleep(WATCH_TICK);
            }
        }
    }
}

fn project_session_alive(project: &str, netns_pid: u32) -> bool {
    if !Path::new(&format!("/proc/{netns_pid}")).exists() {
        return false;
    }
    match with_daemon(Request::Status) {
        Ok(Response::Status { projects }) => projects.iter().any(|p| p.name == project),
        _ => true,
    }
}

fn spawn_nsenter_devenv(
    dir: &Path,
    project: &str,
    netns_pid: u32,
    devenv_args: &[&str],
    extra_env: &[(&str, &std::ffi::OsStr)],
) -> Result<Child> {
    let mut cmd = nsenter_command(dir, project, netns_pid);
    cmd.args(devenv_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn()
        .context("failed to run `nsenter`/`devenv` (is util-linux + devenv on PATH?)")
}

fn nsenter_command(dir: &Path, project: &str, netns_pid: u32) -> Command {
    let mut cmd = Command::new("nsenter");
    cmd.args([
        "--user",
        "--net",
        "--mount",
        "--preserve-credentials",
        "-t",
        &netns_pid.to_string(),
        &format!("--wdns={}", dir.display()),
        "--",
        "devenv",
    ])
    .current_dir(dir)
    .env("NEALS_PROJECT", project);
    cmd
}

fn nsenter_devenv_stdin(
    dir: &Path,
    project: &str,
    netns_pid: u32,
    devenv_args: &[&str],
    script: &str,
) -> Result<ExitStatus> {
    use std::io::Write;
    let mut child = nsenter_command(dir, project, netns_pid)
        .args(devenv_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to run `nsenter`/`devenv` (is util-linux + devenv on PATH?)")?;
    child
        .stdin
        .take()
        .context("devenv shell has no stdin")?
        .write_all(script.as_bytes())
        .context("failed to write the command into devenv shell stdin")?;
    child.wait().context("waiting for devenv shell")
}

fn exit_code_from_status(status: ExitStatus) -> ExitCode {
    match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_rc_clears_and_sets_project() {
        let dir = std::env::temp_dir().join(format!(
            "neals-bashrc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_STATE_HOME", &dir);
        let path = write_bash_rc("demo").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("NEALS_PROJECT=\"demo\""));
        assert!(text.contains("clear") || text.contains("\\033[H\\033[2J"));
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn project_session_alive_false_when_netns_missing() {
        assert!(!project_session_alive("anything", 4_294_967_293));
    }
}
