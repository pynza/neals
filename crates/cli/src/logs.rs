use anyhow::{bail, Context, Result};
use neals_common::{ensure_dir, state_dir};
use std::fs;
use std::path::{Path, PathBuf};

pub const LOG_TAIL_LINES: usize = 100;

pub fn project_log_path(project: &str) -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join(format!("{project}.log")))
}

pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let lines: Vec<String> = content.lines().map(str::to_string).collect();
    let start = lines.len().saturating_sub(n);
    Ok(lines[start..].to_vec())
}

pub fn print_project_logs(project: &str) -> Result<()> {
    let path = project_log_path(project)?;
    if !path.is_file() {
        bail!(
            "no log file for project `{project}` at {} (has it been started with `neals up`?)",
            path.display()
        );
    }
    for line in tail_lines(&path, LOG_TAIL_LINES)? {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn tail_lines_returns_last_n() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-tail-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&tmp, "a\nb\nc\nd\ne\n").unwrap();
        let got = tail_lines(&tmp, 3).unwrap();
        assert_eq!(got, vec!["c", "d", "e"]);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn tail_lines_short_file() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-tail-short-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&tmp, "only\n").unwrap();
        let got = tail_lines(&tmp, 100).unwrap();
        assert_eq!(got, vec!["only"]);
        let _ = fs::remove_file(&tmp);
    }
}
