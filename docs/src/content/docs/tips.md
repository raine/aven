---
title: Tips
description: Configure terminal workflows and compatibility details.
---

## Use Ctrl-Enter in Alacritty and tmux

Terminal applications can distinguish `Ctrl-Enter` only when every layer reports the modifier. Alacritty supports Kitty CSI-u directly, while tmux uses its extended-key mode between the outer terminal and the application.

### Configure Alacritty

Add an explicit `Ctrl-Enter` binding to `~/.config/alacritty/alacritty.toml`:

```toml
[[keyboard.bindings]]
chars = "\u001B[13;5u"
key = "Return"
mods = "Control"
```

This binding emits the Kitty CSI-u representation of Control-modified Enter. Alacritty applies it immediately when `live_config_reload` is enabled. Otherwise, open a fresh Alacritty window.

The binding applies to every terminal application. Programs that do not understand CSI-u may display or ignore the escape sequence, so `Ctrl-s` remains Aven's portable composer shortcut.

### Configure tmux

Add these server options to `~/.tmux.conf`:

```text
set -s extended-keys on
set -s extended-keys-format csi-u
```

Apply them to the running tmux server:

```sh
tmux source-file ~/.tmux.conf
tmux set-option -s extended-keys on
tmux set-option -s extended-keys-format csi-u
```

The `csi-u` output format is required so Crossterm receives `Ctrl-Enter` as a modified Enter event. Confirm the active values:

```sh
tmux show-options -sv extended-keys
tmux show-options -sv extended-keys-format
```

Expected output:

```text
on
csi-u
```

### Verify in Aven

1. Run `aven tui` inside tmux.
2. Press `a` to open the composer.
3. Focus Description and type a line.
4. Press plain `Enter` and confirm it inserts a newline.
5. Press `Ctrl-Enter` and confirm the task is created.
6. Open another composer and confirm `Ctrl-s` also creates the task.

If `Ctrl-Enter` inserts a newline, open a fresh Alacritty window, reapply the tmux settings, and restart Aven. Use `Ctrl-s` when any terminal layer cannot preserve the modifier.
