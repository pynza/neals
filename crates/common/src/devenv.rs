use anyhow::{bail, Context, Result};
use rnix::ast::{self, Attr, Expr, HasEntry, InterpolPart};
use rnix::Root;
use std::collections::HashMap;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecl {
    pub service: String,
    pub socket_file: String,
}

impl RouteDecl {
    pub fn public_host(&self, project: &str) -> String {
        format!("{}.{project}.localhost", self.service)
    }
}

/// Extract literal `neals.name = "..."` from Nix source via rnix AST.
///
/// ponytail: no `nix eval` — only string literals; expressions like `neals.name = lib.foo` are ignored.
pub fn parse_neals_name(src: &str) -> Option<String> {
    let bindings = collect_neals_bindings(src).ok()?;
    let name = bindings.get(&["name".into()][..])?;
    if is_valid_project_name(name) {
        Some(name.clone())
    } else {
        None
    }
}

/// Extract `neals.route.<service> = "file.sock"` literal bindings.
pub fn parse_neals_routes(src: &str) -> Result<Vec<RouteDecl>> {
    let bindings = collect_neals_bindings(src)?;
    let mut routes = Vec::new();
    let mut seen = HashMap::new();

    for (path, value) in bindings {
        if path.len() != 2 || path[0] != "route" {
            continue;
        }
        let service = &path[1];
        if !is_valid_service_name(service) {
            bail!("invalid neals.route service name `{service}`");
        }
        if !is_valid_socket_file(&value) {
            bail!(
                "invalid neals.route.{service} socket file `{value}` \
                 (expected a bare filename like backend.sock)"
            );
        }
        if let Some(prev) = seen.insert(service.clone(), value.clone()) {
            bail!("duplicate neals.route.{service} (`{prev}` and `{value}`)");
        }
        routes.push(RouteDecl {
            service: service.clone(),
            socket_file: value.clone(),
        });
    }

    routes.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(routes)
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

pub fn is_valid_service_name(name: &str) -> bool {
    is_valid_project_name(name)
}

pub fn is_valid_socket_file(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\0')
        && name != "."
        && name != ".."
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

pub fn read_neals_routes(project_dir: &Path) -> Result<Vec<RouteDecl>> {
    let devenv_nix = project_dir.join("devenv.nix");
    if !devenv_nix.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&devenv_nix)
        .with_context(|| format!("failed to read {}", devenv_nix.display()))?;
    parse_neals_routes(&content)
}

/// Map relative attr paths under `neals` → literal string values.
fn collect_neals_bindings(src: &str) -> Result<HashMap<Vec<String>, String>> {
    let root = Root::parse(src);
    let Some(expr) = root.tree().expr() else {
        return Ok(HashMap::new());
    };
    let Some(set) = top_level_attrset(&expr) else {
        return Ok(HashMap::new());
    };

    let mut out = HashMap::new();
    walk_attrset(&set, &[], &mut out)?;
    Ok(out
        .into_iter()
        .filter_map(|(path, value)| {
            path.strip_prefix(&["neals".to_string()][..])
                .map(|rest| (rest.to_vec(), value))
        })
        .collect())
}

fn top_level_attrset(expr: &Expr) -> Option<ast::AttrSet> {
    match expr {
        Expr::AttrSet(set) => Some(set.clone()),
        Expr::Lambda(lambda) => match lambda.body()? {
            Expr::AttrSet(set) => Some(set),
            other => top_level_attrset(&other),
        },
        Expr::With(with) => top_level_attrset(&with.body()?),
        Expr::LetIn(let_in) => top_level_attrset(&let_in.body()?),
        _ => None,
    }
}

fn walk_attrset(
    set: &ast::AttrSet,
    prefix: &[String],
    out: &mut HashMap<Vec<String>, String>,
) -> Result<()> {
    for apv in set.attrpath_values() {
        let Some(attrpath) = apv.attrpath() else {
            continue;
        };
        let Some(mut path) = attr_path_idents(&attrpath) else {
            continue;
        };
        let mut full = prefix.to_vec();
        full.append(&mut path);

        let Some(value) = apv.value() else {
            continue;
        };
        match value {
            Expr::AttrSet(inner) => walk_attrset(&inner, &full, out)?,
            Expr::Str(s) => {
                if let Some(lit) = literal_string(&s) {
                    if let Some(prev) = out.insert(full.clone(), lit.clone()) {
                        bail!(
                            "duplicate Nix attr `{}` (`{prev}` and `{lit}`)",
                            full.join(".")
                        );
                    }
                } else if is_neals_route_path(&full) {
                    bail!(
                        "`{}` must be a string literal (no interpolation)",
                        full.join(".")
                    );
                }
            }
            _ if is_neals_route_path(&full) => {
                bail!("`{}` must be a string literal", full.join("."));
            }
            _ => {}
        }
    }
    Ok(())
}

fn attr_path_idents(attrpath: &ast::Attrpath) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for attr in attrpath.attrs() {
        match attr {
            Attr::Ident(ident) => {
                let token = ident.ident_token()?;
                parts.push(token.text().to_string());
            }
            Attr::Str(s) => {
                parts.push(literal_string(&s)?);
            }
            Attr::Dynamic(_) => return None,
        }
    }
    Some(parts)
}

fn is_neals_route_path(path: &[String]) -> bool {
    path.len() >= 3 && path[0] == "neals" && path[1] == "route"
}

fn literal_string(s: &ast::Str) -> Option<String> {
    let mut out = String::new();
    for part in s.normalized_parts() {
        match part {
            InterpolPart::Literal(text) => out.push_str(&text),
            InterpolPart::Interpolation(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_double_quoted_name() {
        let src = r#"
          { pkgs, ... }: {
            neals.name = "demo";
          }
        "#;
        assert_eq!(parse_neals_name(src).as_deref(), Some("demo"));
    }

    #[test]
    fn parse_double_quoted_name_only() {
        // Nix strings are "..." or ''...''; single-quoted '...' is not a string.
        assert_eq!(
            parse_neals_name(r#"{ neals.name = "my-app"; }"#).as_deref(),
            Some("my-app")
        );
        assert_eq!(parse_neals_name("{ neals.name = 'my-app'; }"), None);
    }

    #[test]
    fn parse_ignores_invalid_and_finds_later() {
        let src = r#"
            {
              # neals.name = "BAD_NAME";
              neals.name = "ok-name";
            }
        "#;
        assert_eq!(parse_neals_name(src).as_deref(), Some("ok-name"));
    }

    #[test]
    fn parse_name_hash_inside_string() {
        let src = r#"neals.name = "ok#name";"#;
        // invalid project name due to `#`
        assert_eq!(parse_neals_name(src), None);
    }

    #[test]
    fn parse_missing() {
        assert_eq!(parse_neals_name("services.nginx.enable = true;"), None);
    }

    #[test]
    fn parse_nested_neals_attrset() {
        let src = r#"
          {
            neals = {
              name = "demo";
              route = {
                backend = "backend.sock";
              };
            };
          }
        "#;
        assert_eq!(parse_neals_name(src).as_deref(), Some("demo"));
        let routes = parse_neals_routes(src).unwrap();
        assert_eq!(
            routes,
            vec![RouteDecl {
                service: "backend".into(),
                socket_file: "backend.sock".into(),
            }]
        );
    }

    #[test]
    fn parse_dotted_routes() {
        let src = r#"
          { pkgs, ... }: {
            neals.name = "demo";
            neals.route.backend = "backend.sock";
            neals.route.web = "web.sock";
          }
        "#;
        let routes = parse_neals_routes(src).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].service, "backend");
        assert_eq!(routes[1].service, "web");
    }

    #[test]
    fn parse_routes_ignores_commented() {
        let src = r#"
          {
            # neals.route.ghost = "ghost.sock";
            neals.route.backend = "backend.sock";
          }
        "#;
        let routes = parse_neals_routes(src).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].service, "backend");
    }

    #[test]
    fn parse_routes_duplicate_errors() {
        let src = r#"
          {
            neals.route.backend = "a.sock";
            neals.route.backend = "b.sock";
          }
        "#;
        assert!(parse_neals_routes(src).is_err());
    }

    #[test]
    fn parse_routes_rejects_path_socket() {
        let src = r#"{ neals.route.backend = "../escape.sock"; }"#;
        assert!(parse_neals_routes(src).is_err());
        let src = r#"{ neals.route.backend = "dir/backend.sock"; }"#;
        assert!(parse_neals_routes(src).is_err());
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_project_name("demo"));
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
        fs::write(tmp.join("devenv.nix"), r#"{ neals.name = "from-nix"; }"#).unwrap();
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
