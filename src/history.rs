//! Focus history, fed by the manifest's `[[events]]` hooks (`herdr-ws record`).
//! One id per line, oldest first: `w1` for a workspace focus, `w1:t2` for a tab focus.
use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;

const MAX: usize = 200;

fn path() -> Option<PathBuf> {
    std::env::var_os("HERDR_PLUGIN_STATE_DIR").map(|d| PathBuf::from(d).join("history"))
}

pub fn load() -> Vec<String> {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map_or(vec![], |s| s.lines().map(String::from).collect())
}

/// Called with HERDR_PLUGIN_EVENT_JSON; records the focused tab, else the focused workspace.
pub fn record(event_json: &str) -> std::io::Result<()> {
    let Some(p) = path() else { return Ok(()) };
    let v: Value = serde_json::from_str(event_json).unwrap_or_default();
    let Some(id) = find(&v, "tab_id").or_else(|| find(&v, "workspace_id")) else { return Ok(()) };
    std::fs::create_dir_all(p.parent().unwrap())?;
    // workspace.focused and tab.focused hooks run concurrently: append (atomic for a short line), never rewrite.
    std::fs::OpenOptions::new().create(true).append(true).open(&p)?.write_all(format!("{id}\n").as_bytes())?;
    let lines = load();
    if lines.len() > MAX * 2 {
        // shortcut: a rare concurrent append during this rewrite can be lost; harmless for a recency list.
        std::fs::write(p, trim(lines).join("\n") + "\n")?;
    }
    Ok(())
}

fn trim(mut lines: Vec<String>) -> Vec<String> {
    let extra = lines.len().saturating_sub(MAX);
    lines.drain(..extra);
    lines
}

/// The event payload shape isn't documented, so look for the key at any depth.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v {
        Value::Object(m) => m.get(key).and_then(Value::as_str).or_else(|| m.values().find_map(|x| find(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

/// Recency rank of `id` (higher = more recent; 0 = never). `ws_level` counts tab visits toward their workspace.
pub fn rank(history: &[String], id: &str, ws_level: bool) -> usize {
    history
        .iter()
        .rposition(|h| if ws_level { h.split(':').next() == Some(id) } else { h == id })
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_ids_at_any_depth() {
        let v = json!({"event": "tab.focused", "data": {"type": "tab_focused", "tab_id": "w1:t2", "workspace_id": "w1"}});
        assert_eq!(find(&v, "tab_id"), Some("w1:t2"));
        assert_eq!(find(&json!({"workspace_id": "w3"}), "tab_id"), None);
    }

    #[test]
    fn trims_to_max() {
        let lines: Vec<String> = (0..MAX + 5).map(|i| i.to_string()).collect();
        let t = trim(lines);
        assert_eq!(t.len(), MAX);
        assert_eq!(t[0], "5");
    }

    #[test]
    fn rank_prefers_latest() {
        let h: Vec<String> = ["w1", "w2:t1", "w1:t3", "w3"].map(String::from).to_vec();
        assert_eq!(rank(&h, "w1", true), 3);
        assert_eq!(rank(&h, "w2", true), 2);
        assert_eq!(rank(&h, "w9", true), 0);
        assert_eq!(rank(&h, "w1:t3", false), 3);
        assert_eq!(rank(&h, "w1", false), 1);
    }
}
