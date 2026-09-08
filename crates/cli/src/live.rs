use crate::daemon_client::with_daemon;
use crate::logs::{
    format_process_line, wait_for_log_file, LogFollower, ProcessLogMux, LOG_TAIL_LINES,
};
use crate::style;
use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use neals_common::{Registry, Request, Response};
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(200);
const DISCOVER_EVERY: u8 = 5; // ~1s at 200ms tick
const DEVENV_STREAM: &str = "devenv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutcome {
    Detached,
    Stopped,
    Exited,
    Shell,
}

struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub fn run_live_view(
    project: &str,
    from_start: bool,
    merged_from: Option<u64>,
) -> Result<LiveOutcome> {
    if !io::stdout().is_terminal() {
        return follow_plain(project, from_start, merged_from);
    }

    let project_path = resolve_project_path(project)?;
    style::print_dim("following logs — Ctrl+Q detach, Ctrl+B shell, Ctrl+C/X stop");
    enable_raw_mode().context("failed to enable raw mode")?;
    let _guard = RawGuard;

    let outcome = follow_loop(project, &project_path, from_start, merged_from)?;
    drop(_guard);

    match outcome {
        LiveOutcome::Detached => {
            style::print_dim(&format!(
                "detached from `{project}` (still running; `neals logs {project} -f` to reattach)"
            ));
        }
        LiveOutcome::Stopped => {
            style::print_ok(&format!("stopped `{project}`"));
        }
        LiveOutcome::Exited => {
            style::print_dim(&format!("`{project}` is no longer running"));
        }
        LiveOutcome::Shell => {
            style::print_dim(&format!("entering `{project}` shell"));
        }
    }
    Ok(outcome)
}

fn follow_plain(
    project: &str,
    from_start: bool,
    merged_from: Option<u64>,
) -> Result<LiveOutcome> {
    let project_path = resolve_project_path(project)?;
    follow_loop(project, &project_path, from_start, merged_from)
}

fn resolve_project_path(project: &str) -> Result<std::path::PathBuf> {
    let registry = Registry::load()?;
    match registry.get(project) {
        Some(p) => Ok(p.path.clone()),
        None => bail!("project `{project}` is not registered"),
    }
}

fn project_is_running(project: &str) -> Option<bool> {
    match with_daemon(Request::Status) {
        Ok(Response::Status { projects }) => {
            Some(projects.iter().any(|p| p.name == project))
        }
        _ => None,
    }
}

fn follow_loop(
    project: &str,
    project_path: &Path,
    from_start: bool,
    merged_from: Option<u64>,
) -> Result<LiveOutcome> {
    let raw = io::stdout().is_terminal();
    let mut mux: Option<ProcessLogMux> = None;
    let mut discover_ticks: u8 = 0;

    let path = wait_for_log_file(project)?;
    let mut merged = Some(attach_devenv_merged(&path, from_start, merged_from, raw)?);
    let mut devenv_width = DEVENV_STREAM.len();

    loop {
        if raw {
            if event::poll(TICK)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                            return Ok(LiveOutcome::Detached);
                        }
                        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                            return Ok(LiveOutcome::Shell);
                        }
                        (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                            let _ = with_daemon(Request::Down {
                                project: project.to_string(),
                            });
                            return Ok(LiveOutcome::Stopped);
                        }
                        _ => {}
                    }
                }
            }
        } else {
            std::thread::sleep(TICK);
        }

        discover_ticks = discover_ticks.wrapping_add(1);
        if discover_ticks % DISCOVER_EVERY == 0 {
            if project_is_running(project) == Some(false) {
                return Ok(LiveOutcome::Exited);
            }
            if mux.is_none() {
                if let Ok(rt) = neals_common::devenv::devenv_runtime(project_path) {
                    mux = Some(ProcessLogMux::new(rt));
                }
            }
            if let Some(mux) = mux.as_mut() {
                let was_empty = mux.is_empty();
                let initial = mux.refresh(None)?;
                if was_empty && !mux.is_empty() {
                    merged = None;
                }
                devenv_width = devenv_width.max(mux.width());
                for (name, line) in initial {
                    emit_line(raw, &format_process_line(&name, mux.width(), &line))?;
                }
            }
        }

        if let Some(mux) = mux.as_mut() {
            let width = mux.width().max(devenv_width);
            for (name, line) in mux.poll_lines()? {
                emit_line(raw, &format_process_line(&name, width, &line))?;
            }
        }
        if let Some(follower) = merged.as_mut() {
            let width = mux
                .as_ref()
                .map(|m| m.width().max(devenv_width))
                .unwrap_or(devenv_width);
            for line in follower.poll_lines()? {
                emit_line(raw, &format_process_line(DEVENV_STREAM, width, &line))?;
            }
        }
    }
}

fn attach_devenv_merged(
    path: &Path,
    from_start: bool,
    merged_from: Option<u64>,
    raw: bool,
) -> Result<LogFollower> {
    if let Some(offset) = merged_from {
        let mut follower = LogFollower::open_from_offset(path, offset)?;
        for line in follower.poll_lines()? {
            emit_line(raw, &format_process_line(DEVENV_STREAM, DEVENV_STREAM.len(), &line))?;
        }
        return Ok(follower);
    }

    if from_start {
        return LogFollower::open_at_end(path);
    }

    let (follower, lines) = LogFollower::open_with_tail(path, LOG_TAIL_LINES)?;
    for line in lines {
        emit_line(
            raw,
            &format_process_line(DEVENV_STREAM, DEVENV_STREAM.len(), &line),
        )?;
    }
    Ok(follower)
}

fn emit_line(raw: bool, line: &str) -> Result<()> {
    let mut out = io::stdout();
    if raw {
        write!(out, "{line}\r\n")?;
    } else {
        writeln!(out, "{line}")?;
    }
    out.flush().ok();
    Ok(())
}
