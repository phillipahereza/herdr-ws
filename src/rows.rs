//! Turns a herdr `session.snapshot` into picker rows for each mode.
use serde_json::Value;

use crate::history;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Ws,
    Tab,
    Agent,
    Dir,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Ws, Mode::Tab, Mode::Agent, Mode::Dir];

    pub fn prefix(self) -> &'static str {
        match self {
            Mode::Ws => "",
            Mode::Tab => "@",
            Mode::Agent => "!",
            Mode::Dir => "/",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Mode::Ws => "Workspaces",
            Mode::Tab => "@Tabs",
            Mode::Agent => "!Agents",
            Mode::Dir => "/Dirs",
        }
    }

    pub fn next(self) -> Mode {
        Mode::ALL[(Mode::ALL.iter().position(|m| *m == self).unwrap() + 1) % Mode::ALL.len()]
    }
}

/// Prefix character picks the mode; returns (mode, query).
pub fn parse_input(input: &str) -> (Mode, &str) {
    for m in [Mode::Tab, Mode::Agent, Mode::Dir] {
        if let Some(q) = input.strip_prefix(m.prefix()) {
            return (m, q);
        }
    }
    (Mode::Ws, input)
}

#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    Ws(String),
    Tab(String),
    Pane { tab: String, pane: String },
    Dir { path: String, ws: Option<String> },
}

pub struct Row {
    /// What fuzzy matching runs on: name, two spaces, then dimmed context.
    pub text: String,
    pub name_len: usize,
    /// Raw label, prefilled when renaming.
    pub label: String,
    pub status: String,
    pub target: Target,
    /// Pane shown in the preview.
    pub pane: Option<String>,
    /// Sorts before fuzzy score (agents: blocked first).
    pub group: u8,
}

impl Row {
    fn new(name: String, ctx: String, label: &str, status: &str, target: Target, pane: Option<&str>) -> Row {
        Row {
            name_len: name.chars().count(),
            text: if ctx.is_empty() { name } else { format!("{name}  {ctx}") },
            label: label.into(),
            status: status.into(),
            target,
            pane: pane.map(String::from),
            group: 0,
        }
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map_or(&[], Vec::as_slice)
}

pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn pane_cwd(p: &Value) -> &str {
    if s(p, "foreground_cwd").is_empty() { s(p, "cwd") } else { s(p, "foreground_cwd") }
}

/// The tab's focused pane (from its layout), else its first pane.
fn tab_pane<'a>(snap: &'a Value, tab_id: &str) -> Option<&'a Value> {
    let panes = arr(snap, "panes");
    let focused = arr(snap, "layouts").iter().find(|l| s(l, "tab_id") == tab_id).map(|l| s(l, "focused_pane_id"));
    focused
        .and_then(|id| panes.iter().find(|p| s(p, "pane_id") == id))
        .or_else(|| panes.iter().find(|p| s(p, "tab_id") == tab_id))
}

/// Context that tells same-named tabs apart: the focused pane's cwd plus any agents in the tab.
fn tab_context(snap: &Value, tab_id: &str, home: &str) -> String {
    let cwd = tab_pane(snap, tab_id).map_or(String::new(), |p| tilde(pane_cwd(p), home));
    let mut agents: Vec<&str> =
        arr(snap, "panes").iter().filter(|p| s(p, "tab_id") == tab_id).map(|p| s(p, "agent")).filter(|a| !a.is_empty()).collect();
    agents.dedup();
    if agents.is_empty() { cwd } else { format!("{cwd}  [{}]", agents.join(",")) }
}

fn workspace<'a>(snap: &'a Value, id: &str) -> Option<&'a Value> {
    arr(snap, "workspaces").iter().find(|w| s(w, "workspace_id") == id)
}

fn tab_name(snap: &Value, t: &Value) -> String {
    let (wl, wn) = workspace(snap, s(t, "workspace_id")).map_or(("?", Value::Null), |w| (s(w, "label"), w["number"].clone()));
    format!("{wl}#{wn} › {} {}", t["number"], s(t, "label"))
}

/// Most recently visited first, where you are now last (so Enter on an empty query toggles back).
fn sort_recent(rows: &mut [Row], history: &[String], current: &str, ws_level: bool) {
    let key = |r: &Row| match &r.target {
        Target::Ws(id) | Target::Tab(id) => (id == current, std::cmp::Reverse(history::rank(history, id, ws_level))),
        _ => (false, std::cmp::Reverse(0)),
    };
    rows.sort_by_key(key);
}

pub fn build(mode: Mode, snap: &Value, history: &[String], dirs: &[String], home: &str) -> Vec<Row> {
    match mode {
        Mode::Ws => {
            let mut rows: Vec<Row> = arr(snap, "workspaces")
                .iter()
                .map(|w| {
                    let tab = s(w, "active_tab_id");
                    Row::new(
                        format!("{} {}", w["number"], s(w, "label")),
                        format!("{}  {} tabs", tab_context(snap, tab, home), w["tab_count"]),
                        s(w, "label"),
                        s(w, "agent_status"),
                        Target::Ws(s(w, "workspace_id").into()),
                        tab_pane(snap, tab).map(|p| s(p, "pane_id")),
                    )
                })
                .collect();
            sort_recent(&mut rows, history, s(snap, "focused_workspace_id"), true);
            rows
        }
        Mode::Tab => {
            let mut rows: Vec<Row> = arr(snap, "tabs")
                .iter()
                .map(|t| {
                    let id = s(t, "tab_id");
                    Row::new(
                        tab_name(snap, t),
                        tab_context(snap, id, home),
                        s(t, "label"),
                        s(t, "agent_status"),
                        Target::Tab(id.into()),
                        tab_pane(snap, id).map(|p| s(p, "pane_id")),
                    )
                })
                .collect();
            sort_recent(&mut rows, history, s(snap, "focused_tab_id"), false);
            rows
        }
        Mode::Agent => arr(snap, "panes")
            .iter()
            .filter(|p| !s(p, "agent").is_empty())
            .map(|p| {
                let tab = arr(snap, "tabs").iter().find(|t| s(t, "tab_id") == s(p, "tab_id"));
                let title = s(p, "terminal_title_stripped");
                let mut r = Row::new(
                    format!("{} {}", s(p, "agent"), title).trim_end().into(),
                    format!("{}  {}", tab.map_or(String::new(), |t| tab_name(snap, t)), tilde(pane_cwd(p), home)),
                    title,
                    s(p, "agent_status"),
                    Target::Pane { tab: s(p, "tab_id").into(), pane: s(p, "pane_id").into() },
                    Some(s(p, "pane_id")),
                );
                r.group = status_priority(s(p, "agent_status"));
                r
            })
            .collect(),
        Mode::Dir => dirs
            .iter()
            .map(|d| {
                // A workspace already "at" this dir: one of its panes started there.
                let ws = arr(snap, "panes")
                    .iter()
                    .find(|p| s(p, "cwd") == d)
                    .and_then(|p| workspace(snap, s(p, "workspace_id")));
                let ctx = ws.map_or(String::new(), |w| format!("→ {} {}", w["number"], s(w, "label")));
                let pane = ws.and_then(|w| tab_pane(snap, s(w, "active_tab_id"))).map(|p| s(p, "pane_id"));
                Row::new(
                    tilde(d, home),
                    ctx,
                    "",
                    ws.map_or("", |w| s(w, "agent_status")),
                    Target::Dir { path: d.clone(), ws: ws.map(|w| s(w, "workspace_id").into()) },
                    pane,
                )
            })
            .collect(),
    }
}

/// Agents that need you first.
fn status_priority(status: &str) -> u8 {
    match status {
        "blocked" => 0,
        "done" => 1,
        "idle" => 2,
        "working" => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap() -> Value {
        json!({
            "focused_workspace_id": "w3", "focused_tab_id": "w3:t1",
            "workspaces": [
                {"workspace_id":"w1","number":1,"label":"agent-rust","active_tab_id":"w1:t1","tab_count":2,"agent_status":"idle"},
                {"workspace_id":"w2","number":2,"label":"web","active_tab_id":"w2:t1","tab_count":1,"agent_status":"blocked"},
                {"workspace_id":"w3","number":3,"label":"agent-rust","active_tab_id":"w3:t1","tab_count":1,"agent_status":"unknown"}],
            "tabs": [
                {"tab_id":"w1:t1","workspace_id":"w1","number":1,"label":"1","agent_status":"idle"},
                {"tab_id":"w2:t1","workspace_id":"w2","number":1,"label":"1","agent_status":"blocked"},
                {"tab_id":"w3:t1","workspace_id":"w3","number":1,"label":"1","agent_status":"unknown"}],
            "panes": [
                {"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"/h/a"},
                {"pane_id":"w1:p2","tab_id":"w1:t1","workspace_id":"w1","cwd":"/h/a","foreground_cwd":"/h/a/lesson","agent":"claude","agent_status":"working","terminal_title_stripped":"fix"},
                {"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"/h/web","agent":"pi","agent_status":"blocked"},
                {"pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3","cwd":"/h/b"}],
            "layouts": [{"tab_id":"w1:t1","focused_pane_id":"w1:p2"}]
        })
    }

    #[test]
    fn duplicate_labels_are_distinguishable_and_recent_first() {
        let hist: Vec<String> = ["w1", "w2:t1", "w3"].map(String::from).to_vec();
        let w = build(Mode::Ws, &snap(), &hist, &[], "/h");
        // w2 visited most recently (w3 is current, so it goes last).
        assert_eq!(w.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), [
            "2 web  ~/web  [pi]  1 tabs",
            "1 agent-rust  ~/a/lesson  [claude]  2 tabs", // focused pane from layout, not first pane
            "3 agent-rust  ~/b  1 tabs",
        ]);
        assert_eq!(w[1].pane.as_deref(), Some("w1:p2"));

        let t = build(Mode::Tab, &snap(), &hist, &[], "/h");
        assert_eq!(t[0].target, Target::Tab("w2:t1".into()));
        assert_eq!(t[0].text, "web#2 › 1 1  ~/web  [pi]");
        assert_eq!(t[2].target, Target::Tab("w3:t1".into()));
    }

    #[test]
    fn agents_need_you_first() {
        let a = build(Mode::Agent, &snap(), &[], &[], "/h");
        assert_eq!(a.len(), 2);
        let blocked = a.iter().find(|r| r.status == "blocked").unwrap();
        let working = a.iter().find(|r| r.status == "working").unwrap();
        assert!(blocked.group < working.group);
        assert_eq!(working.text, "claude fix  agent-rust#1 › 1 1  ~/a/lesson");
        assert_eq!(working.target, Target::Pane { tab: "w1:t1".into(), pane: "w1:p2".into() });
    }

    #[test]
    fn dirs_map_to_existing_workspaces() {
        let d = build(Mode::Dir, &snap(), &[], &["/h/web".into(), "/h/new".into()], "/h");
        assert_eq!(d[0].text, "~/web  → 2 web");
        assert_eq!(d[0].target, Target::Dir { path: "/h/web".into(), ws: Some("w2".into()) });
        assert_eq!(d[1].text, "~/new");
        assert_eq!(d[1].target, Target::Dir { path: "/h/new".into(), ws: None });
    }

    #[test]
    fn prefixes_pick_modes() {
        assert_eq!(parse_input("foo"), (Mode::Ws, "foo"));
        assert_eq!(parse_input("@foo"), (Mode::Tab, "foo"));
        assert_eq!(parse_input("!cl"), (Mode::Agent, "cl"));
        assert_eq!(parse_input("/"), (Mode::Dir, ""));
        assert_eq!(Mode::Dir.next(), Mode::Ws);
    }
}
