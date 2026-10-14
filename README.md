# fuzzy-jump

A [herdr](https://herdr.dev) plugin: a floating fuzzy finder for jumping between workspaces and tabs.

- Type to filter workspaces. Start with `@` (or press `Tab`) to search tabs instead.
- `Enter` jumps, `Esc` closes, `↑/↓` or `ctrl+p/ctrl+n` move.
- Rows show the number, cwd and running agents, so same-named workspaces and tabs are easy to tell apart.

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
