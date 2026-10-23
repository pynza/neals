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
    if hosts_file_equivalent(&existing, &merged) {
        return Ok(());
    }
    write_in_place(path, &merged)
}

fn hosts_file_equivalent(a: &str, b: &str) -> bool {
    a.trim_end() == b.trim_end()
}

fn write_in_place(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
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

/// Best-effort copy for the system path unit. Skips the write when content is
/// unchanged so PathModified does not fire in a loop.
///
/// Only runs when `NEALS_MODE=system` (set by `nealsd@.service`). That keeps
/// `cargo test` / ad-hoc daemons from clobbering the live `/run/neals/hosts`
/// mirror on a developer machine.
pub fn mirror_system_hosts_fragment(block: &str) -> bool {
    if std::env::var_os("NEALS_MODE").as_deref() != Some(std::ffi::OsStr::new("system")) {
        return false;
    }
    let path = Path::new(SYSTEM_HOSTS_MIRROR);
    let Some(parent) = path.parent() else {
        return false;
    };
    if !parent.is_dir() {
        return false;
    }
    if path.is_file() {
        if let Ok(existing) = std::fs::read_to_string(path) {
            if existing == block {
                return true;
            }
        }
    }
    let tmp = parent.join("hosts.neals.tmp");
    if std::fs::write(&tmp, block).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "neals-hosts-{tag}-{}-{nanos}",
            std::process::id()
        ));
        ensure_dir(&dir).unwrap();
        dir
    }

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

    #[test]
    fn apply_is_idempotent_without_touching_mtime() {
        let dir = tmp_dir("idem");
        let hosts = dir.join("hosts");
        let block = render_host_block(&[(Ipv4Addr::new(127, 53, 0, 1), "demo_db".into())]);

        apply_hosts_file(&hosts, &block).unwrap();
        let mtime1 = std::fs::metadata(&hosts).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        apply_hosts_file(&hosts, &block).unwrap();
        let mtime2 = std::fs::metadata(&hosts).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "second apply must not rewrite");

        let text = std::fs::read_to_string(&hosts).unwrap();
        assert!(text.contains("demo_db"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_updates_when_block_changes() {
        let dir = tmp_dir("chg");
        let hosts = dir.join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();

        let a = render_host_block(&[(Ipv4Addr::new(127, 53, 0, 1), "demo_db".into())]);
        apply_hosts_file(&hosts, &a).unwrap();
        assert!(std::fs::read_to_string(&hosts).unwrap().contains("demo_db"));

        let b = render_host_block(&[(Ipv4Addr::new(127, 53, 0, 2), "demo_redis".into())]);
        apply_hosts_file(&hosts, &b).unwrap();
        let text = std::fs::read_to_string(&hosts).unwrap();
        assert!(text.contains("demo_redis"));
        assert!(!text.contains("demo_db"));
        assert!(text.contains("127.0.0.1 localhost"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mirror_skipped_without_system_mode() {
        let prev = std::env::var_os("NEALS_MODE");
        std::env::remove_var("NEALS_MODE");
        assert!(
            !mirror_system_hosts_fragment("# BEGIN neals\n"),
            "must not touch /run/neals/hosts outside NEALS_MODE=system"
        );
        match prev {
            Some(v) => std::env::set_var("NEALS_MODE", v),
            None => std::env::remove_var("NEALS_MODE"),
        }
    }
}
