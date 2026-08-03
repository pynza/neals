mod daemon_client;
mod logs;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{
    engine::{ArgValueCompleter, CompletionCandidate},
    CompleteEnv,
};
use comfy_table::{Cell, Table};
use daemon_client::with_daemon;
use logs::print_project_logs;
use neals_common::{
    resolve_project_name, Project, ProjectName, Registry, Request, Response,
};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus, Stdio};

#[derive(Parser)]
#[command(name = "neals", about = "Local platform orchestrator for devenv projects")]
struct Cli {
    /// Skip confirmation prompts
    #[arg(short = 'y', long = "yes", global = true)]
    yes: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register the current directory in the global project registry
    Register,
    /// List registered projects
    List,
    /// Remove a project from the registry
    Unregister {
        /// Project name as shown by `neals list`
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },
    /// Remove registry entries whose paths no longer exist
    Prune,
    /// Start a project via nealsd (`devenv up`)
    Up {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },
    /// Stop a project via nealsd
    Down {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },
    /// Show projects currently running under nealsd
    Status,
    /// Print the last lines of a project's daemon log
    Logs {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },
    /// Open an interactive devenv shell for a registered project
    Bash {
        /// Project name as shown by `neals list`
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },
    /// Run a command inside a registered project's devenv shell
    Exec {
        /// Project name as shown by `neals list`
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
        /// Command and args after `--`, e.g. `neals exec app -- npm test`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Print shell completion setup for bash, zsh, fish, elvish, or powershell
    Completions {
        shell: CompletionShell,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    Powershell,
    Zsh,
}

fn complete_projects(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    Registry::load()
        .map(|r| r.projects)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.name.starts_with(prefix.as_ref()))
        .map(|p| CompletionCandidate::new(p.name))
        .collect()
}

fn main() -> ExitCode {
    CompleteEnv::with_factory(Cli::command).complete();

    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Register => {
            cmd_register(cli.yes)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::List => {
            cmd_list()?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Unregister { project } => {
            cmd_unregister(&project)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Prune => {
            cmd_prune(cli.yes)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Up { project } => {
            cmd_up(&project)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Down { project } => {
            cmd_down(&project)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Status => {
            cmd_status()?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Logs { project } => {
            print_project_logs(&project)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Bash { project } => cmd_bash(&project),
        Commands::Exec { project, command } => cmd_exec(&project, &command),
        Commands::Completions { shell } => {
            cmd_completions(shell)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        bail!("refusing to prompt without a TTY; re-run with --yes");
    }
    eprint!("{prompt} [y/N] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read confirmation")?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

fn cmd_register(yes: bool) -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    let path = cwd
        .canonicalize()
        .with_context(|| format!("failed to resolve path {}", cwd.display()))?;

    let resolved = resolve_project_name(&path)?;
    let name = match &resolved {
        ProjectName::FromDevenv(name) => name.clone(),
        ProjectName::Fallback(name) => {
            eprintln!("warning: no `neals.name` in devenv.nix; falling back to folder name `{name}`");
            eprintln!("         services will be reachable at <service>.{name}.localhost");
            if !confirm("Register using this folder name?", yes)? {
                bail!("registration cancelled");
            }
            name.clone()
        }
    };

    let mut registry = Registry::load()?;
    if let Some(existing) = registry.get(&name) {
        if existing.path == path {
            println!("already registered `{name}` -> {}", path.display());
            return Ok(());
        }
        eprintln!(
            "warning: project `{name}` is already registered at {}",
            existing.path.display()
        );
        eprintln!("         override with {}?", path.display());
        if !confirm("Override existing registration?", yes)? {
            bail!("registration cancelled");
        }
        registry.upsert(Project {
            name: name.clone(),
            path: path.clone(),
        });
        registry.save()?;
        println!("overrode `{name}` -> {}", path.display());
        return Ok(());
    }

    registry.add(Project {
        name: name.clone(),
        path: path.clone(),
    })?;
    registry.save()?;
    println!("registered `{name}` -> {}", path.display());
    Ok(())
}

fn cmd_list() -> Result<()> {
    let registry = Registry::load()?;
    if registry.projects.is_empty() {
        println!("no projects registered");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["Name", "Path", "Status"]);
    for project in &registry.projects {
        let status = if project.is_ghost() {
            "ghost"
        } else {
            "ok"
        };
        table.add_row(vec![
            Cell::new(&project.name),
            Cell::new(project.path.display().to_string()),
            Cell::new(status),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn cmd_unregister(name: &str) -> Result<()> {
    let mut registry = Registry::load()?;
    let removed = registry.remove(name)?;
    registry.save()?;
    println!(
        "unregistered `{}` (was {})",
        removed.name,
        removed.path.display()
    );
    Ok(())
}

fn cmd_prune(yes: bool) -> Result<()> {
    let mut registry = Registry::load()?;
    let ghosts: Vec<_> = registry
        .projects
        .iter()
        .filter(|p| p.is_ghost())
        .cloned()
        .collect();
    if ghosts.is_empty() {
        println!("nothing to prune");
        return Ok(());
    }

    eprintln!("ghost projects:");
    for project in &ghosts {
        eprintln!("  {} -> {}", project.name, project.path.display());
    }
    if !confirm(&format!("Remove {} ghost project(s)?", ghosts.len()), yes)? {
        bail!("prune cancelled");
    }

    let removed = registry.take_ghosts();
    registry.save()?;
    println!("pruned {} project(s)", removed.len());
    Ok(())
}

fn cmd_up(project: &str) -> Result<()> {
    match with_daemon(Request::Up {
        project: project.to_string(),
    })? {
        Response::Ok => {
            println!("started `{project}`");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
    }
}

fn cmd_down(project: &str) -> Result<()> {
    match with_daemon(Request::Down {
        project: project.to_string(),
    })? {
        Response::Ok => {
            println!("stopped `{project}`");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
    }
}

fn cmd_status() -> Result<()> {
    match with_daemon(Request::Status)? {
        Response::Status { projects } => {
            if projects.is_empty() {
                println!("no projects running");
                return Ok(());
            }
            let mut table = Table::new();
            table.set_header(vec!["Name", "PID", "Uptime"]);
            for project in projects {
                table.add_row(vec![
                    Cell::new(&project.name),
                    Cell::new(project.pid.to_string()),
                    Cell::new(format_uptime(project.uptime_secs)),
                ]);
            }
            println!("{table}");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
    }
}

fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn cmd_completions(shell: CompletionShell) -> Result<()> {
    let line = match shell {
        CompletionShell::Bash => "source <(COMPLETE=bash neals)",
        CompletionShell::Zsh => "source <(COMPLETE=zsh neals)",
        CompletionShell::Fish => "COMPLETE=fish neals | source",
        CompletionShell::Elvish => "eval (E:COMPLETE=elvish neals | slurp)",
        CompletionShell::Powershell => {
            "$env:COMPLETE = \"powershell\"; neals | Out-String | Invoke-Expression; Remove-Item Env:\\COMPLETE"
        }
    };
    println!("{line}");
    Ok(())
}

fn project_path(name: &str) -> Result<std::path::PathBuf> {
    let registry = Registry::load()?;
    match registry.get(name) {
        Some(project) => Ok(project.path.clone()),
        None => bail!("project `{name}` is not registered"),
    }
}

fn exit_code_from_status(status: ExitStatus) -> ExitCode {
    match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    }
}

fn run_devenv(dir: &Path, args: &[&str]) -> Result<ExitCode> {
    let status = Command::new("devenv")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run `devenv` (is it installed and on PATH?)")?;
    Ok(exit_code_from_status(status))
}

fn cmd_bash(project: &str) -> Result<ExitCode> {
    let path = project_path(project)?;
    run_devenv(&path, &["shell"])
}

fn cmd_exec(project: &str, command: &[String]) -> Result<ExitCode> {
    if command.is_empty() {
        bail!("no command provided");
    }
    let path = project_path(project)?;
    let mut args: Vec<&str> = vec!["shell", "--"];
    args.extend(command.iter().map(String::as_str));
    run_devenv(&path, &args)
}

#[cfg(test)]
mod format_tests {
    use super::format_uptime;

    #[test]
    fn format_uptime_examples() {
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m 5s");
        assert_eq!(format_uptime(3661), "1h 1m");
    }
}
