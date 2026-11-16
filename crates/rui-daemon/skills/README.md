# Skills

Each folder holds one skill in the agentskills.io format: a `SKILL.md`, plus optional `references/*.md` and `scripts/*`.
Before each turn the daemon copies a member's attached skills into its worktree's `.claude/skills/`, and the CLI loads each one by its description.

`SKILL.md` starts with `---` frontmatter, one field per line (the daemon parses it line by line):
`name` (matches the folder), `description` (third person, "Use when…"), `license` (MIT, Apache-2.0 or Proprietary-RUI),
`metadata: {source: "<url>", adapted: true|false}`, `bash` (a one-line JSON array of allowed command patterns) and an optional `run_hint`.

Bodies stay under about 400 lines; longer detail goes in `references/`. Skills adapted from public sources are credited in `THIRD_PARTY.md`.
Never copy from CC BY-SA or MPL sources.
