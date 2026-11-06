# AGENTS.md

Guidance for AI agents working on this repository.

## Layout

```
crates/chezmate-core   chezmoi client: runs the real chezmoi, parses output. No GUI deps.
crates/chezmate-cli    `chezmate` binary. Thin: argument parsing + output rendering.
crates/chezmate-app    GPUI Kit desktop app. Depends on core only.
docs/STATUS.md         Implementation status. Update it when a phase changes.
```

Dependencies point one way: `app` → `core`, `cli` → `core`. Never put chezmoi
logic in the app or CLI; add it to core with a test.

## Rules

- **Run the real chezmoi.** No mocks, fixtures pretending to be chezmoi output,
  or stub implementations reported as done. If something cannot work (platform
  limit, missing tool), keep what works and document the limit in
  `docs/STATUS.md` and README.
- **Never touch the developer's real dotfiles in tests.** Use the sandbox in
  `crates/chezmate-core/tests/common/mod.rs` (own HOME, source, config,
  persistent state, cache). Manual checks against the real setup must be
  read-only (`info`, `status`, `managed`, `diff`).
- **Always pass absolute targets to chezmoi.** chezmoi resolves relative
  arguments against the process working directory. `Chezmoi::target_args`
  does this; route every target through it.
- **chezmoi never prompts.** Core passes `--no-tty`; commands that would prompt
  either take `--force` after the caller confirmed (forget) or fail with a
  typed error (`Error::Conflict` for apply).
- **Don't leak secrets.** Don't call `chezmoi data` (it returns user template
  data); `Chezmoi::paths` asks for exactly the paths via `execute-template`.
- Keep error `kind()` strings and CLI JSON field names stable: agents rely on
  them. Add new ones rather than renaming.

## GPUI Kit

The app uses `gpui-kit` 0.9 only (it pins GPUI). Before writing UI code read
the upstream skill and guides (`skills/gpui-kit` in
https://github.com/longbridge/gpui-kit): Root at the window top, retained state
in `Entity<T>`, no async work or `cx.notify()` inside `render`, stable
`ElementId`s from domain IDs, one scroll owner per region. Prefer an existing
GPUI Kit component over a custom `div`; when none exists, build a small one
on GPUI rather than adding another GUI framework.

## Commands

```sh
export PATH="$HOME/.cargo/bin:$PATH"   # if cargo is not on PATH
cargo test                              # all tests; requires chezmoi installed
cargo clippy --all-targets
cargo fmt --all
cargo run -p chezmate-app
```

The first GPUI build takes several minutes. Commit after a phase is verified.
