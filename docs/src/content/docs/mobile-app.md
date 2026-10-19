---
title: Mobile app
description: Pair Aven for iPhone or Android with your sync, and use Ask Aven to manage tasks by voice.
---

Aven for [iPhone](https://apps.apple.com/app/aven-tasks/id6811171767) and
[Android](https://play.google.com/store/apps/details?id=fi.zendit.aven) keeps
its own copy of your tasks and syncs it with your other devices through your
sync server or [Aven Cloud](https://cloud.aventasks.dev/).

## Pair the app

A phone joins an existing sync; it can't start one. Set up sync on a computer
first, as in [Sync across devices](/sync/setup/). Then, on that computer, run
`aven sync invite`, or choose **Add device** in the TUI's Sync dialog, and scan
the QR code with the app. See [Add a device](/sync/setup/#add-a-device) for
details.

## Ask Aven

Ask Aven, on iPhone, manages tasks from a spoken or typed request, such as
"move the dentist thing to next Monday" or "add tasks for the trip". It needs a
self-hosted sync server with [AI set up](/server-configuration/#ai-for-ask-aven);
it isn't available on Aven Cloud yet.

Speak or type a request. Ask Aven reads the tasks it needs and proposes
changes. Nothing is saved until you review the proposed changes and apply them.
You can edit or leave out individual changes before applying, or discard them
all. After applying, you can undo the last applied set of changes.

The conversation stays in memory only. Closing it or quitting Aven clears it,
along with any changes you haven't applied.

### What is sent where

- **Audio stays on the phone.** Speech is transcribed on the iPhone. If
  on-device recognition isn't available, you type instead.
- **Your request and task context go to your server.** The phone sends the
  transcript, the conversation so far, and the task content Ask Aven reads to
  answer, such as titles, descriptions, notes, projects, and labels.
- **Your server forwards them to its AI provider.** The provider is the one
  configured on the server: a local model such as Ollama, or a cloud service.
  The provider's own retention policy applies.

Encrypted sync does not protect this text from the server or the provider. The
developer of Aven receives none of it. Before the first request, the app asks
for your consent, once per paired server. Settings › Ask Aven lists what's sent.

## Ask Aven status messages

Settings › Ask Aven shows whether your server is ready, with a message for each
state. When the server offers a setup hint, the app shows it as text you can
copy. After changing the server, restart it; the app checks again whenever you
return to it, or when you tap **Check again**.

| Status | Message | What to do |
| --- | --- | --- |
| Ready | Ask Aven is ready. | Nothing. |
| AI not set up | Set up AI on your server: | Run `aven server ai setup` on the server host, then restart the server. See [AI for Ask Aven](/server-configuration/#ai-for-ask-aven). |
| Provider login needed | Your server needs a provider login. | Follow the setup hint: log in with `aven server ai login`, or set the provider's API key in the server's environment, then restart the server. |
| Provider can't use tools | Your server's AI provider can't use tools. Choose another provider. | Pick a provider with `tools=true` in `aven server ai providers`. |
| Version mismatch | Update Aven on your server or on this iPhone. | Update whichever is older. |
| Not authorized | This iPhone isn't authorized on the server. | The server refused the phone's credential (HTTP 401). Check sync status in Settings › Diagnostics; the phone may have been removed from sync. |
| No AI on server | The server doesn't offer AI. Update it, or check your proxy. | The server returned HTTP 404: update it, or make the reverse proxy forward `/ai/` paths. |
| Temporarily unavailable | Your server's AI provider is temporarily unavailable. | Check the provider settings and credentials, and the server log. |
| HTTP *status* | Your server answered with an error. | Check the server log and any reverse proxy. |
| Unreadable answer | Your server sent an answer Aven couldn't read. | Check that the reverse proxy passes `/ai/` responses through unchanged. |
| Offline, Timed out, Server not found, Can't connect, Secure connection failed, Unreachable | The phone couldn't reach the server. | Check the network, the server address, and its TLS certificate. Tap **Try again**. |

To see the same states from a computer paired with the server, run
`aven ai status`. See [Troubleshooting](/server-configuration/#troubleshooting)
for server-side causes.
