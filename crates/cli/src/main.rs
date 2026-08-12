mod daemon_client;
mod doctor;
mod live;
mod logs;
mod repl;
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
use live::run_live_view;
use neals_common::{
    resolve_project_name, Project, ProjectName, Registry, Request, Response,
};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

const LONG_ABOUT: &str = "\
Neals orchestrates local devenv projects: registry, lifecycle (up/down),
per-project network namespaces, HTTP routes via Caddy, and branded shells.

Typical flow:
  neals register
  neals up my-app          # live view: routes + logs
  # browser → http://api.my-app.localhost/  (system daemon)
  #        or http://api.my-app.localhost:2015/  (ad-hoc)
  neals bash my-app        # same netns as the running project

Requires bubblewrap + slirp4netns. System daemon for portless :80 URLs:
  see contrib/systemd/README.md
";

const AFTER_HELP: &str = "\
Directories:
  ~/.config/neals/projects.json     project registry
  ~/.local/state/neals/             logs, caddy.json, shell rc snippets
  $XDG_RUNTIME_DIR/neals/           ad-hoc IPC + sockets
  /run/neals/nealsd.sock            system daemon socket (if installed)
  <project>/.neals/                 convenience symlinks to UNIX sockets

Keys in the live view (neals up / logs -f):
  Ctrl+C / q    detach (project keeps running)
  Ctrl+X        stop the project
";

#[derive(Parser)]
#[command(
    name = "neals",
    version,
    about = "Local platform orchestrator for devenv projects",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    color = ColorChoice::Auto,
    styles = clap_styles()
)]
struct Cli {
    // Skip confirmation prompts
    #[arg(short = 'y', long = "yes", global = true)]
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
    // Register the current directory in the global project registry
    #[command(long_about = "\
Reads `neals.name` from devenv.nix (folder name as fallback) and adds the
project to ~/.config/neals/projects.json.")]
    Register,

    // List registered projects
    List,

    // Remove a project from the registry
    Unregister {
        // Project name as shown by `neals list`
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    // Remove registry entries whose paths no longer exist
    Prune,

    // Start a project (`devenv up`) and open the live log view
    #[command(long_about = "\
Starts the project under nealsd, prints HTTP routes, then opens a live view
with sticky route URLs and scrolling logs.\n\n\
Ctrl+C / q detach (keeps running). Ctrl+X stops the project.\n\
Use -d/--detach to skip the live view.")]
    Up {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
        // Start without opening the live view
        #[arg(short = 'd', long = "detach")]
        detach: bool,
    },

    // Stop a running project
    Down {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    // Show projects currently running under nealsd
    Status,

    // Show a project's daemon log
    #[command(long_about = "\
Prints the last 100 log lines. With -f/--follow, opens the same live view
as `neals up` (routes header + scrolling logs).")]
    Logs {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
        // Open the live view and follow new lines
        #[arg(short = 'f', long = "follow")]
        follow: bool,
    },

    // Check that required tools and directories are available
    Doctor,

    // Open an interactive shell in the project's devenv
    #[command(name = "bash", long_about = "\
Enters a quiet `devenv shell` using $SHELL inside the project's network
namespace (project must be up). bash/zsh get a short prompt
`neals:<project>`; use `neals status` for host/guest ports.")]
    Bash {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
    },

    // Run a command inside a project's devenv shell
    Exec {
        #[arg(add = ArgValueCompleter::new(complete_projects))]
        project: String,
        // Command and args after `--`, e.g. `neals exec app -- npm test`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },

    // Interactive command loop (list, up, logs, …)
    Repl,

    // Print shell completion setup for bash, zsh, fish, elvish, or powershell
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
            style::print_err(&format!("{err:#}"));
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
        Commands::Up { project, detach } => {
            cmd_up(&project, detach)?;
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
        Commands::Logs { project, follow } => {
            if follow {
                let _ = run_live_view(&project, false)?;
            } else {
                logs::print_project_logs(&project, false)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Doctor => doctor::run_doctor(),
        Commands::Bash { project } => {
            let path = project_path(&project)?;
            shell::enter_project_shell(&project, &path)
        }
        Commands::Exec { project, command } => {
            let path = project_path(&project)?;
            shell::run_project_exec(&project, &path, &command)
        }
        Commands::Repl => repl::run_repl(cli.yes),
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

pub(crate) fn cmd_up(project: &str, detach: bool) -> Result<()> {
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
                    "detached; use `neals logs {project} -f` or `neals repl` to follow"
                ));
                return Ok(());
            }
            let _ = run_live_view(project, true)?;
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response from nealsd: {other:?}"),
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
