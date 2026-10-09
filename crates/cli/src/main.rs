mod daemon_client;
mod doctor;
mod init;
mod live;
mod logs;
mod refresh;
mod shell;
mod style;

use anyhow::{bail, Context, Result};
use clap::{builder::styling, ColorChoice, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{
    engine::{ArgValueCompleter, CompletionCandidate},
    CompleteEnv,
};
use comfy_table::Cell;
use daemon_client::with_daemon;
use live::{run_live_view, LiveOutcome};
use neals_common::{resolve_project_name, Project, ProjectName, Registry, Request, Response};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

const LONG_ABOUT: &str = "\
Register devenv projects, run them under nealsd in per-project network
namespaces, reverse-proxy HTTP services via Caddy, and attach to logs or a
project shell.

Requires bubblewrap and slirp4netns. See neals(1) and
contrib/systemd/README.md for system (portless :80) install.
";

const AFTER_LONG_HELP: &str = "\
Directories:
  ~/.config/neals/projects.json     project registry
  ~/.local/state/neals/             logs, caddy.json, shell rc snippets
  $XDG_RUNTIME_DIR/neals/           ad-hoc IPC + sockets
  /run/neals/nealsd.sock            system daemon socket (if installed)
  <project>/.neals/                 convenience symlinks to UNIX sockets

Keys while following logs (neals up / logs -f):
  Ctrl+Q        detach (project keeps running)
  Ctrl+B        enter project shell (same as `neals bash`)
  Ctrl+C / X    stop the project (other follow tabs exit too)
";

#[derive(Parser)]
#[command(
    name = "neals",
    version,
    about = "Manage devenv projects in isolated network namespaces",
    long_about = LONG_ABOUT,
    after_long_help = AFTER_LONG_HELP,
    color = ColorChoice::Auto,
    styles = clap_styles()
)]
struct Cli {
    #[arg(short = 'y', long = "yes", global = true, help = "Assume yes for confirmations")]
    yes: bool,

    #[command(subcommand)]
    command: Commands,
}

fn clap_styles() -> styling::Styles {
    styling::Styles::styled()
        .header(styling::AnsiColor::Cyan.on_default().bold())
        .usage(styling::AnsiColor::Cyan.on_default().bold())
        .literal(styling::AnsiColor::Magenta.on_default().bold())
        .placeholder(styling::AnsiColor::BrightBlue.on_default())
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        about = "Initialize devenv.nix with a neals block",
        long_about = "\
If the current directory has no devenv.nix, runs `devenv init`. Then adds the
`neals` option stub and `neals = { name; services; }` block (name = folder).
Does nothing if that block is already present. Does not register the project."
    )]
    Init,

    #[command(
        about = "Register the current directory",
        long_about = "\
Reads `neals.name` from devenv.nix (folder name as fallback) and adds the
project to ~/.config/neals/projects.json."
    )]
    Register,

    #[command(about = "List registered projects")]
    List,

    #[command(about = "Unregister a project")]
    Unregister {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    #[command(about = "Remove ghost registry entries")]
    Prune,

    #[command(
        about = "Start a project and follow logs",
        long_about = "\
Start the project under nealsd, print HTTP routes, then follow process logs.\n\n\
Ctrl+Q detach (keeps running). Ctrl+B enter the project shell.\n\
Ctrl+C / Ctrl+X stop the project (other follow tabs exit too).\n\
Use -d/--detach to skip following logs."
    )]
    Up {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
        #[arg(short = 'd', long = "detach", help = "Do not follow logs after start")]
        detach: bool,
    },

    #[command(about = "Stop a running project")]
    Down {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    #[command(
        about = "Re-evaluate a project's devenv environment",
        long_about = "\
Stop the project if running and re-evaluate the devenv environment.\n\
Does not start the project — run `neals up` afterwards.\n\n\
--update runs `devenv update` (modifies devenv.lock) before evaluation.\n\
--hard also deletes managed state (.devenv, .neals, project runtime) after\n\
confirmation (use -y/--yes to skip). Does not remove source, devenv.lock,\n\
.env, or external volumes."
    )]
    Refresh {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
        #[arg(long = "update", help = "Run devenv update before evaluation")]
        update: bool,
        #[arg(
            long = "hard",
            help = "Wipe managed state (.devenv, .neals, runtime)"
        )]
        hard: bool,
    },

    #[command(about = "Show running projects and service ports")]
    Status,

    #[command(
        about = "Show or follow project logs",
        long_about = "\
Print the last 100 lines of the merged project log. With PROCESS, print that
process's stdout/stderr instead (devenv >= 2; project must be up). While
per-process files are not ready, -f tails the merged log until they appear.
With -f and no PROCESS, follow merged + all process logs (same as `neals up`)."
    )]
    Logs {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
        #[arg(
            value_name = "PROCESS",
            help = "Process name (per-process logs; devenv >= 2)",
            add = ArgValueCompleter::new(complete_processes)
        )]
        process: Option<String>,
        #[arg(short = 'f', long = "follow", help = "Follow log output")]
        follow: bool,
    },

    #[command(about = "Check host tools, paths, and daemon")]
    Doctor,

    #[command(
        about = "Show devenv info for a project",
        long_about = "\
Run `devenv info` in the registered project directory (host path; project
need not be up)."
    )]
    Info {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    #[command(
        name = "bash",
        about = "Open a shell in the project network namespace",
        long_about = "\
Enter a quiet `devenv shell` using $SHELL inside the project's network
namespace (project must be up). bash/zsh get a short prompt
`neals:<project>`; use `neals status` for host/guest ports."
    )]
    Bash {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    #[command(
        about = "Run a command in the project network namespace",
        long_about = "\
Run one command inside the project netns and devenv (project must be up).
Working directory is the project root; stdio is inherited; exit status is
the command's. Arguments are passed literally (no shell).\n\
\n\
  neals exec demo -- redis-cli ping\n\
  neals exec demo -- bash -lc 'cd be && make migrate'\n\
\n\
Use `neals bash` for an interactive session."
    )]
    Exec {
        #[arg(help = "Project name", add = ArgValueCompleter::new(complete_projects))]
        project: String,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            required = true,
            help = "Command and arguments"
        )]
        command: Vec<String>,
    },

    #[command(about = "Print shell completion code")]
    Completions {
        #[arg(help = "Target shell")]
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

fn complete_processes(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let Some(base) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Vec::new();
    };
    logs::running_process_names(std::path::Path::new(&base))
        .into_iter()
        .filter(|name| name.starts_with(prefix.as_ref()))
        .map(CompletionCandidate::new)
        .collect()
}

fn main() -> ExitCode {
    CompleteEnv::with_factory(Cli::command).complete();

    match run() {
        Ok(code) => code,
        Err(err) => {
            style::print_err(&format!("{err:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init => {
            init::run()?;
            Ok(ExitCode::SUCCESS)
        }
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
        Commands::Up { project, detach } => cmd_up(&project, detach),
        Commands::Down { project } => {
            cmd_down(&project)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Refresh {
            project,
            update,
            hard,
        } => refresh::run(&project, update, hard, cli.yes),
        Commands::Status => {
            cmd_status()?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Logs {
            project,
            process,
            follow,
        } => {
            match process.as_deref() {
                Some(process) => {
                    let path = project_path(&project)?;
                    logs::print_process_logs(&project, &path, process, follow)?;
                }
                None if follow => {
                    return after_live_view(&project, run_live_view(&project, false, None)?);
                }
                None => logs::print_project_logs(&project, false)?,
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Doctor => doctor::run_doctor(),
        Commands::Info { project } => {
            let path = project_path(&project)?;
            shell::run_project_info(&path)
        }
        Commands::Bash { project } => {
            let path = project_path(&project)?;
            shell::enter_project_shell(&project, &path)
        }
        Commands::Exec { project, command } => {
            let path = project_path(&project)?;
            shell::run_project_exec(&project, &path, &command)
        }
        Commands::Completions { shell } => {
            cmd_completions(shell)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(crate) fn confirm(prompt: &str, yes: bool) -> Result<bool> {
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
            style::print_warn(&format!(
                "no `neals.name` in devenv.nix; falling back to folder name `{name}`"
            ));
            style::eprint_dim(&format!(
                "services will be reachable at <service>.{name}.localhost"
            ));
            if !confirm("Register using this folder name?", yes)? {
                bail!("registration cancelled");
            }
            name.clone()
        }
    };

    let mut registry = Registry::load()?;
    if let Some(existing) = registry.get(&name) {
        if existing.path == path {
            style::print_ok(&format!("already registered `{name}` → {}", path.display()));
            return Ok(());
        }
        style::print_warn(&format!(
            "project `{name}` is already registered at {}",
            existing.path.display()
        ));
        style::eprint_dim(&format!("override with {}?", path.display()));
        if !confirm("Override existing registration?", yes)? {
            bail!("registration cancelled");
        }
        registry.upsert(Project {
            name: name.clone(),
            path: path.clone(),
        });
        registry.save()?;
        style::print_ok(&format!("overrode `{name}` → {}", path.display()));
        return Ok(());
    }

    registry.add(Project {
        name: name.clone(),
        path: path.clone(),
    })?;
    registry.save()?;
    style::print_ok(&format!("registered `{name}` → {}", path.display()));
    Ok(())
}

pub(crate) fn cmd_list() -> Result<()> {
    let registry = Registry::load()?;
    if registry.projects.is_empty() {
        style::print_dim("no projects registered");
        return Ok(());
    }

    let mut table = style::new_table();
    table.set_header(vec![
        style::header_cell("Name"),
        style::header_cell("Path"),
        style::header_cell("Status"),
    ]);
    for project in &registry.projects {
        let status = if project.is_ghost() {
            style::status_warn("ghost")
        } else {
            style::status_ok("ok")
        };
        table.add_row(vec![
            Cell::new(&project.name),
            Cell::new(project.path.display().to_string()),
            status,
        ]);
    }
    println!("{table}");
    Ok(())
}

fn cmd_unregister(name: &str) -> Result<()> {
    let mut registry = Registry::load()?;
    let removed = registry.remove(name)?;
    registry.save()?;
    style::print_ok(&format!(
        "unregistered `{}` (was {})",
        removed.name,
        removed.path.display()
    ));
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
        style::print_dim("nothing to prune");
        return Ok(());
    }

    style::print_warn("ghost projects:");
    for project in &ghosts {
        style::eprint_dim(&format!("  {} → {}", project.name, project.path.display()));
    }
    if !confirm(&format!("Remove {} ghost project(s)?", ghosts.len()), yes)? {
        bail!("prune cancelled");
    }

    let removed = registry.take_ghosts();
    registry.save()?;
    style::print_ok(&format!("pruned {} project(s)", removed.len()));
    Ok(())
}

pub(crate) fn cmd_up(project: &str, detach: bool) -> Result<ExitCode> {
    let merged_from = logs::project_log_len(project).unwrap_or(0);
    match with_daemon(Request::Up {
        project: project.to_string(),
    })? {
        Response::Ok => {
            style::print_ok(&format!("started `{project}`"));
            if let Ok(Response::Status { projects }) = with_daemon(Request::Status) {
                if let Some(p) = projects.iter().find(|p| p.name == project) {
                    for route in &p.routes {
                        println!("  → {}", style::accent(route));
                    }
                }
            }
            if detach {
                style::print_dim(&format!(
                    "detached; use `neals logs {project} -f` to follow"
                ));
                return Ok(ExitCode::SUCCESS);
            }
            after_live_view(project, run_live_view(project, true, Some(merged_from))?)
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
    }
}

fn after_live_view(project: &str, outcome: LiveOutcome) -> Result<ExitCode> {
    match outcome {
        LiveOutcome::Shell => {
            let path = project_path(project)?;
            shell::enter_project_shell(project, &path)
        }
        _ => Ok(ExitCode::SUCCESS),
    }
}

pub(crate) fn cmd_down(project: &str) -> Result<()> {
    match with_daemon(Request::Down {
        project: project.to_string(),
    })? {
        Response::Ok => {
            style::print_ok(&format!("stopped `{project}`"));
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
    }
}

pub(crate) fn cmd_status() -> Result<()> {
    match with_daemon(Request::Status)? {
        Response::Status { projects } => {
            if projects.is_empty() {
                style::print_dim("no projects running");
                return Ok(());
            }
            let mut table = style::new_table();
            table.set_header(vec![
                style::header_cell("Name"),
                style::header_cell("PID"),
                style::header_cell("Uptime"),
                style::header_cell("Services"),
            ]);
            for project in projects {
                let routes = if project.routes.is_empty() {
                    "-".into()
                } else {
                    project.routes.join("\n")
                };
                table.add_row(vec![
                    Cell::new(&project.name),
                    Cell::new(project.pid.to_string()),
                    Cell::new(format_uptime(project.uptime_secs)),
                    Cell::new(routes),
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
