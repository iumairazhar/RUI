---
name: reporting-status
description: Use whenever a member is about to call report_status, finishes a task, gets stuck, or is tempted to say "done" — and whenever a Lead receives a status note and must decide what to do with it. Never report without reading this first.
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/subagent-driven-development/implementer-prompt.md", adapted: true}
bash: ["git status*", "git diff*", "git log*"]
---
# Reporting Status

A status note is the only thing the Lead sees of your work. It must be honest, short, and backed by evidence. Never silently hand over work you're unsure about.

## The four codes

| Code | Use when | Not when |
|---|---|---|
| `DONE` | Everything in the task is built, the done command ran **last** and **passed**, and you have no doubts. | The done command was not run, failed, or was run before your last edit. |
| `DONE_WITH_CONCERNS` | The work is complete and the done command passes, but you doubt correctness, scope, or quality (a file grew too big, an edge case you're unsure of, a ruling that could be wrong). | You know something is broken. That is not done. |
| `NEEDS_CONTEXT` | You can't continue without information that wasn't provided: a missing spec detail, an interface another member hasn't defined, credentials you must not invent. | You could find it by reading the repo. Read first. |
| `BLOCKED` | You can't complete the task: the approach fails, it needs a decision above your level, you hit a hard stop, or three fixes have failed. | You're merely unsure. Rule, record the assumption, continue. |

**Iron rule:** never claim `DONE` or `DONE_WITH_CONCERNS` without running the done command in this session and reading its output. Agreement with yourself is not evidence. See verification-before-completion.

## Self-review before you report

Read your own diff with fresh eyes (`git diff` against the task's base) and ask:

**Completeness**
- Did I build everything the task asked for? Every done criterion?
- Edge cases from the plan's Review focus?

**Quality**
- Do names say what things do?
- Is it clean, following the codebase's existing patterns?

**Discipline**
- Did I avoid building what wasn't asked (YAGNI)?
- Did I stay inside the files I was given?
- Did I leave other people's tests and checks untouched?

**Testing**
- Do the tests check real behaviour, not mocks?
- Did I watch each new test fail before it passed?
- Is the test output clean (no stray warnings or noise)?

Fix what you find **before** reporting. Self-review findings you fixed don't need to be reported; ones you couldn't fix are concerns.

## What the note contains

Keep it under ~15 lines. Detail lives in the repo and the commits.

```
Status: DONE_WITH_CONCERNS
Task: Task 3 — POST /items
Changed: app/items.py (new), app/main.py (router), tests/test_items.py (new)
Commits: a1b2c3d feat: create items
Done command: uv run pytest -q → 14 passed in 0.42s
Assumptions:
- Ruling: names are trimmed before the empty check — spec silent — if wrong, "  " is accepted
Concerns:
- app/main.py is 410 lines; worth splitting in a later task
```

Rules for each field:
- **Changed:** every file, new or modified. No "and some others".
- **Done command:** the exact command and the key result line, copied, not paraphrased.
- **Assumptions:** every `Ruling:` you made, with its cost if wrong. Exhaustive.
- **Concerns:** only real doubts. Omit the field if there are none.

For `NEEDS_CONTEXT` and `BLOCKED`, replace "Done command" with:
- **Stuck on:** the exact problem, in one or two sentences.
- **Tried:** what you did and what each attempt showed.
- **Need:** the specific thing that would unblock you, as options if possible (A / B, with your recommendation).

## How a Lead handles each code

- **DONE** → Verify, don't trust: check the done command result is real and the files match the task. Then pass to review or the next dependent task.
- **DONE_WITH_CONCERNS** → Read the concerns first. Correctness or scope concerns: resolve them (rule, or send back) before review. Observations ("file is getting large"): note them and proceed.
- **NEEDS_CONTEXT** → Provide the missing context and re-dispatch. If you don't have it either and can't rule on it, pass it up to Rui with options.
- **BLOCKED** → Assess the blocker:
  1. Context problem → give more context, re-dispatch.
  2. Needs more reasoning → re-dispatch with a more capable setup or a narrower ask.
  3. Task too large → split it into smaller tasks.
  4. Plan is wrong → rule on the correction, record it, re-dispatch with the ruling included.
  5. Hard stop that only Rui or the owner can clear → escalate with options (escalation-ladder).

**Never** ignore an escalation or send the same task back unchanged. If a member said it's stuck, something has to change.

## Red flags

| Thought | Reality |
|---|---|
| "Tests passed earlier, so DONE" | Earlier is not last. Re-run after your final edit. |
| "I'll mention the concern later" | There is no later. Concerns go in this note. |
| "BLOCKED looks bad, I'll say DONE_WITH_CONCERNS" | Mislabelling a broken task wastes the whole pod's time. Use the true code. |
| "NEEDS_CONTEXT" without saying what context | The Lead can't act on that. Name the exact missing fact. |
| A 60-line report | The Lead needs a decision, not a diary. Under ~15 lines. |

---
Adapted from obra/superpowers skills/subagent-driven-development (implementer-prompt.md and references/handling-reports.md) (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI: report_status tool, Lead-side handling, assumption ledger in the note.
