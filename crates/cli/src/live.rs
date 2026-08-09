use crate::daemon_client::with_daemon;
use crate::logs::{wait_for_log_file, LogFollower, LOG_TAIL_LINES};
use crate::style;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use neals_common::{ProjectRuntime, Request, Response};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

const MAX_LINES: usize = 2000;
const TICK: Duration = Duration::from_millis(200);
const STATUS_REFRESH: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutcome {
    Detached,
    Stopped,
}

pub fn run_live_view(project: &str, from_start: bool) -> Result<LiveOutcome> {
    if !io::stdout().is_terminal() {
        if from_start {
            crate::logs::follow_project_logs(project)?;
        } else {
            crate::logs::print_project_logs(project, true)?;
        }
        return Ok(LiveOutcome::Detached);
    }

    let path = wait_for_log_file(project)?;
    let (mut follower, initial) = if from_start {
        (LogFollower::open_at_end(&path)?, Vec::new())
    } else {
        LogFollower::open_with_tail(&path, LOG_TAIL_LINES)?
    };

    let mut lines: VecDeque<String> = initial.into();
    let mut meta = fetch_meta(project);
    let mut last_status = Instant::now();

    let mut terminal = ratatui::init();
    let result = (|| -> Result<LiveOutcome> {
        loop {
            terminal.draw(|frame| draw(frame, project, &meta, &lines))?;

            if event::poll(TICK)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Char('q'), KeyModifiers::NONE)
                        | (KeyCode::Esc, _) => {
                            return Ok(LiveOutcome::Detached);
                        }
                        (KeyCode::Char('x'), KeyModifiers::CONTROL) => {
                            let _ = with_daemon(Request::Down {
                                project: project.to_string(),
                            });
                            return Ok(LiveOutcome::Stopped);
                        }
                        _ => {}
                    }
                }
            }

            for line in follower.poll_lines()? {
                push_line(&mut lines, line);
            }

            if last_status.elapsed() >= STATUS_REFRESH {
                meta = fetch_meta(project);
                last_status = Instant::now();
            }
        }
    })();

    ratatui::restore();

    match &result {
        Ok(LiveOutcome::Detached) => {
            style::print_dim(&format!(
                "detached from `{project}` (still running; `neals logs {project} -f` to reattach)"
            ));
        }
        Ok(LiveOutcome::Stopped) => {
            style::print_ok(&format!("stopped `{project}`"));
        }
        Err(_) => {}
    }

    result
}

fn push_line(buf: &mut VecDeque<String>, line: String) {
    buf.push_back(line);
    while buf.len() > MAX_LINES {
        buf.pop_front();
    }
}

fn fetch_meta(project: &str) -> Option<ProjectRuntime> {
    match with_daemon(Request::Status) {
        Ok(Response::Status { projects }) => projects.into_iter().find(|p| p.name == project),
        _ => None,
    }
}

fn draw(frame: &mut Frame, project: &str, meta: &Option<ProjectRuntime>, lines: &VecDeque<String>) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height(meta)),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    let header = Paragraph::new(header_text(project, meta))
        .style(Style::default().fg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .title(Span::styled(
                    " neals ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(header, chunks[0]);

    let visible = visible_lines(lines, chunks[1].height as usize);
    let items: Vec<ListItem> = visible
        .iter()
        .map(|l| ListItem::new(Line::raw(l.as_str())))
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .title(Span::styled(" logs ", Style::default().fg(Color::DarkGray))),
    );
    frame.render_widget(list, chunks[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Ctrl+C", Style::default().fg(Color::Yellow)),
        Span::raw("/"),
        Span::styled("q", Style::default().fg(Color::Yellow)),
        Span::raw(" detach  "),
        Span::styled("Ctrl+X", Style::default().fg(Color::Red)),
        Span::raw(" stop project"),
    ]))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, chunks[2]);
}

fn header_height(meta: &Option<ProjectRuntime>) -> u16 {
    let routes = meta.as_ref().map(|m| m.routes.len()).unwrap_or(0);
    (2 + routes.max(1) + 1).min(12) as u16
}

fn header_text(project: &str, meta: &Option<ProjectRuntime>) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    match meta {
        Some(m) => {
            out.push(Line::from(vec![
                Span::styled(
                    format!(" {project} "),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("pid {}  up {}", m.pid, format_uptime(m.uptime_secs))),
            ]));
            if m.routes.is_empty() {
                out.push(Line::from(Span::styled(
                    "  (no services declared)",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for route in &m.routes {
                    out.push(Line::from(vec![
                        Span::raw("  → "),
                        Span::styled(route.clone(), Style::default().fg(Color::Green)),
                    ]));
                }
            }
        }
        None => {
            out.push(Line::from(vec![
                Span::styled(
                    format!(" {project} "),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("not running", Style::default().fg(Color::Yellow)),
            ]));
            out.push(Line::from(Span::styled(
                "  waiting for status…",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    out
}

fn visible_lines(lines: &VecDeque<String>, height: usize) -> Vec<&String> {
    if height == 0 {
        return Vec::new();
    }
    let skip = lines.len().saturating_sub(height);
    lines.iter().skip(skip).collect()
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
