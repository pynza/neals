use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Table};
use neals_common::{Project, Registry};
use std::env;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus, Stdio};

#[derive(Parser)]
#[command(name = "neals", about = "Local platform orchestrator for devenv projects")]
struct Cli {
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
        project: String,
    },
    /// Open an interactive devenv shell for a registered project
    Bash {
        /// Project name as shown by `neals list`
        project: String,
    },
    /// Run a command inside a registered project's devenv shell
    Exec {
        /// Project name as shown by `neals list`
        project: String,
        /// Command and args after `--`, e.g. `neals exec app -- npm test`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> ExitCode {
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
            cmd_register()?;
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
        Commands::Bash { project } => cmd_bash(&project),
        Commands::Exec { project, command } => cmd_exec(&project, &command),
    }
}

fn cmd_register() -> Result<()> {
    let cwd = env::current_dir().context("failed to get current directory")?;
    let path = cwd
        .canonicalize()
        .with_context(|| format!("failed to resolve path {}", cwd.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("current directory has no valid name")?
        .to_string();

    let mut registry = Registry::load()?;
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
    table.set_header(vec!["Name", "Path"]);
    for project in &registry.projects {
        table.add_row(vec![
            Cell::new(&project.name),
            Cell::new(project.path.display().to_string()),
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
