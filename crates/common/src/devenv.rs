use anyhow::{bail, Context, Result};
use rnix::ast::{self, Attr, Expr, HasEntry, InterpolPart, LiteralKind};
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

/// How a declared Neals service is exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceKind {
    /// TCP: preferred start port (`None` = ephemeral, legacy `route = "tcp"`).
    Tcp {
        preferred_port: Option<u16>,
        proxy: bool,
    },
    Unix { socket_file: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDecl {
    pub service: String,
    pub kind: ServiceKind,
}

impl ServiceDecl {
    pub fn public_host(&self, project: &str) -> String {
        format!("{}.{project}.localhost", self.service)
    }

    pub fn is_unix(&self) -> bool {
        matches!(self.kind, ServiceKind::Unix { .. })
    }

    pub fn wants_proxy(&self) -> bool {
        match &self.kind {
            ServiceKind::Tcp { proxy, .. } => *proxy,
            ServiceKind::Unix { .. } => true,
        }
    }
}

/// Legacy route declaration (kept for older APIs/tests). Prefer [`ServiceDecl`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKind {
    Unix { socket_file: String },
    Tcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecl {
    pub service: String,
    pub kind: RouteKind,
}

impl RouteDecl {
    pub fn public_host(&self, project: &str) -> String {
        format!("{}.{project}.localhost", self.service)
    }
}

/// Normalize a service name for env vars: `api-backend` → `API_BACKEND`.
pub fn env_service_key(service: &str) -> String {
    service
        .chars()
        .map(|c| match c {
            '-' => '_',
            c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase(),
            _ => '_',
        })
        .collect()
}

/// Environment variable for an allocated TCP port: `redis` → `NEALS_REDIS_PORT`.
pub fn env_port_var(service: &str) -> String {
    format!("NEALS_{}_PORT", env_service_key(service))
}

/// Extract literal `neals.name = "..."` from Nix source via rnix AST.
///
/// ponytail: no `nix eval` — only string literals; expressions like `neals.name = lib.foo` are ignored.
pub fn parse_neals_name(src: &str) -> Option<String> {
    let bindings = collect_neals_literals(src).ok()?;
    let NixLit::Str(name) = bindings.get(&["name".into()][..])? else {
        return None;
    };
    if is_valid_project_name(name) {
        Some(name.clone())
    } else {
        None
    }
}

/// Extract `neals.services` (+ legacy `neals.route`) declarations.
pub fn parse_neals_services(src: &str) -> Result<Vec<ServiceDecl>> {
    let bindings = collect_neals_literals(src)?;
    let mut drafts: HashMap<String, ServiceDraft> = HashMap::new();

    for (path, value) in &bindings {
        if path.len() < 2 || path[0] != "services" {
            continue;
        }
        let service = &path[1];
        if !is_valid_service_name(service) {
            bail!("invalid neals.services service name `{service}`");
        }
        let draft = drafts.entry(service.clone()).or_default();

        if path.len() == 2 {
            bail!(
                "neals.services.{service} must be an attrset \
                 (e.g. {{ port = 6379; }} or {{ port = 8025; proxy = true; }})"
            );
        }
        if path.len() != 3 {
            bail!("unsupported neals.services path `{}`", path.join("."));
        }
        match path[2].as_str() {
            "port" => {
                let port = lit_port(value, &format!("neals.services.{service}.port"))?;
                if draft.port.is_some() {
                    bail!("duplicate neals.services.{service}.port");
                }
                draft.port = Some(port);
            }
            "proxy" => {
                let proxy = lit_bool(value, &format!("neals.services.{service}.proxy"))?;
                if draft.proxy.is_some() {
                    bail!("duplicate neals.services.{service}.proxy");
                }
                draft.proxy = Some(proxy);
            }
            "socket" => {
                let sock = lit_str(value, &format!("neals.services.{service}.socket"))?;
                if !is_valid_socket_file(&sock) {
                    bail!(
                        "invalid neals.services.{service}.socket `{sock}` \
                         (expected a bare filename like backend.sock)"
                    );
                }
                if draft.socket.is_some() {
                    bail!("duplicate neals.services.{service}.socket");
                }
                draft.socket = Some(sock);
            }
            other => bail!("unknown neals.services.{service} field `{other}`"),
        }
    }

    let mut services = Vec::new();
    for (service, draft) in drafts {
        services.push(draft.into_decl(service)?);
    }

    // Legacy neals.route.* → services (deprecated).
    let mut seen: HashMap<String, ()> = services
        .iter()
        .map(|s| (s.service.clone(), ()))
        .collect();

    for (path, value) in &bindings {
        if path.len() != 2 || path[0] != "route" {
            continue;
        }
        let service = &path[1];
        if !is_valid_service_name(service) {
            bail!("invalid neals.route service name `{service}`");
        }
        if seen.contains_key(service) {
            bail!(
                "neals.route.{service} conflicts with neals.services.{service} \
                 (use only neals.services)"
            );
        }
        let NixLit::Str(raw) = value else {
            bail!("neals.route.{service} must be a string literal");
        };
        let kind = if raw == "tcp" {
            ServiceKind::Tcp {
                preferred_port: None,
                proxy: true,
            }
        } else if is_valid_socket_file(raw) {
            ServiceKind::Unix {
                socket_file: raw.clone(),
            }
        } else {
            bail!(
                "invalid neals.route.{service} value `{raw}` \
                 (expected \"tcp\" or a bare filename like backend.sock)"
            );
        };
        seen.insert(service.clone(), ());
        services.push(ServiceDecl {
            service: service.clone(),
            kind,
        });
    }

    services.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(services)
}

/// Extract `neals.route.<service> = "file.sock" | "tcp"` (legacy).
pub fn parse_neals_routes(src: &str) -> Result<Vec<RouteDecl>> {
    Ok(parse_neals_services(src)?
        .into_iter()
        .map(|s| RouteDecl {
            service: s.service,
            kind: match s.kind {
                ServiceKind::Unix { socket_file } => RouteKind::Unix { socket_file },
                ServiceKind::Tcp { .. } => RouteKind::Tcp,
            },
        })
        .collect())
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
        && name != "tcp"
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

pub fn read_neals_services(project_dir: &Path) -> Result<Vec<ServiceDecl>> {
    let devenv_nix = project_dir.join("devenv.nix");
    if !devenv_nix.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&devenv_nix)
        .with_context(|| format!("failed to read {}", devenv_nix.display()))?;
    parse_neals_services(&content)
}

pub fn read_neals_routes(project_dir: &Path) -> Result<Vec<RouteDecl>> {
    Ok(read_neals_services(project_dir)?
        .into_iter()
        .map(|s| RouteDecl {
            service: s.service,
            kind: match s.kind {
                ServiceKind::Unix { socket_file } => RouteKind::Unix { socket_file },
                ServiceKind::Tcp { .. } => RouteKind::Tcp,
            },
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq)]
enum NixLit {
    Str(String),
    Int(i64),
    Bool(bool),
}

#[derive(Default)]
struct ServiceDraft {
    port: Option<u16>,
    proxy: Option<bool>,
    socket: Option<String>,
}

impl ServiceDraft {
    fn into_decl(self, service: String) -> Result<ServiceDecl> {
        match (self.port, self.socket) {
            (Some(port), None) => Ok(ServiceDecl {
                service,
                kind: ServiceKind::Tcp {
                    preferred_port: Some(port),
                    proxy: self.proxy.unwrap_or(false),
                },
            }),
            (None, Some(socket_file)) => {
                if self.proxy == Some(false) {
                    bail!(
                        "neals.services.{service}.socket cannot set proxy = false \
                         (UNIX services are always reverse-proxied)"
                    );
                }
                Ok(ServiceDecl {
                    service,
                    kind: ServiceKind::Unix { socket_file },
                })
            }
            (Some(_), Some(_)) => bail!(
                "neals.services.{service} cannot set both port and socket"
            ),
            (None, None) => bail!(
                "neals.services.{service} needs port = <n> or socket = \"file.sock\""
            ),
        }
    }
}

fn lit_port(value: &NixLit, path: &str) -> Result<u16> {
    let NixLit::Int(n) = value else {
        bail!("`{path}` must be an integer literal");
    };
    if *n < 1 || *n > 65535 {
        bail!("`{path}` must be in 1..=65535, got {n}");
    }
    Ok(*n as u16)
}

fn lit_bool(value: &NixLit, path: &str) -> Result<bool> {
    let NixLit::Bool(b) = value else {
        bail!("`{path}` must be a boolean literal (true/false)");
    };
    Ok(*b)
}

fn lit_str(value: &NixLit, path: &str) -> Result<String> {
    let NixLit::Str(s) = value else {
        bail!("`{path}` must be a string literal");
    };
    Ok(s.clone())
}

/// Map relative attr paths under `neals` → literal values.
fn collect_neals_literals(src: &str) -> Result<HashMap<Vec<String>, NixLit>> {
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
    out: &mut HashMap<Vec<String>, NixLit>,
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
                    insert_lit(out, full, NixLit::Str(lit))?;
                } else if requires_literal(&full) {
                    bail!(
                        "`{}` must be a string literal (no interpolation)",
                        full.join(".")
                    );
                }
            }
            Expr::Literal(lit) => match lit.kind() {
                LiteralKind::Integer(i) => {
                    let n = i.value().map_err(|e| {
                        anyhow::anyhow!("invalid integer at `{}`: {e}", full.join("."))
                    })?;
                    insert_lit(out, full, NixLit::Int(n))?;
                }
                _ if requires_literal(&full) => {
                    bail!("`{}` must be a supported literal", full.join("."));
                }
                _ => {}
            },
            Expr::Ident(ident) => {
                let Some(token) = ident.ident_token() else {
                    continue;
                };
                match token.text() {
                    "true" => insert_lit(out, full, NixLit::Bool(true))?,
                    "false" => insert_lit(out, full, NixLit::Bool(false))?,
                    _ if requires_literal(&full) => {
                        bail!("`{}` must be a literal", full.join("."));
                    }
                    _ => {}
                }
            }
            _ if requires_literal(&full) => {
                bail!("`{}` must be a literal", full.join("."));
            }
            _ => {}
        }
    }
    Ok(())
}

fn insert_lit(
    out: &mut HashMap<Vec<String>, NixLit>,
    full: Vec<String>,
    lit: NixLit,
) -> Result<()> {
    if let Some(prev) = out.insert(full.clone(), lit) {
        bail!(
            "duplicate Nix attr `{}` (previous value {:?})",
            full.join("."),
            prev
        );
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

fn requires_literal(path: &[String]) -> bool {
    path.len() >= 3
        && path[0] == "neals"
        && (path[1] == "route" || path[1] == "services" || path[1] == "name")
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
                kind: RouteKind::Unix {
                    socket_file: "backend.sock".into(),
                },
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
    fn parse_tcp_and_unix_mix() {
        let src = r#"
          {
            neals.route.web = "web.sock";
            neals.route.api = "tcp";
            neals.route.api-backend = "tcp";
          }
        "#;
        let routes = parse_neals_routes(src).unwrap();
        assert_eq!(routes.len(), 3);
        assert_eq!(
            routes.iter().find(|r| r.service == "api").unwrap().kind,
            RouteKind::Tcp
        );
        assert_eq!(
            routes
                .iter()
                .find(|r| r.service == "api-backend")
                .unwrap()
                .kind,
            RouteKind::Tcp
        );
    }

    #[test]
    fn env_service_key_normalizes() {
        assert_eq!(env_service_key("api"), "API");
        assert_eq!(env_service_key("api-backend"), "API_BACKEND");
        assert_eq!(env_service_key("web2"), "WEB2");
    }

    #[test]
    fn env_port_var_format() {
        assert_eq!(env_port_var("redis"), "NEALS_REDIS_PORT");
        assert_eq!(env_port_var("redis-insight"), "NEALS_REDIS_INSIGHT_PORT");
    }

    #[test]
    fn parse_services_preferred_and_proxy() {
        let src = r#"
          {
            neals.services.redis.port = 6379;
            neals.services.mailpit = { port = 8025; proxy = true; };
            neals.services.backend.socket = "backend.sock";
          }
        "#;
        let services = parse_neals_services(src).unwrap();
        assert_eq!(services.len(), 3);
        assert_eq!(
            services.iter().find(|s| s.service == "redis").unwrap().kind,
            ServiceKind::Tcp {
                preferred_port: Some(6379),
                proxy: false,
            }
        );
        assert_eq!(
            services
                .iter()
                .find(|s| s.service == "mailpit")
                .unwrap()
                .kind,
            ServiceKind::Tcp {
                preferred_port: Some(8025),
                proxy: true,
            }
        );
        assert_eq!(
            services
                .iter()
                .find(|s| s.service == "backend")
                .unwrap()
                .kind,
            ServiceKind::Unix {
                socket_file: "backend.sock".into(),
            }
        );
    }

    #[test]
    fn parse_services_merges_legacy_route() {
        let src = r#"
          {
            neals.services.redis.port = 6379;
            neals.route.api = "tcp";
          }
        "#;
        let services = parse_neals_services(src).unwrap();
        assert_eq!(services.len(), 2);
        let api = services.iter().find(|s| s.service == "api").unwrap();
        assert_eq!(
            api.kind,
            ServiceKind::Tcp {
                preferred_port: None,
                proxy: true,
            }
        );
    }

    #[test]
    fn parse_services_conflict_with_route() {
        let src = r#"
          {
            neals.services.api.port = 8000;
            neals.route.api = "tcp";
          }
        "#;
        assert!(parse_neals_services(src).is_err());
    }

    #[test]
    fn parse_services_rejects_port_and_socket() {
        let src = r#"{
          neals.services.x = { port = 1; socket = "x.sock"; };
        }"#;
        assert!(parse_neals_services(src).is_err());
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
