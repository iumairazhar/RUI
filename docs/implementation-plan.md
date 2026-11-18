# RUI implementation plan

Written 2026-11-18 against branch `feat/per-client-account-lead` (PR #1). It covers every item in the RUI spec that is not built yet.

## Where things stand

Built or on PR #1: Rui, Account Lead, Pod members, done_command prechecks, loops, protected paths, grants, escalation ladder with risk/reversible/if_approved, Inbox, skills, Playbooks, client state, thread projector and `/threads` routes.

Missing, found by reading the code:

| Area | State | Where |
|---|---|---|
| merge / forward_pr / approve off the async runtime | not done | `mcp.rs:1311`, `mcp.rs:1162`, `scheduler.rs:1144` |
| `/threads` skips old done threads | not done | `thread.rs:548` |
| takeover node kind, checkpoint, take-over, hand-back | absent | `thread.rs:238` |
| approval hold with undo | partial (hold exists, no undo) | `mcp.rs:656` |
| heartbeat wake for stalled work | absent | `scheduler.rs:350` |
| seats: many seats, API keys, switch at 90%, stop when used up | partial | `seat.rs` |
| Coach, Scout | absent (only a `researcher` role) | |
| email, calendar, manual knowledge, connectors | absent | |
| CI / PR comments routed back, triggers, prod deploy flow | absent | |
| cost per client, tokens, analytics | partial (`db.rs:734` cost by task) | |
| Codex and other CLIs | absent | `runner.rs:146` |
| Thread view UI (lanes, decision cards, inspector, home, inbox tabs/snooze) | absent in app | app never calls `/threads` |

## Rules for every phase

Each phase is one small branch-sized chunk, with one commit per logical change.

A phase is done only when `cargo test --workspace` passes, `cargo clippy --workspace` adds no new warnings, and the phase's own check (listed below) passes. If a check fails, fix and rerun; after three failed rounds on the same problem, stop and re-plan instead of pushing on.

No new database migration is written without the owner's yes. Where a phase would need one, it stores the data as events first, which the spec already allows.

UI phases are checked by running the app and taking screenshots, not only by tests.

## Phase 0 — finish PR #1

1. Run `merge_subtask`, `forward_pr` and `approve` through the existing `blocking()` helper.
2. Re-run the full test suite and the manual GitHub round trip (create task, split, build, forward PR).
3. Owner merges PR #1.

Check: tests pass; a real task goes from chat to an open PR with Lead remarks.

## Phase 1 — thread backend controls

1. `/threads` skips done threads older than N days unless `?all=1`.
2. Checkpoint: `POST /tasks/{id}/checkpoint` stores a `checkpoint_set` event; the scheduler won't start that subtask until the owner releases it. The node shows `waiting`.
3. Take over / hand back: `POST /tasks/{id}/take-over` stops the agent and marks the node `owner`; `POST /tasks/{id}/hand-back` takes a note, reruns the done_command, and resumes or marks done (see open decision 8).
4. Approval hold with undo: approving writes `approval_pending` with a run-at time; `POST /approvals/{event_id}/cancel` cancels inside the window; land and deploy have no window.
5. Heartbeat: a scheduler tick every few minutes finds stalled subtasks and pending items, logs a `heartbeat` event, and raises stalls to the Lead.

Check: unit tests per route plus a scheduler test that a checkpointed subtask does not start and a stalled one gets raised.

## Phase 2 — thread view UI (GPUI app)

Read the Open Design mocks first and match them.

1. API client for `/threads`, `/threads/{id}`, node detail.
2. Thread page: three lanes, now line, story, one node per subtask.
3. Inspector panel: what happened, output, tool calls, done check output, cost, paged events.
4. Decision card shape, used in thread and Inbox.
5. Home: needs-you cards across clients sorted by risk, then threads by last activity.
6. Inbox slide-over with Decisions / Mentions / Assigned / Snoozed tabs and snooze.
7. Controls wired to Phase 1 routes.

Check: run the app against a daemon with seeded tasks; screenshot each screen; click every control and confirm the matching event lands.

## Phase 3 — seats

1. Seat list in owner state: subscription seats from CLI login, plus API keys read from the environment or keychain (never stored in plain files).
2. Usage tracking per seat from result events.
3. Switch to the next seat at 90%; when all are used, Inbox alert and the scheduler stops starting new turns.
4. Seat page shows all seats and usage.

Check: tests with fake usage numbers for switch and stop; app screenshot.

Blocked by open decision 1 (are subscription seats allowed for 24/7 use).

## Phase 4 — Scout and Coach

1. Scout role + prompt + MCP server (read-only tools, Fetch URL). The Lead can call it before planning; the owner can call "Ask Scout to diagnose" on a failed node.
2. Coach: runs on finished threads, reads send-backs, mistakes and owner corrections, writes proposals (`coach_proposal` events) for Playbook, role or skill edits. Proposals show in the Inbox; approve applies the edit. Coach can't touch done_commands or evals.

Check: scripted test with a fake finished thread produces a proposal; approving it changes the Playbook file.

## Phase 5 — inputs

1. Manual client knowledge: owner-added notes in the client folder, injected above inferred knowledge in Lead prompts.
2. Client email: a mail connector reads a label/folder, the Lead turns each mail into a task proposal in the Inbox. Mail text is wrapped as untrusted data.
3. Calendar prep: before a client meeting, build a brief from recent threads, mails and notes; shown in the Inbox (open decision 3 decides if anything is sent).

Check: fixture mails and calendar events produce the right proposals and brief; a prompt-injection fixture mail does not change behaviour.

Needs the owner to choose providers (Gmail / Google Calendar / IMAP) and set up OAuth themselves.

## Phase 6 — later

CI failures and PR review comments routed back to the agent that made the change; triggers (PR merged, client reply, schedule); production deploy flow with Inbox approval; cost per client and tokens; analytics page; Codex runner behind a `runner` trait.

Each gets its own plan when we get there.

## Open decisions (need the owner)

1. Seat terms: subscription seats for automated use, or default to API keys?
2. Final list of Rui tools.
3. Does calendar prep need approval before anything is sent?
4. Thread deadline: new column (owner's migration) or an event?
5. Is a 10 minute stall limit right for every role?
6. Undo length: fixed 10 s or a setting?
7. Cost on subscription seats: dollars or tokens?
8. Hand back: require the done check to pass, or allow forcing done?
