use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectName {
    FromDevenv(String),
    Fallback(String),
}

impl ProjectName {
    pub fn as_str(&self) -> &str {
        match self {
            Self::FromDevenv(name) | Self::Fallback(name) => name,
        }
    }

    pub fn is_fallback(&self) -> bool {
        matches!(self, Self::Fallback(_))
    }
}

pub fn parse_neals_name(src: &str) -> Option<String> {
    const KEY: &str = "neals.name";
    let mut search = src;
    while let Some(idx) = search.find(KEY) {
        let after_key = &search[idx + KEY.len()..];
        let trimmed = after_key.trim_start();
        let Some(after_eq) = trimmed.strip_prefix('=') else {
            search = after_key;
            continue;
        };
        let after_eq = after_eq.trim_start();
        let quote = match after_eq.chars().next() {
            Some('"') | Some('\'') => after_eq.chars().next().unwrap(),
            _ => {
                search = after_key;
                continue;
            }
        };
        let body = &after_eq[quote.len_utf8()..];
        let Some(end) = body.find(quote) else {
            search = after_key;
            continue;
        };
        let name = &body[..end];
        if is_valid_project_name(name) {
            return Some(name.to_string());
        }
        search = after_key;
    }
    None
}

pub fn is_valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

pub fn resolve_project_name(project_dir: &Path) -> Result<ProjectName> {
    let devenv_nix = project_dir.join("devenv.nix");
    if devenv_nix.is_file() {
        let content = fs::read_to_string(&devenv_nix)
            .with_context(|| format!("failed to read {}", devenv_nix.display()))?;
        if let Some(name) = parse_neals_name(&content) {
            return Ok(ProjectName::FromDevenv(name));
        }
    }

    let fallback = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("current directory has no valid name")?
        .to_string();
    if !is_valid_project_name(&fallback) {
        bail!(
            "folder name `{fallback}` is not a valid project name \
             (use lowercase letters, digits, hyphens; set neals.name in devenv.nix)"
        );
    }
    Ok(ProjectName::Fallback(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_double_quoted_name() {
        let src = r#"
          { pkgs, ... }: {
            neals.name = "ferrari";
          }
        "#;
        assert_eq!(parse_neals_name(src).as_deref(), Some("ferrari"));
    }

    #[test]
    fn parse_single_quoted_name() {
        assert_eq!(
            parse_neals_name("neals.name = 'my-app';").as_deref(),
            Some("my-app")
        );
    }

    #[test]
    fn parse_ignores_invalid_and_finds_later() {
        let src = r#"
            # neals.name = "BAD_NAME";
            neals.name = "ok-name";
        "#;
        assert_eq!(parse_neals_name(src).as_deref(), Some("ok-name"));
    }

    #[test]
    fn parse_missing() {
        assert_eq!(parse_neals_name("services.nginx.enable = true;"), None);
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_project_name("ferrari"));
        assert!(is_valid_project_name("my-app2"));
        assert!(!is_valid_project_name("MyApp"));
        assert!(!is_valid_project_name("-x"));
        assert!(!is_valid_project_name("x-"));
        assert!(!is_valid_project_name(""));
    }

    #[test]
    fn resolve_from_devenv_file() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-devenv-name-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("devenv.nix"), r#"neals.name = "from-nix";"#).unwrap();
        let got = resolve_project_name(&tmp).unwrap();
        assert_eq!(got, ProjectName::FromDevenv("from-nix".into()));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_fallback_without_neals_name() {
        let tmp = std::env::temp_dir().join(format!(
            "neals-fallback-proj-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("devenv.nix"), "{\n}\n").unwrap();
        let got = resolve_project_name(&tmp).unwrap();
        assert!(got.is_fallback());
        assert_eq!(got.as_str(), tmp.file_name().unwrap().to_str().unwrap());
        let _ = fs::remove_dir_all(&tmp);
    }
}
