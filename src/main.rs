mod herdr;
mod history;
mod rows;

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListState, Paragraph};
use serde_json::{Value, json};

use rows::{Mode, Row, Target};

const ACCENT: Color = Color::Cyan;
const PREVIEW_LINES: usize = 10;

/// Indices into `rows` sorted by group then score, plus matched char positions for highlighting.
fn filter(rows: &[Row], query: &str, matcher: &mut Matcher) -> Vec<(usize, Vec<u32>)> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(u8, u32, usize, Vec<u32>)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let mut idx = Vec::new();
            let score = pattern.indices(Utf32Str::new(&r.text, &mut buf), matcher, &mut idx)?;
            idx.sort_unstable();
            idx.dedup();
            Some((r.group, score, i, idx))
        })
        .collect();
    // Ties keep the builder's order (recent first, current last).
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().map(|(_, _, i, idx)| (i, idx)).collect()
}

fn status_color(status: &str) -> Color {
    match status {
        "blocked" => Color::Red,
        "working" => Color::Yellow,
        "idle" | "done" => Color::Green,
        _ => Color::DarkGray,
    }
}

/// Status dot, then the name in normal text, context dimmed, matched chars in accent.
fn render_row(row: &Row, idx: &[u32]) -> Line<'static> {
    let hl = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled("● ", Style::new().fg(status_color(&row.status)))];
    spans.extend(row.text.chars().enumerate().map(|(i, c)| {
        let st = if idx.binary_search(&(i as u32)).is_ok() {
            hl
        } else if i < row.name_len {
            Style::new()
        } else {
            Style::new().fg(Color::DarkGray)
        };
        Span::styled(c.to_string(), st)
    }));
    Line::from(spans)
}

/// zoxide may not be on the PATH herdr hands to plugin panes, so try the usual install spots too.
fn zoxide_dirs(home: &str) -> Option<Vec<String>> {
    let candidates = ["zoxide".to_string(), "/opt/homebrew/bin/zoxide".into(), "/usr/local/bin/zoxide".into(),
        format!("{home}/.local/bin/zoxide"), format!("{home}/.cargo/bin/zoxide")];
    candidates.iter().find_map(|bin| {
        let out = std::process::Command::new(bin).args(["query", "-l"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect())
    })
}

/// Enter on a row. Ok means the picker can close.
fn activate(target: &Target) -> Result<Value, String> {
    match target {
        Target::Ws(id) | Target::Dir { ws: Some(id), .. } => herdr::rpc("workspace.focus", json!({"workspace_id": id})),
        Target::Tab(id) => herdr::rpc("tab.focus", json!({"tab_id": id})),
        Target::Pane { tab, pane } => {
            herdr::rpc("tab.focus", json!({"tab_id": tab}))?;
            herdr::rpc("pane.focus", json!({"pane_id": pane}))
        }
        Target::Dir { path, ws: None } => {
            let label = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);
            herdr::rpc("workspace.create", json!({"cwd": path, "label": label, "focus": true}))
        }
    }
}

enum Prompt {
    Search,
    Rename(Target, String),
    Confirm(Target, String),
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("record") {
        let event = std::env::var("HERDR_PLUGIN_EVENT_JSON").unwrap_or_default();
        if let Err(e) = history::record(&event) {
            eprintln!("herdr-ws record: {e}");
        }
        return;
    }

    let mut snap = match herdr::snapshot() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("herdr-ws: {e}");
            std::process::exit(1);
        }
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let mut hist = history::load();
    let mut dirs: Option<Option<Vec<String>>> = None; // loaded on first visit to / mode
    let (tx, rx) = mpsc::channel();
    herdr::subscribe(tx);

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut input = String::new();
    let mut prompt = Prompt::Search;
    let mut msg: Option<String> = None;
    let mut sticky: Option<Target> = None; // keep the selection on this row across reloads
    let mut previews: HashMap<String, (Instant, Vec<String>)> = HashMap::new();
    let mut state = ListState::default().with_selected(Some(0));
    let mut term = ratatui::init();

    loop {
        let (mode, query) = rows::parse_input(&input);
        let query = query.to_string();
        if mode == Mode::Dir && dirs.is_none() {
            dirs = Some(zoxide_dirs(&home));
        }
        let dir_list = dirs.as_ref().and_then(|d| d.as_deref()).unwrap_or(&[]);
        let rows = rows::build(mode, &snap, &hist, dir_list, &home);
        let hits = filter(&rows, &query, &mut matcher);
        if let Some(t) = sticky.take() {
            if let Some(pos) = hits.iter().position(|(i, _)| rows[*i].target == t) {
                state.select(Some(pos));
            }
        }
        if state.selected().is_some_and(|s| s >= hits.len()) {
            state.select(Some(hits.len().saturating_sub(1)));
        }
        let selected = state.selected().and_then(|s| hits.get(s)).map(|(i, _)| &rows[*i]);

        let preview = match selected.and_then(|r| r.pane.as_ref()) {
            Some(p) => {
                let fresh = previews.get(p).is_some_and(|(t, _)| t.elapsed() < Duration::from_secs(1));
                if !fresh {
                    previews.insert(p.clone(), (Instant::now(), herdr::read_tail(p, PREVIEW_LINES)));
                }
                previews[p].1.clone()
            }
            None => vec![],
        };

        let _ = term.draw(|f| {
            let area = f.area(); // herdr sizes the popup (herdr-plugin.toml), the box fills it
            let dim = Style::new().fg(Color::DarkGray);
            let chip = |m: Mode| {
                if m == mode { Span::styled(format!(" {} ", m.title()), Style::new().fg(Color::Black).bg(ACCENT).bold()) }
                else { Span::styled(format!(" {} ", m.title()), dim) }
            };
            let mut chips = vec![Span::raw(" ")];
            chips.extend(Mode::ALL.map(chip));
            chips.push(Span::raw(" "));
            let footer = match &msg {
                Some(m) => Span::styled(format!(" {m} "), Style::new().fg(Color::Red)),
                None => Span::styled(" ↵ go · tab mode · ^r rename · ^x close · esc ", dim),
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Line::from(chips))
                .title(Line::from(Span::styled(format!(" {}/{} ", hits.len(), rows.len()), dim)).right_aligned())
                .title_bottom(Line::from(footer).centered());
            let inner = block.inner(area);
            f.render_widget(Clear, area);
            f.render_widget(block, area);

            let show_preview = inner.height >= PREVIEW_LINES as u16 + 8;
            let preview_h = if show_preview { PREVIEW_LINES as u16 + 1 } else { 0 };
            let [line, sep, list, pv] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(preview_h),
            ])
            .areas(inner);

            let accent = Style::new().fg(ACCENT);
            let (spans, cursor) = match &prompt {
                Prompt::Search => (vec![Span::styled(" › ", accent), Span::raw(query.to_string())], Some(3 + query.chars().count())),
                Prompt::Rename(_, buf) => {
                    (vec![Span::styled(" rename › ", accent), Span::raw(buf.clone())], Some(10 + buf.chars().count()))
                }
                Prompt::Confirm(_, name) => {
                    (vec![Span::styled(format!(" close {name}? y/n"), Style::new().fg(Color::Red).bold())], None)
                }
            };
            f.render_widget(Paragraph::new(Line::from(spans)), line);
            if let Some(x) = cursor {
                f.set_cursor_position((line.x + x as u16, line.y));
            }
            f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(dim), sep);

            if mode == Mode::Dir && matches!(dirs, Some(None)) {
                f.render_widget(Paragraph::new("  zoxide not found").style(dim), list);
            } else {
                let items: Vec<Line> = hits.iter().map(|(i, idx)| render_row(&rows[*i], idx)).collect();
                let list_w = List::new(items)
                    .highlight_style(Style::new().bg(Color::Indexed(237)))
                    .highlight_symbol(Span::styled(" ▍", accent))
                    .highlight_spacing(HighlightSpacing::Always);
                f.render_stateful_widget(list_w, list, &mut state);
            }

            if show_preview {
                let mut lines = vec![Line::styled("─".repeat(pv.width as usize), dim)];
                lines.extend(preview.iter().map(|l| Line::styled(format!("  {l}"), dim)));
                f.render_widget(Paragraph::new(lines), pv);
            }
        });

        let mut reload = false;
        if event::poll(Duration::from_millis(250)).unwrap_or(false) {
            let Ok(Event::Key(k)) = event::read() else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            msg = None;
            match &mut prompt {
                Prompt::Confirm(target, _) => {
                    if k.code == KeyCode::Char('y') {
                        let r = match target {
                            Target::Ws(id) => herdr::rpc("workspace.close", json!({"workspace_id": id})),
                            Target::Tab(id) => herdr::rpc("tab.close", json!({"tab_id": id})),
                            _ => Ok(Value::Null),
                        };
                        msg = r.err();
                        reload = true;
                    }
                    prompt = Prompt::Search;
                }
                Prompt::Rename(target, buf) => match k.code {
                    KeyCode::Esc => prompt = Prompt::Search,
                    KeyCode::Backspace => _ = buf.pop(),
                    KeyCode::Char(c) if !ctrl => buf.push(c),
                    KeyCode::Enter => {
                        let r = match target {
                            Target::Ws(id) => herdr::rpc("workspace.rename", json!({"workspace_id": id, "label": buf})),
                            Target::Tab(id) => herdr::rpc("tab.rename", json!({"tab_id": id, "label": buf})),
                            _ => Ok(Value::Null),
                        };
                        msg = r.err();
                        sticky = Some(target.clone());
                        prompt = Prompt::Search;
                        reload = true;
                    }
                    _ => {}
                },
                Prompt::Search => match k.code {
                    KeyCode::Esc => break,
                    KeyCode::Char('c') if ctrl => break,
                    KeyCode::Enter => {
                        let r = match selected {
                            Some(row) => activate(&row.target),
                            None if query.is_empty() => continue,
                            // Nothing matched: create one named after the query.
                            None => match mode {
                                Mode::Ws => herdr::rpc("workspace.create", json!({"label": query, "focus": true})),
                                Mode::Tab => herdr::rpc(
                                    "tab.create",
                                    json!({"label": query, "workspace_id": snap["focused_workspace_id"], "focus": true}),
                                ),
                                _ => continue,
                            },
                        };
                        match r {
                            Ok(_) => break,
                            Err(e) => msg = Some(e),
                        }
                    }
                    KeyCode::Char('r') if ctrl => {
                        if let Some(row) = selected.filter(|r| matches!(r.target, Target::Ws(_) | Target::Tab(_))) {
                            prompt = Prompt::Rename(row.target.clone(), row.label.clone());
                        }
                    }
                    KeyCode::Char('x') if ctrl => {
                        if let Some(row) = selected.filter(|r| matches!(r.target, Target::Ws(_) | Target::Tab(_))) {
                            prompt = Prompt::Confirm(row.target.clone(), row.text[..].chars().take(row.name_len).collect());
                        }
                    }
                    KeyCode::Up => state.select_previous(),
                    KeyCode::Char('p') if ctrl => state.select_previous(),
                    KeyCode::Down => state.select_next(),
                    KeyCode::Char('n') if ctrl => state.select_next(),
                    KeyCode::Tab => {
                        input = format!("{}{query}", mode.next().prefix());
                        state.select(Some(0));
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        state.select(Some(0));
                    }
                    KeyCode::Char(c) if !ctrl => {
                        input.push(c);
                        state.select(Some(0));
                    }
                    _ => {}
                },
            }
        }

        // Live refresh: herdr pushed an event (agent status, tab/workspace changes).
        if rx.try_iter().count() > 0 || reload {
            if sticky.is_none() {
                sticky = selected.map(|r| r.target.clone());
            }
            if let Ok(s) = herdr::snapshot() {
                snap = s;
            }
            hist = history::load();
            previews.clear();
        }
    }
    ratatui::restore();
}
