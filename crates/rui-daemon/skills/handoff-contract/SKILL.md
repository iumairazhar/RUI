---
name: handoff-contract
description: Use whenever work changes hands inside a pod, either a Lead writing a job for a member or a member writing a status note back. Defines the shared job format (objective, output, done criteria, done command, files and boundaries, budget) and the status note format (code, evidence, assumptions, concerns). Use it for every job and every report_status, even small ones, because vague handoffs are the main cause of duplicate and wasted work.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: []
---
# Handoff Contract

Every handoff in a pod uses one of two shapes: a **job** (Lead → member) or a **status note** (member → Lead).

The member starts with none of the Lead's context. The job is the only thing it reads before it works.

The Lead sees only the status note, not the member's session. The note is the only evidence the work is done.

So both must stand on their own.

## Why this matters

When a delegated task is described in a few words, agents misread it. Two members research the same thing, or one builds the wrong thing well.

What prevents that is the same every time:

- a clear objective,
- a defined output,
- guidance on which tools and sources to use,
- and firm boundaries on what is and is not this job.

Effort should match the job. A one-file fix gets a short job and a small budget. A new module gets a longer job and more room. Do not give a simple lookup the same budget as a feature.

## The job (Lead → member)

```
JOB: <short title>
OBJECTIVE: <one or two sentences: the outcome, not the steps>
WHY: <how this fits the owner's goal, so the member can make good small calls>
OUTPUT: <exact deliverable: files, endpoints, a report section, its format>
DONE WHEN:
  - <testable criterion>
  - <testable criterion>
DONE COMMAND: <the command whose clean output proves it, e.g. uv run pytest -q>
IN SCOPE: <files or folders this job may create or edit>
DO NOT TOUCH: <files, folders, checks or other members' areas>
TOOLS / SOURCES: <skills to use, docs to read, commands to prefer>
BUDGET: <rough size: small (≈5 tool calls) | medium (≈15) | large (≈30)>
SIBLINGS: <other jobs running now and what they own, to avoid overlap>
ASK ME IF: <the specific decisions the Lead wants to make itself>
```

Rules for writing jobs:

- **Outcome over steps.** Say what must be true at the end. Give steps only when the order matters.
- **Done criteria must be checkable.** "Works well" is not checkable. "`GET /notes` returns 200 with a JSON list; test `test_list_notes` passes" is.
- **Always name a done command.** If no command can prove it, say how the evaluator will check it (open URL X, expect Y).
- **Boundaries both ways.** List what the member may touch and what it must not. Name the checks it must not edit: no agent edits the tests that judge its own output.
- **List siblings.** "Frontend is building the list page against this API at the same time" prevents two members inventing two API shapes.
- **One job, one owner.** If two members share a file, split the file or sequence the jobs.
- **Scale the budget.** Simple lookups: small. A feature with tests: medium. A new subsystem: large, or split it.
- **Name the skills.** "Use `test-driven-development` and `fastapi`" is more useful than "write good code".

## The status note (member → Lead)

Send it with `report_status`. Use exactly one code.

| Code | Meaning |
|---|---|
| `DONE` | All done criteria met and the done command passed. |
| `DONE_WITH_CONCERNS` | Done criteria met, but something worries you (a risk, a shortcut, a doubt). |
| `NEEDS_CONTEXT` | You cannot finish without information you could not find. Say exactly what. |
| `BLOCKED` | You cannot proceed at all (tool failure, conflict, a hard stop). Say what you tried. |

```
CODE: <DONE | DONE_WITH_CONCERNS | NEEDS_CONTEXT | BLOCKED>
SUMMARY: <one or two sentences: what now exists or what stopped you>
EVIDENCE: <the done command and the key lines of its output, e.g. "uv run pytest -q → 14 passed">
FILES CHANGED: <paths, one per line or comma separated>
ASSUMPTIONS: <each call you made without asking, and why it was safe>
CONCERNS: <risks, shortcuts, things the evaluator should look at; "none" if none>
NEXT: <what the Lead or the next job should do; for NEEDS_CONTEXT/BLOCKED, the exact question>
```

Rules for status notes:

- **Evidence or it did not happen.** Paste the real command and the real result line. Never write "tests should pass".
- **Report honestly.** `DONE_WITH_CONCERNS` is not failure. Hiding a concern behind `DONE` is.
- **Assumptions are not concerns.** An assumption is a decision you made. A concern is a risk you see. List both.
- **Stay short.** The Lead reads many notes. Ten lines beats a page.
- **Partial work is fine to report.** If you hit `BLOCKED` halfway, list what is done and what is not.

## Example 1: backend job and its note

```
JOB: Notes API
OBJECTIVE: A FastAPI service that can create, list and delete notes.
WHY: The owner wants a small journal app; the frontend member builds the UI against this API.
OUTPUT: app/main.py, app/routers/notes.py, tests/test_notes.py
DONE WHEN:
  - POST /notes with {"text": "..."} returns 201 and the note with id and created_at
  - GET /notes returns notes newest first
  - DELETE /notes/{id} returns 204, or 404 with a clear detail when missing
  - Empty text returns 422
DONE COMMAND: uv run pytest -q
IN SCOPE: app/, tests/, pyproject.toml
DO NOT TOUCH: web/ (Frontend owns it), tests/acceptance/ (Tester owns it)
TOOLS / SOURCES: skills fastapi, test-driven-development
BUDGET: medium
SIBLINGS: Frontend builds web/ against these routes; Tester writes tests/acceptance/.
ASK ME IF: you want a database other than SQLite, or a field not listed here.
```

```
CODE: DONE_WITH_CONCERNS
SUMMARY: Notes API with create, list, delete; 9 tests.
EVIDENCE: uv run pytest -q → 9 passed in 0.41s
FILES CHANGED: app/main.py, app/routers/notes.py, app/models.py, tests/test_notes.py, pyproject.toml
ASSUMPTIONS: Max note length 10,000 chars (422 above it); reversible, one constant.
CONCERNS: SQLite file is created in the repo root; Devops may want it under data/.
NEXT: Frontend can call GET/POST/DELETE /notes now.
```

## Example 2: research job blocked on context

```
JOB: Pick a charting library
OBJECTIVE: Recommend one charting library for the dashboard page, with reasons.
WHY: Frontend will build three charts next; the choice is hard to change later.
OUTPUT: A short report: recommendation, two runners-up, a table of bundle size, license, React 19 support.
DONE WHEN:
  - Each claim cites a source URL
  - License of each option is stated
DONE COMMAND: none; the Lead reviews the report.
IN SCOPE: notes only, no code changes
DO NOT TOUCH: any source file
TOOLS / SOURCES: skill research; official docs and npm pages over blog posts
BUDGET: small
SIBLINGS: none
ASK ME IF: every option fails one criterion.
```

```
CODE: NEEDS_CONTEXT
SUMMARY: Two good options found, but the choice depends on whether charts must work offline.
EVIDENCE: Compared 4 libraries from official docs (URLs in report draft).
FILES CHANGED: none
ASSUMPTIONS: React 19 is required (from package.json).
CONCERNS: One option is MIT but its maps add-on is commercial.
NEXT: Question: must the dashboard render with no network? If yes, pick A; if no, B is lighter.
```
