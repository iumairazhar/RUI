---
name: executing-plans
description: Use when a member has been handed a task or plan from its Lead and is about to implement it — follow it step by step, decide reversible ambiguities yourself, and stop only for the four hard stops. Use it even for a single small task.
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/executing-plans/SKILL.md", adapted: true}
bash: ["git status*", "git diff*", "git log*", "git add *", "git commit *"]
---
# Executing Plans

**Core principle:** The plan already did the thinking. Execute it exactly, prove each step with a test you watched fail and then pass, and leave a record that survives your own forgetting.

Narrate at most one short line between tool calls. The progress file and the tool output carry the record.

## Rulings, not stalls

Conflicts, ambiguities, small plan defects: **decide them yourself.** The spec is the binding authority, the plan is its argument, and your judgment settles what neither answers. When unsure about something reversible, pick the most conservative reasonable option and keep going.

Record every decision as:

`Ruling: <what you decided> — <why> — <what it costs if wrong>`

Deviating from the plan without a recorded ruling is a decision made in secret. Every ruling goes into your final status report under "Assumptions".

## The four hard stops

Only these four stop you. For each one, call `ask_lead` **before** acting, with the options and your recommendation:

1. **Irreversible.** Deleting data, dropping tables, rewriting git history, overwriting files you did not create in this task.
2. **Security-sensitive.** Auth, permissions, secrets, crypto, anything that widens access (CORS beyond localhost, public ports, disabled checks).
3. **Side effects outside the workspace.** Pushing, publishing, deploying, sending messages, calling paid or production APIs, installing things system-wide, writing outside the worktree.
4. **Every path is a guess.** The plan is so broken or the goal so unclear that no option is more defensible than another.

Anything else — naming, structure inside your files, a small mismatch between two tasks' interfaces, a missing import — is yours to rule on. "Should I continue?" is never a question for the Lead.

## Setup

1. Read your task brief fully. If it names a plan file, read your task in it, plus its Global constraints and Review focus. If it names a spec, read that too; conflicts in the plan resolve against the spec.
2. Note the done command and the files you may and may not touch.
3. Create a progress file next to the plan (e.g. `docs/plans/<plan>.progress.md`) or, if none, in your report notes. First line: `# Progress — <task name>`.
4. If a progress file exists and lists steps as complete, trust it and `git log` over your memory. Resume at the first incomplete step. Don't redo committed work.
5. Load test-driven-development. It governs every step.

## Work the steps

Steps come in RED → GREEN order. Write the test first, run it first. Watching it fail is a step, not a formality: a test that passes before the code exists is a finding about the test.

Every step that runs a command has an `Expected:` line. Run the command, read the whole output, compare. Three outcomes:

- **Matches.** Next step.
- **The code is wrong.** Use systematic-debugging. Find the cause. Never patch the symptom to make the output match.
- **The plan is wrong** — a step contradicts the spec, an interface from another task doesn't match, a command that can't work. Make the smallest change that satisfies the spec, record a `Ruling:`, continue. If the fix would cross a hard stop or touch files you don't own, `ask_lead` instead.

Mark each step done in the progress file in the same tool call as the commit. Don't save bookkeeping for later.

Run each command on its own from the repo root. No `&&`, no `;`, no `cd`.

## Stay in your lane

- Touch only the files your task lists. Need another file? If it's clearly part of your deliverable, rule and record it. If another member owns it, `ask_lead`.
- Never edit tests or checks written by someone else to make your code pass. If a check seems wrong, `ask_lead` with the evidence.
- Don't add features the task doesn't ask for (YAGNI). Note ideas as concerns instead.

## The completion contract

Before you call the task done, all of these are true **with evidence from this session**, not inferred from the diff looking right:

- Every test the brief names exists, ran in this task, and you read its output.
- The done command ran last and passed. You read the full result.
- Every `Expected:` line was compared against real output.
- Every deviation has a `Ruling:` line.

Then follow verification-before-completion and reporting-status. If any item is missing, the task is not done: finish it.

## When you are over your head

Bad work is worse than no work. Escalating costs nothing. Stop and report `BLOCKED` or `NEEDS_CONTEXT` (see reporting-status) when:

- The task needs an architectural decision with several valid approaches the plan didn't settle.
- You've read file after file without getting closer to understanding.
- You've tried three fixes and each revealed a new problem (see systematic-debugging).

Say exactly what you tried, what you found, and what would unblock you.

## Common rationalizations

| Excuse | Reality |
|---|---|
| "I remember what the task says" | You remember a summary. The brief has the exact values. Read it. |
| "The plan's code is right, skip watching the test fail" | A test you never saw fail proves nothing. Run it. |
| "I'll run the full suite at the end" | Per-step runs tell you which step broke it. The end run is the contract, not a substitute. |
| "The plan is wrong, I'll quietly do the right thing" | Do the right thing and record the ruling. |
| "Let me check with the Lead before the next step" | Only the four hard stops stop you. |
| "It should pass, the change was trivial" | "Should" is not evidence. Run the done command. |
| "I'll fix that unrelated thing while I'm here" | Not your task. Note it as a concern. |

---
Adapted from obra/superpowers skills/executing-plans (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI: members ask their Lead via ask_lead, four hard stops, progress file instead of plugin scripts.
