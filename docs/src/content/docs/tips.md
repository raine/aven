---
title: Tips
description: Configure terminal input and troubleshoot image previews.
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

## Troubleshoot image previews

If task detail shows a text label instead of a picture, use the label and the checks below to find the cause. The attachment remains part of the task even when Aven cannot show a preview.

### The image is pending download or unavailable

Run sync when the label says the image is pending download:

```sh
aven sync
```

If the image remains unavailable, check whether this device has its file:

```sh
aven attachment list APP-7KQ9 --json
aven attachment get 7KQ9A1X4MV2P8D6R --json
```

`has_blob: false` means the image file is absent from this device. Sync from a server that has it, or restore a [backup that includes images](/backups/). JSON exports do not include image files.

### The terminal shows labels instead of previews

Inline previews work in iTerm2, Kitty, WezTerm, and Ghostty. Other terminals, including Sixel-only terminals, show labels instead. In task detail, use `Tab` to focus a locally available image and press `Enter` or `o` to open it in the operating system's default viewer. Clicking the image label performs the same external open. You can also save a regular copy with `aven attachment get --output`.

Check the preview mode in `~/.config/aven/config.yaml`:

```yaml
local:
  inline_images: auto
```

- `off` always shows labels.
- `auto` shows previews in supported terminals outside tmux and labels inside tmux.
- `on` also enables tmux passthrough for a supported outer terminal.

Inside tmux, try `on` only when the outer terminal and tmux support the same image protocol. Aven uses the iTerm2 inline-image protocol in iTerm2 and the Kitty graphics protocol in Kitty, WezTerm, and Ghostty. See [Image previews](/configuration/#image-previews) for the complete setting behavior.

For detection problems, check that the terminal exports its normal markers into the shell that launches Aven. Aven recognizes `TERM_PROGRAM`, `TERM`, `KITTY_WINDOW_ID`, `WEZTERM_PANE`, and `GHOSTTY_RESOURCES_DIR`.

### The image is available but the preview still fails

When `has_blob: true`, confirm that Aven can save the image:

```sh
aven attachment get <attachment-id> --output ./attachment-check.png
```

If that succeeds, restart Aven to retry preview generation. The stored image is unaffected, and missing cached previews regenerate when needed.

A text label remains visible above a successful preview. This is intentional and indicates that an attachment is present. See [Image attachments](/tui/#image-attachments) for preview controls.
