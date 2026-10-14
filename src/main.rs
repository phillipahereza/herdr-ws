use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListState, Paragraph};
use serde_json::{Value, json};

struct Row {
    text: String,
    focus: (&'static str, &'static str, String), // (method, param, id)
    focused: bool,
}

fn socket_path() -> String {
    std::env::var("HERDR_SOCKET_PATH").unwrap_or_else(|_| {
        format!("{}/.config/herdr/herdr.sock", std::env::var("HOME").unwrap_or_default())
    })
}

fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let mut s = UnixStream::connect(socket_path()).map_err(|e| format!("connect: {e}"))?;
    writeln!(s, "{}", json!({"id": "ws", "method": method, "params": params})).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match v.get("error") {
        Some(e) => Err(format!("{method}: {e}")),
        None => Ok(v["result"].take()),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

/// Context that tells same-named tabs apart: cwd of the tab's focused (else first) pane, plus agent names.
fn tab_context(tab_id: &str, panes: &[Value], home: &str) -> String {
    let in_tab: Vec<&Value> = panes.iter().filter(|p| s(p, "tab_id") == tab_id).collect();
    let pane = in_tab.iter().find(|p| p["focused"] == true).or(in_tab.first());
    let cwd = pane
        .map(|p| if s(p, "foreground_cwd").is_empty() { s(p, "cwd") } else { s(p, "foreground_cwd") })
        .unwrap_or("");
    let cwd = match cwd.strip_prefix(home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => cwd.to_string(),
    };
    let mut agents: Vec<&str> = in_tab.iter().map(|p| s(p, "agent")).filter(|a| !a.is_empty()).collect();
    agents.dedup();
    if agents.is_empty() { cwd } else { format!("{cwd}  [{}]", agents.join(",")) }
}

fn build_rows(workspaces: &Value, tabs: &Value, panes: &Value, home: &str) -> (Vec<Row>, Vec<Row>) {
    let empty = vec![];
    let ws = workspaces["workspaces"].as_array().unwrap_or(&empty);
    let tabs = tabs["tabs"].as_array().unwrap_or(&empty);
    let panes = panes["panes"].as_array().unwrap_or(&empty);

    let mut ws_rows: Vec<Row> = ws
        .iter()
        .map(|w| Row {
            text: format!(
                "{} {}  {}  · {} tabs · {}",
                w["number"], s(w, "label"), tab_context(s(w, "active_tab_id"), panes, home),
                w["tab_count"], s(w, "agent_status")
            ),
            focus: ("workspace.focus", "workspace_id", s(w, "workspace_id").into()),
            focused: w["focused"] == true,
        })
        .collect();

    let mut tab_rows: Vec<Row> = tabs
        .iter()
        .map(|t| {
            let w = ws.iter().find(|w| s(w, "workspace_id") == s(t, "workspace_id"));
            let (wl, wn) = w.map_or(("?", Value::Null), |w| (s(w, "label"), w["number"].clone()));
            Row {
                text: format!(
                    "{wl}#{wn} › {} {}  {}  · {}",
                    t["number"], s(t, "label"), tab_context(s(t, "tab_id"), panes, home), s(t, "agent_status")
                ),
                focus: ("tab.focus", "tab_id", s(t, "tab_id").into()),
                focused: t["focused"] == true,
            }
        })
        .collect();

    // Where you already are goes last, so the default selection is somewhere else.
    ws_rows.sort_by_key(|r| r.focused);
    tab_rows.sort_by_key(|r| r.focused);
    (ws_rows, tab_rows)
}

/// `@` prefix means tab mode; returns (tab_mode, query).
fn parse_input(input: &str) -> (bool, &str) {
    match input.strip_prefix('@') {
        Some(q) => (true, q),
        None => (false, input),
    }
}

/// Indices into `rows` sorted by score, plus matched char positions for highlighting.
fn filter(rows: &[Row], query: &str, matcher: &mut Matcher) -> Vec<(usize, Vec<u32>)> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut hits: Vec<(u32, usize, Vec<u32>)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let mut idx = Vec::new();
            let score = pattern.indices(Utf32Str::new(&r.text, &mut buf), matcher, &mut idx)?;
            idx.sort_unstable();
            idx.dedup();
            Some((score, i, idx))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1))); // stable on ties keeps "focused last"
    hits.into_iter().map(|(_, i, idx)| (i, idx)).collect()
}

const ACCENT: Color = Color::Cyan;

/// Name part (before the first double space) in normal text, context dimmed, matched chars in accent.
fn highlight(text: &str, idx: &[u32]) -> Line<'static> {
    let name_len = text.find("  ").map_or(usize::MAX, |b| text[..b].chars().count());
    let hl = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
    Line::from(
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                let st = if idx.binary_search(&(i as u32)).is_ok() {
                    hl
                } else if i < name_len {
                    Style::new()
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                Span::styled(c.to_string(), st)
            })
            .collect::<Vec<_>>(),
    )
}

fn main() {
    let load = || -> Result<_, String> {
        Ok((rpc("workspace.list", json!({}))?, rpc("tab.list", json!({}))?, rpc("pane.list", json!({}))?))
    };
    let (w, t, p) = match load() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("herdr-ws: {e}");
            std::process::exit(1);
        }
    };
    let (ws_rows, tab_rows) = build_rows(&w, &t, &p, &std::env::var("HOME").unwrap_or_default());

    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut input = String::new();
    let mut state = ListState::default().with_selected(Some(0));
    let mut term = ratatui::init();

    let chosen = loop {
        let (tab_mode, query) = parse_input(&input);
        let rows = if tab_mode { &tab_rows } else { &ws_rows };
        let hits = filter(rows, query, &mut matcher);
        if state.selected().is_some_and(|s| s >= hits.len()) {
            state.select(Some(hits.len().saturating_sub(1)));
        }

        let _ = term.draw(|f| {
            let area = f.area(); // herdr sizes the popup (herdr-plugin.toml), the box fills it

            let dim = Style::new().fg(Color::DarkGray);
            let tab = |name: &'static str, on: bool| {
                if on { Span::styled(format!(" {name} "), Style::new().fg(Color::Black).bg(ACCENT).bold()) }
                else { Span::styled(format!(" {name} "), dim) }
            };
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Line::from(vec![Span::raw(" "), tab("Workspaces", !tab_mode), tab("@Tabs", tab_mode), Span::raw(" ")]))
                .title(Line::from(Span::styled(format!(" {}/{} ", hits.len(), rows.len()), dim)).right_aligned())
                .title_bottom(Line::from(Span::styled(" ↵ go · tab/@ switch · esc close ", dim)).centered());
            let inner = block.inner(area);
            f.render_widget(Clear, area);
            f.render_widget(block, area);

            let [prompt, sep, list] =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" › ", Style::new().fg(ACCENT)), Span::raw(query)])), prompt);
            f.set_cursor_position((prompt.x + 3 + query.chars().count() as u16, prompt.y));
            f.render_widget(Paragraph::new("─".repeat(sep.width as usize)).style(dim), sep);

            let items: Vec<Line> = hits.iter().map(|(i, idx)| highlight(&rows[*i].text, idx)).collect();
            let list_w = List::new(items)
                .highlight_style(Style::new().bg(Color::Indexed(237)))
                .highlight_symbol(Span::styled(" ▍", Style::new().fg(ACCENT)))
                .highlight_spacing(HighlightSpacing::Always);
            f.render_stateful_widget(list_w, list, &mut state);
        });

        let Ok(Event::Key(k)) = event::read() else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => break None,
            KeyCode::Char('c') if ctrl => break None,
            KeyCode::Enter => break state.selected().and_then(|s| hits.get(s)).map(|(i, _)| rows[*i].focus.clone()),
            KeyCode::Up => state.select_previous(),
            KeyCode::Char('p') if ctrl => state.select_previous(),
            KeyCode::Down => state.select_next(),
            KeyCode::Char('n') if ctrl => state.select_next(),
            KeyCode::Tab => {
                if tab_mode { input.remove(0); } else { input.insert(0, '@'); }
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
        }
    };
    ratatui::restore();

    if let Some((method, param, id)) = chosen {
        if let Err(e) = rpc(method, json!({ param: id })) {
            eprintln!("herdr-ws: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_labels_are_distinguishable() {
        let ws = json!({"workspaces": [
            {"workspace_id":"w1","number":1,"label":"agent-rust","active_tab_id":"w1:t1","tab_count":2,"agent_status":"idle","focused":false},
            {"workspace_id":"w3","number":3,"label":"agent-rust","active_tab_id":"w3:t1","tab_count":1,"agent_status":"unknown","focused":true}]});
        let tabs = json!({"tabs": [
            {"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"1","agent_status":"idle","focused":false},
            {"tab_id":"w3:t1","workspace_id":"w3","number":1,"label":"1","agent_status":"unknown","focused":true}]});
        let panes = json!({"panes": [
            {"tab_id":"w1:t1","cwd":"/h/a","foreground_cwd":"/h/a/lesson","agent":"claude","focused":false},
            {"tab_id":"w3:t1","cwd":"/h/b","focused":false}]});
        let (w, t) = build_rows(&ws, &tabs, &panes, "/h");
        assert_eq!(w[0].text, "1 agent-rust  ~/a/lesson  [claude]  · 2 tabs · idle");
        assert_eq!(w[1].text, "3 agent-rust  ~/b  · 1 tabs · unknown"); // focused sorts last
        assert_eq!(t[0].text, "agent-rust#1 › 1 1  ~/a/lesson  [claude]  · idle");
        assert_eq!(t[1].text, "agent-rust#3 › 1 1  ~/b  · unknown");
        assert_eq!(t[0].focus, ("tab.focus", "tab_id", "w1:t1".to_string()));

        let mut m = Matcher::new(Config::DEFAULT);
        let hits = filter(&t, "claude", &mut m);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 0);
    }

    #[test]
    fn at_prefix_selects_tab_mode() {
        assert_eq!(parse_input("@foo"), (true, "foo"));
        assert_eq!(parse_input("foo"), (false, "foo"));
        assert_eq!(parse_input("@"), (true, ""));
    }
}
