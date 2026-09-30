@AGENTS.md

# Claude Code specifics

- `AGENTS.md` above is the source of truth; this file only adds Claude-specific notes. Do not
  duplicate rules here.
- Use the `tdd` skill for parsers, `update()` logic, and layout code. Use `diagnosing-bugs` for
  anything flaky. Use the `pr` skill only if the owner asks for a PR.
- Prefer the dedicated Read/Edit/Grep tools over shell one-liners for code changes.
- You cannot see the TUI. Do not claim a visual result is good from code alone: use `TestBackend`
  snapshots, and list real-terminal checks as **[owner]** steps in your final message.
- Sub-agents are not needed for normal tasks; delegate only large read-only research (for example a
  spike from `TODO.md` phase 1).
- At the end of a task, state plainly what was verified, what was only reasoned about, and which
  **[owner]** steps remain.
