//! herdr socket API: newline-delimited JSON over a Unix socket.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;

use serde_json::{Value, json};

fn socket_path() -> String {
    std::env::var("HERDR_SOCKET_PATH").unwrap_or_else(|_| {
        format!("{}/.config/herdr/herdr.sock", std::env::var("HOME").unwrap_or_default())
    })
}

pub fn rpc(method: &str, params: Value) -> Result<Value, String> {
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

/// Everything the picker shows, in one call: workspaces, tabs, panes, layouts.
pub fn snapshot() -> Result<Value, String> {
    Ok(rpc("session.snapshot", json!({}))?["snapshot"].take())
}

/// Bottom `n` non-blank lines of a pane's recent output.
pub fn read_tail(pane_id: &str, n: usize) -> Vec<String> {
    let Ok(v) = rpc("pane.read", json!({"pane_id": pane_id, "source": "recent", "lines": n * 2})) else {
        return vec![];
    };
    let text = v["read"]["text"].as_str().unwrap_or("");
    let lines: Vec<&str> = text.lines().collect();
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
    lines[..end].iter().skip(end.saturating_sub(n)).map(|l| l.trim_end().to_string()).collect()
}

/// Background thread: one `()` per herdr event that could change what the picker shows.
pub fn subscribe(tx: Sender<()>) {
    std::thread::spawn(move || {
        let kinds = [
            "workspace.created", "workspace.closed", "workspace.renamed", "workspace.updated",
            "tab.created", "tab.closed", "tab.renamed", "pane.created", "pane.closed",
            "pane.agent_status_changed", "pane.agent_detected",
        ];
        let subs: Vec<Value> = kinds.iter().map(|k| json!({"type": k})).collect();
        let Ok(mut s) = UnixStream::connect(socket_path()) else { return };
        let req = json!({"id": "sub", "method": "events.subscribe", "params": {"subscriptions": subs}});
        if writeln!(s, "{req}").is_err() {
            return;
        }
        // First line is the subscription ack; every later line is an event.
        for _ in BufReader::new(s).lines().skip(1).map_while(Result::ok) {
            if tx.send(()).is_err() {
                return;
            }
        }
    });
}
