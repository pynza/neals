use crate::xdg::{ensure_dir, state_dir};
use anyhow::{Context, Result};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

pub const BEGIN_MARKER: &str = "# BEGIN neals";
pub const END_MARKER: &str = "# END neals";
pub const SYSTEM_HOSTS_MIRROR: &str = "/run/neals/hosts";

pub fn render_host_block(entries: &[(Ipv4Addr, String)]) -> String {
    let mut lines = vec![BEGIN_MARKER.to_string()];
    for (ip, name) in entries {
        lines.push(format!("{ip} {name}"));
    }
    lines.push(END_MARKER.to_string());
    lines.join("\n") + "\n"
}

pub fn merge_hosts_file(existing: &str, block: &str) -> String {
    let stripped = strip_neals_block(existing);
    let mut out = stripped.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    if !block.trim().is_empty() && block.contains(BEGIN_MARKER) {
        out.push('\n');
        out.push_str(block.trim_end());
        out.push('\n');
    }
    out
}

fn strip_neals_block(existing: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in existing.lines() {
        if line.trim() == BEGIN_MARKER {
            skipping = true;
            continue;
        }
        if line.trim() == END_MARKER {
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn state_hosts_path() -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join("hosts"))
}

pub fn system_hosts_path() -> PathBuf {
    std::env::var_os("NEALS_HOSTS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/hosts"))
}

pub fn apply_hosts_file(path: &Path, block: &str) -> Result<()> {
    let existing = if path.exists() {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let merged = merge_hosts_file(&existing, block);
    let tmp = path.with_extension("neals.tmp");
    std::fs::write(&tmp, &merged).with_context(|| format!("failed to write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("failed to replace {}", path.display())
    })?;
    Ok(())
}

pub fn read_hosts_fragment(path: &Path) -> Result<String> {
    if path.exists() {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))
    } else {
        Ok(String::new())
    }
}

pub fn apply_from_state() -> Result<()> {
    apply_hosts_fragment_file(&state_hosts_path()?)
}

pub fn apply_hosts_fragment_file(fragment: &Path) -> Result<()> {
    let block = read_hosts_fragment(fragment)?;
    apply_hosts_file(&system_hosts_path(), &block)
}

/// Best-effort copy for the system path unit (`neals-hosts-sync.path`).
/// Returns whether the mirror was written.
pub fn mirror_system_hosts_fragment(block: &str) -> bool {
    let path = Path::new(SYSTEM_HOSTS_MIRROR);
    let Some(parent) = path.parent() else {
        return false;
    };
    if !parent.is_dir() {
        return false;
    }
    std::fs::write(path, block).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_previous_block() {
        let existing = "\
127.0.0.1 localhost
# BEGIN neals
127.53.0.1 old_db
# END neals
";
        let block = render_host_block(&[(Ipv4Addr::new(127, 53, 0, 2), "demo_db".into())]);
        let merged = merge_hosts_file(existing, &block);
        assert!(merged.contains("127.0.0.1 localhost"));
        assert!(merged.contains("127.53.0.2 demo_db"));
        assert!(!merged.contains("old_db"));
        assert_eq!(merged.matches(BEGIN_MARKER).count(), 1);
    }

    #[test]
    fn merge_clears_block_when_empty() {
        let existing = "\
127.0.0.1 localhost
# BEGIN neals
127.53.0.1 demo_db
# END neals
";
        let merged = merge_hosts_file(existing, "");
        assert!(merged.contains("127.0.0.1 localhost"));
        assert!(!merged.contains("BEGIN neals"));
        assert!(!merged.contains("demo_db"));
    }
}
