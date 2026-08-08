use crate::doctor;
use crate::live::run_live_view;
use crate::shell::enter_project_shell;
use crate::style;
use anyhow::{bail, Result};
use neals_common::Registry;
use std::io::{self, Write};
use std::process::ExitCode;

pub fn run_repl(yes: bool) -> Result<ExitCode> {
    style::print_ok("neals repl — type `help`, `quit` to exit");
    let stdin = io::stdin();
    loop {
        eprint!("{} ", style::accent("neals>"));
        io::stderr().flush().ok();
        let mut line = String::new();
        let n = stdin.read_line(&mut line)?;
        if n == 0 {
            println!();
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match dispatch(line, yes) {
            Ok(ReplAction::Continue) => {}
            Ok(ReplAction::Quit) => break,
            Err(err) => style::print_err(&format!("{err:#}")),
        }
    }
    Ok(ExitCode::SUCCESS)
}

enum ReplAction {
    Continue,
    Quit,
}

fn dispatch(line: &str, yes: bool) -> Result<ReplAction> {
    let mut parts = line.split_whitespace();
    let Some(cmd) = parts.next() else {
        return Ok(ReplAction::Continue);
    };
    let args: Vec<&str> = parts.collect();

    match cmd {
        "help" | "?" => {
            println!(
                "\
commands:
  help                 show this help
  list                 registered projects
  status               running projects
  up <project> [-d]    start (live view unless -d)
  down <project>       stop
  logs <project> [-f]  show logs (-f opens live view)
  bash <project>       project devenv shell
  doctor               environment checks
  quit / exit          leave the repl"
            );
            Ok(ReplAction::Continue)
        }
        "quit" | "exit" | "q" => Ok(ReplAction::Quit),
        "list" => {
            crate::cmd_list()?;
            Ok(ReplAction::Continue)
        }
        "status" => {
            crate::cmd_status()?;
            Ok(ReplAction::Continue)
        }
        "doctor" => {
            let _ = doctor::run_doctor()?;
            Ok(ReplAction::Continue)
        }
        "up" => {
            let project = args
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("usage: up <project> [-d]"))?;
            let detach = args.iter().any(|a| *a == "-d" || *a == "--detach");
            crate::cmd_up(project, detach)?;
            Ok(ReplAction::Continue)
        }
        "down" => {
            let project = args
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("usage: down <project>"))?;
            crate::cmd_down(project)?;
            Ok(ReplAction::Continue)
        }
        "logs" => {
            let project = args
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("usage: logs <project> [-f]"))?;
            let follow = args.iter().any(|a| *a == "-f" || *a == "--follow");
            if follow {
                let _ = run_live_view(project, false)?;
            } else {
                crate::logs::print_project_logs(project, false)?;
            }
            Ok(ReplAction::Continue)
        }
        "bash" => {
            let project = args
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("usage: bash <project>"))?;
            let path = project_path(project)?;
            let _ = enter_project_shell(project, &path)?;
            Ok(ReplAction::Continue)
        }
        "register" | "unregister" | "prune" => {
            let _ = yes;
            bail!("use `neals {cmd}` from the outer shell for registry changes");
        }
        other => {
            style::print_warn(&format!("unknown command `{other}` — try `help`"));
            Ok(ReplAction::Continue)
        }
    }
}

fn project_path(name: &str) -> Result<std::path::PathBuf> {
    let registry = Registry::load()?;
    match registry.get(name) {
        Some(project) => Ok(project.path.clone()),
        None => bail!("project `{name}` is not registered"),
    }
}
