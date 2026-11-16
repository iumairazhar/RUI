# Roles

Each folder holds one `AGENT.md`: the role file for one pod member. The daemon uses it as that agent's system prompt.

The file starts with `---` frontmatter, one field per line:
`name` (matches the folder), `code` (two uppercase letters), `title`, `description` (one line, what it owns),
`tools` (a JSON array, a subset of `["Read","Search","Edit","Shell","Run tests","Fetch URL"]`),
`skills` (a JSON array of folder names under `../skills/`) and `max_loops`.

The body (30–80 lines) says the job, what the role owns, what it never does, its handoffs, how it reports, and when it uses `ask_lead`.
Read-only roles (Architect, Evaluator, Researcher) get no `Edit`. Every skill a role lists must exist as a folder in `../skills/`.
