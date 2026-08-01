**English** | [日本語](README.ja.md)

# ccsessions

Shows your running Claude Code sessions as a flock of creatures in the macOS menu bar.
One session = one creature. Color and motion tell you the state, and hovering shows the details.

![The six states: working, needs you, agents running, idle, done, error](docs/assets/states.svg)

## Install

macOS only. No special permissions (screen recording and the like) are required.

```sh
brew install S-Nakamur-a/tap/ccsessions
brew services start ccsessions
```

Next, inside Claude Code, add the hooks that report session state:

```
/plugin marketplace add S-Nakamur-a/ccsessions
/plugin install ccsessions@ccsessions-marketplace
```

Restart Claude Code and the creatures appear. If something is not working, run
`ccsessions doctor` — it tells you what is installed and what is missing.

## Settings

```sh
ccsessions ui
```

A browser opens. Language, menu bar or bottom of the screen, how the creatures look, how
long they stay — you decide all of that there. Building your own face happens here too.

The settings, the character builder, and the hover card come in English and Japanese, and
follow your OS language by default. Diagnostics (`ccsessions doctor`) are always English.

<details>
<summary>Editing the config file directly</summary>

The file is `~/.config/ccsessions/config.toml`. A running daemon picks up your edits within
a few hundred ms.

```toml
language = "auto"        # "auto" (follow the OS) | "ja" | "en"
                         # Applies to the settings, the builder, and the hover
                         # card. Diagnostics (`ccsessions doctor`) stay English.
placement = "bar"        # "bar" (menu bar) | "dock" (bottom of the screen)
design = "egg"           # built-ins are "egg" | "round" | "squircle" | "bean"
                         # the id of a face you made yourself works too
reduce_motion = false
show_glyphs = true       # show the state glyphs (› ! ⋯ z ✓ ×)
bar_align = "auto"       # "auto" | "center" | "left-of-notch" | "right-of-notch"
compact_flock = "auto"   # shrink the flock once sessions no longer fit
                         # "auto" (default) | "always" | "never"
done_ttl_secs = 180      # how long until done turns into idle
session_ttl_secs = 28800 # remove a creature after this long without an update
                         # (a safety net; see below)
max_sessions = 12
detect_errors = false    # on Stop, also read the transcript to detect an error exit
                         # (best-effort)

# Sessions to keep out of the list (see below)
ignore = ["~/work/tmp", "**/cron-jobs/**"]
```

`bar` draws on the menu bar of the screen that has keyboard focus (it follows you onto
external monitors). To hand-write a face in TOML, see [`faces/README.md`](faces/README.md).

</details>

<details>
<summary>Keeping some sessions out of the list (<code>ignore</code>)</summary>

Some sessions are running but do not need a creature — a directory you use for
scheduled jobs, for instance. Rules are matched against the session's working
directory. Write as many as you like; **a session is hidden as soon as one of them
matches.**

```toml
ignore = [
  "~/work/tmp",        # this directory and everything below it
  "**/worktrees/**",   # a glob: worktrees at any depth
]
```

| Rule | What it matches |
|---|---|
| `/Users/me/work/tmp` · `~/work/tmp` | With no wildcard: that directory and **everything below it**. It breaks on separators, so `/a/foo` does not match `/a/foobar` |
| `~/work/tmp/**` · `**/cron-jobs/**` | A glob. `*` and `?` stay within one path segment; `**` crosses them. A trailing `/**` also matches zero segments, so the directory itself is hidden too |

The dialect is the usual one (the same as gitignore or `rg --glob`), so **a glob is
matched exactly as written** — `~/work/tmp/*` covers only one level; write
`~/work/tmp/**` if you want the whole subtree.

Rules are anchored at the root. A bare `cron-jobs` is refused, because it is matched
against an absolute path and could never hit. Write `**/cron-jobs/**` to match at any
depth.

Only the **display** is affected. The session file stays where it is and the state
does not change.

```sh
ccsessions list          # applies ignore; prints "N hidden" at the end
ccsessions list --all    # every session, ignoring the rules
ccsessions doctor        # tells you how many are currently hidden
```

A rule you typed wrong is dropped on its own, with a warning — the rest of your
settings are never reset because of it. You can also edit the list in
`ccsessions ui`.

</details>

## States

| Display | State | When |
|---|---|---|
| `›` cyan, bobs up and down, blinks | Working | Claude is running after you sent a prompt |
| `!` amber, hops | Needs you | A permission request or notification is waiting for your input |
| `⋯` purple, drifts sideways, looks aside | Agents running | Subagents (Task) are running |
| `z` gray, still, faded | Idle | Some time has passed since the turn finished |
| `✓` green, still | Done | The turn just finished (3 minutes by default) |
| `×` red, blinks slowly | Error | The last turn ended in an error |

The badge is the number of agents that session is running.

## Updating

Two halves: the binaries come from brew, the hooks come from the plugin.

```sh
brew update && brew upgrade ccsessions
```

brew restarts the daemon for you. Then, inside Claude Code — it applies after a restart:

```
/plugin update ccsessions@ccsessions-marketplace
```

<details>
<summary>If <code>brew upgrade</code> says nothing changed</summary>

`brew update` copies the formula from the tap, and the tap is pushed at the very end of a
release. Run it in the first minute after a release is announced and you get the state
from just before it, after which `brew upgrade` quietly finds nothing to do.

```sh
brew outdated ccsessions   # prints the name if brew has seen a newer version
brew list --versions ccsessions
```

If brew has not seen it yet, `brew update` again. This is a source formula
([ADR 0021](docs/adr/0021-distribution.md)), so the upgrade itself builds from source and
takes a few minutes.
</details>

## Stopping and uninstalling

| What you want | Command |
|---|---|
| Stop the daemon | `brew services stop ccsessions` |
| Remove the hooks | In Claude Code, `/plugin uninstall ccsessions@ccsessions-marketplace` |
| Remove everything | Both of the above, then `brew uninstall ccsessions` |

<details>
<summary>Environments where you cannot use the plugin</summary>

The events we subscribe to are listed in `plugins/ccsessions/hooks/hooks.json` (10 of them).
If you cannot install plugins — enterprise managed settings, for example — use that file as
a reference and write them into `settings.json` by hand. In that case the command is the
absolute path to `ccsessions hook`, not `${CLAUDE_PLUGIN_ROOT}/...`. Do not drop `timeout` —
without it Claude Code's own default applies (600 seconds for most events), and a stuck hook
stalls the turn for that long.

</details>

<details>
<summary>When a creature disappears</summary>

1. When the session ends normally (the `SessionEnd` hook).
2. When the session's process is gone — it was force-quit, the terminal was closed, a parent
   tool killed it, and so `SessionEnd` never arrived. The daemon checks whether the pid
   recorded by the hook is still alive.
3. When no hook has arrived for `session_ttl_secs` (the safety net for what 1 and 2 miss).

So raising `session_ttl_secs` does not let dead sessions linger. Whenever liveness cannot be
confirmed, we always err on the side of "alive". Anything that is removed is recorded in
`~/Library/Logs/ccsessions/ccsessionsd.log` in the form `reaped session ... — pid 12345 が居ない`.

</details>

<details>
<summary>Known limitations</summary>

| Symptom | Cause | Workaround |
|---|---|---|
| The state stays "working" after you interrupt a turn with ESC | An interrupt sends neither `Stop` nor `StopFailure` | Send the next prompt and it recovers |
| Errors (red `×`) almost never show up | API errors are caught via `StopFailure`, but other failures are invisible to hooks | — |
| The agent rows on the hover card have no role labels | The payload gives us no way to match `agent_id` against the Agent tool's `description` | — |
| The badge tops out at 32 | A deliberate limit — `MAX_AGENTS` in `event.rs` | — |
| `bar_align = "center"` hides the flock on notched Macs | The notch sits at the horizontal center of the screen, so a centered flock always ends up underneath it | Use the default `auto`, which moves the flock to the right of the notch, then to the left. The startup log and `ccsessions doctor` warn about this as well |
| In some environments the flock does not follow along when you add menu extras | The free width to the right of the notch is measured at runtime and followed (up to a 10 second delay). Where it cannot be measured — Macs without a notch, an auto-hiding menu bar, full screen — it falls back to an estimated 225pt | Set `bar_align` to `left-of-notch` or `center` |
| Past roughly 20 sessions the flock no longer fits in the bar | Shrinking the flock has a floor (0.55×), and past that the creatures stop being legible, so we give up | Lower `max_sessions`, or use `placement = "dock"` |
| Hooks placed in enterprise managed settings are not picked up by the diagnostics | Only the user-wide, project, and local settings files are scanned | If that is where yours live, feel free to ignore `doctor`'s "NOT installed" |
| For hooks installed via the plugin, all you learn is that they are enabled | Hooks shipped by a plugin do not appear under `hooks` in `settings.json`. All `doctor` can see is `enabledPlugins` | To check event by event, read `plugins/ccsessions/hooks/hooks.json` directly |

</details>

## CLI

```sh
ccsessions list [--json] [--all] # list the live sessions (--all ignores `ignore`)
ccsessions ui                   # web UI for settings + face building
ccsessions config get|set|path  # show/change settings (same validation as the UI)
ccsessions doctor               # diagnostics
ccsessions face list|render     # list faces, render an SVG preview
ccsessions hook                 # called by Claude Code's hooks (JSON on stdin)
```

## Development

```sh
make check    # fmt --check + clippy -D warnings + test (the pre-commit quality gate)
make dev      # stop the running daemon, build, then launch the dev binary (no install needed)
make demo     # check the look with dummy sessions in all 6 states (no real sessions needed)
make preview  # run main's code the way production runs it (release build), before a release
make stop     # stop whatever you started here and bring the brew daemon back
make help     # list the targets
```

You need the Rust toolchain from [rustup](https://rustup.rs/) (MSRV 1.89).

**The resident daemon is the brew one, and it is the only one.** There is no
`make install` / `make start`: a second way to run it resident is what made every
creature appear twice. To look at your own build, `make dev` (debug) or `make preview`
(release) runs it in the foreground of your session instead — both park the brew daemon
first (`launchctl bootout`, which keeps the plist), and `make stop` — or `make release` —
brings it back. Forget to, and it comes back at your next login anyway. Start one by hand
and you are on your own; `ccsessions doctor` detects the overlap.
See [ADR 0028](docs/adr/0028-preview-parks-the-brew-daemon.md).

Hooks go in via the plugin during development too — a checkout works as a marketplace as
is, so run `/plugin marketplace add .` and then
`/plugin install ccsessions@ccsessions-marketplace`.

When you edit the README, update both languages: this file and
[`README.ja.md`](README.ja.md).

- [`docs/how-it-works.md`](docs/how-it-works.md) — the path from hook to overlay, the events
  we subscribe to, and why we draw CALayers directly
- [`docs/invariants.md`](docs/invariants.md) — invariants that must not be broken
- [`docs/adr/`](docs/adr/README.md) — why the alternatives were not taken
- [`faces/README.md`](faces/README.md) — how to make a face (the creature's design). No Rust
  required

These documents are currently written in Japanese only.

## License

[MIT](LICENSE)
