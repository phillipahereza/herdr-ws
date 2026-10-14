# fuzzy-jump

A [herdr](https://herdr.dev) plugin: a floating fuzzy finder for jumping between workspaces, tabs, agents and directories.

| Type | Searches | Enter |
|---|---|---|
| (nothing) | workspaces | focus it |
| `@` | tabs | focus it |
| `!` | agent panes, blocked ones first | focus that pane |
| `/` | [zoxide](https://github.com/ajeetdsouza/zoxide) directories | focus the workspace already there, or create one |

- Lists are most-recently-visited first, with where you are now last, so Enter on an empty query jumps back to the previous one.
- Rows show the number, cwd and agents with a status dot (red blocked, yellow working, green idle), so same-named workspaces and tabs are easy to tell apart. The bottom shows a live preview of the selected pane.
- When nothing matches, Enter creates a workspace (or, in `@` mode, a tab) named after your query.

| Key | Action |
|---|---|
| `Tab` | next mode |
| `↑/↓`, `ctrl+p/ctrl+n` | move |
| `ctrl+r` | rename the selected workspace or tab |
| `ctrl+x` | close the selected workspace or tab (asks first) |
| `Esc` | close |

## Install

Requires herdr 0.7.4+ and a Rust toolchain (`cargo`), since the plugin builds on install.

```sh
herdr plugin install phillipahereza/herdr-ws
```

Then bind a key in `~/.config/herdr/config.toml` and run `herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "fuzzy-jump.open"
description = "Fuzzy jump"
```

Update with `herdr plugin update fuzzy-jump`.

## Develop

```sh
cargo build --release && herdr plugin link .
```
