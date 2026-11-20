# Thread view: build spec (Phase 2)

Written 2026-11-18. The app is GPUI (`crates/rui-app`). The daemon data comes from `crates/rui-daemon/src/thread.rs` via `GET /threads`, `GET /threads/{id}`, `GET /threads/{id}/nodes/{node_id}?after_id=`.

## Sources and which one wins

No Open Design mock has the three-lane view, so this spec mixes three sources. When they disagree, `docs/owner-flow.md` wins on words and behaviour.

- **A** `rui-refined-console.html` (project 1901a008…, 2026-11-15): board, drawer, Inbox, decision card. Data `THREADS`, `INBOX`; render fns `renderThreads`, `renderInboxDetail`, `renderInbox`, `renderTaskDetail`.
- **C** `rui-console-v3.html` (project 483e5071…, 2026-11-14): lanes, now line, glass inspector.
- **owner-flow.md**: only five status words: Needs you, Working, Waiting, Stopped, Done. The task page replaces the drawer.

## Mapping daemon → UI

| Daemon | Shown as |
|---|---|
| node status `running` | Working (theme `run`) |
| `planned` | faded node, after the now line |
| `waiting`, `stalled` | Waiting (theme `wait`); stalled adds "No activity for 10 min" |
| `needs_you`, `failed` | Needs you (theme `block`) |
| `owner` | "You're doing this step" (theme `think`) |
| `done` | Done (theme `done`) |

## Screens

**Work board** (`views/activity.rs`): load from `GET /threads` instead of building threads from Store tasks. Row: title, client dot + name, status word, story line, needs-you count, updated time. Keep `moving_row()` and the `("thread-in", id)` animation id.

**Home needs-you strip**: above the board, one card per `needs_you` decision across threads, sorted high → medium → low → unknown risk. Each card is a compact decision card.

**Thread page** (`views/activity/task_page.rs`):
- Header: title, client, status word, story (one line), Stop / Resume.
- Lanes: three rows labelled "Came in", "Agent work", "Went out" (lanes `in`, `work`, `out`). Label column ~206px, then a track. Nodes laid out left → right by `started_at` (planned nodes after the now line, faded). `after[]` drawn as thin connectors.
- Now line: 1.5px vertical line, ~35% fg alpha, label "now".
- Node chip: title, agent, loop dots from `attempts{used,max}`, output label ("Tests pass" / "Tests fail" / "Pull request ready").
- Click a node → Inspector.

**Inspector** (right glass panel ~420px, slides in): sections "What happened" (paged events), "What it produced" (`output`), "Tool calls", "Done check" (`done_checks` output tails), "Cost". Controls for subtask nodes: "Wait for me before this step" (checkpoint), "Take over", "Hand back" (with note), "Retry".

**Decision card** (shared by thread page, Home and Inbox): eyebrow "{Client} · needs a decision", ask, "Why the Lead couldn't decide" (`why`), ladder chips "Pod · why → Account Lead · why → You", risk chip, "Reversible" / "Can't be undone", "If approved:" numbered `if_approved`, "Review the change" link, buttons from `actions` (Approve / Ask for changes / Answer…). After an answer, show "Sending… Undo" for `undo_secs`.

**Inbox slide-over**: opened from the rail; tabs Decisions / Snoozed. Snooze hides an item for 1 hour / until tomorrow (kept in the app's local state; no daemon change). Mentions / Assigned are left out until there is data for them.

## Wording to keep from mock A

- Empty Inbox: "Nothing is waiting on you. Every thread settled below you."
- Composer placeholder: "Answer it, or pass down new instructions…", button "Send down".
- Untrusted quote footer: "Quoted as data. Nothing in it is treated as an instruction."
