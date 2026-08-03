use anyhow::{bail, Context, Result};
use neals_common::{ensure_dir, state_dir};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const LOG_TAIL_LINES: usize = 100;
const TAIL_BLOCK: u64 = 8 * 1024;

pub fn project_log_path(project: &str) -> Result<PathBuf> {
    let dir = state_dir()?;
    ensure_dir(&dir)?;
    Ok(dir.join(format!("{project}.log")))
}

pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    if n == 0 {
        return Ok(Vec::new());
    }

    let mut file = File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut pos = file
        .seek(SeekFrom::End(0))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    if pos == 0 {
        return Ok(Vec::new());
    }

    file.seek(SeekFrom::End(-1))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let threshold = if last[0] == b'\n' {
        n
    } else {
        n.saturating_sub(1)
    };

    let mut newline_count = 0usize;
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    pos = file
        .seek(SeekFrom::End(0))
        .with_context(|| format!("failed to seek {}", path.display()))?;

    while pos > 0 && newline_count <= threshold {
        let size = pos.min(TAIL_BLOCK);
        pos -= size;
        file.seek(SeekFrom::Start(pos))
            .with_context(|| format!("failed to seek {}", path.display()))?;
        let mut block = vec![0u8; size as usize];
        file.read_exact(&mut block)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let mut cut = None;
        for (i, &byte) in block.iter().enumerate().rev() {
            if byte == b'\n' {
                newline_count += 1;
                if newline_count > threshold {
                    cut = Some(i + 1);
                    break;
                }
            }
        }

        if let Some(start) = cut {
            blocks.push(block[start..].to_vec());
            break;
        }
        blocks.push(block);
    }

    blocks.reverse();
    let bytes: Vec<u8> = blocks.into_iter().flatten().collect();
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.lines().map(str::to_string).collect())
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
    use std::fs;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neals-tail-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn tail_lines_returns_last_n() {
        let tmp = temp_path("last-n");
        fs::write(&tmp, "a\nb\nc\nd\ne\n").unwrap();
        let got = tail_lines(&tmp, 3).unwrap();
        assert_eq!(got, vec!["c", "d", "e"]);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn tail_lines_short_file() {
        let tmp = temp_path("short");
        fs::write(&tmp, "only\n").unwrap();
        let got = tail_lines(&tmp, 100).unwrap();
        assert_eq!(got, vec!["only"]);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn tail_lines_no_trailing_newline() {
        let tmp = temp_path("no-nl");
        fs::write(&tmp, "a\nb\nc").unwrap();
        let got = tail_lines(&tmp, 2).unwrap();
        assert_eq!(got, vec!["b", "c"]);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn tail_lines_large_file_reads_from_end() {
        let tmp = temp_path("large");
        let mut file = fs::File::create(&tmp).unwrap();
        for i in 0..50_000 {
            writeln!(file, "line-{i}").unwrap();
        }
        drop(file);
        let got = tail_lines(&tmp, 3).unwrap();
        assert_eq!(
            got,
            vec!["line-49997", "line-49998", "line-49999"]
        );
        let _ = fs::remove_file(&tmp);
    }
}
