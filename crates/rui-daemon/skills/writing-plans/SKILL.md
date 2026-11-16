---
name: writing-plans
description: Use when a Lead has a goal, spec or owner request for multi-step work and is about to split it across the pod — before any member touches code, even if the work looks small enough to "just start".
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/writing-plans/SKILL.md", adapted: true}
bash: ["git log*", "git status*", "ls *", "mkdir *"]
---
# Writing Plans

You are the Lead. Write the plan for members who have never seen this codebase or this spec. Assume each member writes idiomatic code once they know the exact interface and the exact test, and makes a reasonable choice wherever the plan leaves one open. What they cannot know is what you decided: which files, which names and signatures, which values from the spec, which tests prove each task, and who owns what. Write those down.

Bite-sized tasks. DRY. YAGNI. TDD. Frequent commits.

**Save plans to:** `docs/plans/YYYY-MM-DD-<topic>.md` in the worktree. Members read it from there.

## Scope check

If the goal covers several independent subsystems, split it into separate plans, one per subsystem. Each plan must produce working, testable software on its own. If the goal itself is unclear in a way that changes the plan (not just a detail), ask Rui before planning — see the escalation-ladder skill. Do not plan on a guess.

## File structure first

Before writing tasks, map every file that will be created or changed and what each is responsible for. This is where decomposition gets locked in.

- One clear responsibility per file, with a well-defined interface.
- Prefer small, focused files. Members edit focused files more reliably.
- Files that change together live together. Split by responsibility, not by technical layer.
- In an existing codebase, follow its patterns. Don't restructure on a whim; include a split only when a file you must change has become unwieldy.

## Task right-sizing

A task is the smallest unit that carries its own test cycle and is worth its own review.

- Fold setup, config, scaffolding and docs into the task whose deliverable needs them.
- Split only where a reviewer could reject one task while approving its neighbour.
- Every task ends with an independently testable deliverable.
- **Every task has exactly one owner role** from this pod (e.g. `backend`, `frontend`, `tester`). Never assign to a role that is not in the pod. If no pod role fits, that is a staffing question for Rui, not a task.
- Two tasks that may run at the same time must not touch the same files.
- **The checks that judge a task are never written by the task's owner.** Acceptance tests belong to `tester` (or to a task that runs before the build task and is owned by someone else).

## Step granularity

Each step is one action with a checkable result, roughly 2–5 minutes of work:

- "Write the failing test" — step
- "Run it and confirm it fails for the expected reason" — step
- "Write the minimal code to make it pass" — step
- "Run the tests and confirm they pass" — step
- "Commit" — step

## Plan header (required)

```markdown
# <Feature> Plan

**Goal:** <one sentence>
**Architecture:** <2–3 sentences on the approach>
**Stack:** <key technologies>
**Spec:** <path or quote of the owner's request this plan implements>
**Pod:** <roles available, e.g. LD, BE, FE, TS, EV>

## Global constraints
<Project-wide requirements copied verbatim from the spec: versions, limits, naming,
copy, ports ($PORT), "no deploys to production". One line each.
Every task implicitly includes this section.>

## Review focus
<The ~5 inputs or failure modes the spec implies but no task's tests exercise, most
likely to bite a real user first: "empty list → shows empty state, not a crash".
For each line, add the pinning test to the task that owns the code.>
```

## Task template

````markdown
### Task N: <Component>

**Owner:** backend
**Depends on:** Task 2 (uses `create_item`)
**Files:**
- Create: `app/items.py`
- Modify: `app/main.py:12-30`
- Test: `tests/test_items.py`

**Interfaces:**
- Consumes: `get_db() -> Session` from Task 1
- Produces: `create_item(db: Session, name: str) -> Item`; `POST /items` → 201 `{"id": int, "name": str}`

**Done criteria:**
- `POST /items` with an empty name returns 422.
- All tests in `tests/test_items.py` pass.

**Done command:** `uv run pytest -q`

- [ ] **Step 1: Write the failing test**
```python
def test_create_item_rejects_empty_name(client):
    r = client.post("/items", json={"name": ""})
    assert r.status_code == 422
```
- [ ] **Step 2: Run it and confirm it fails**
  Run: `uv run pytest tests/test_items.py -q`
  Expected: FAIL — 404 (route does not exist yet)
- [ ] **Step 3: Implement `create_item` and the `POST /items` route in `app/items.py`**
- [ ] **Step 4: Run it and confirm it passes**
  Run: `uv run pytest tests/test_items.py -q`  Expected: PASS
- [ ] **Step 5: Commit** — `git add app/items.py tests/test_items.py` then `git commit -m "feat: create items"`
````

Commands follow the daemon rule: each command on its own line, from the repo root, no `&&`, no `;`, no `cd`.

## What a step must contain

A step is done when the member can write exactly one reasonable thing from it. Unambiguous, not exhaustive.

- **Test step:** the test name and its assertions, as code, with the spec's exact values.
- **Code step:** the exact signature (name, parameters, return type), the file, and any values the spec pins. The member writes the body. Include a body only for an algorithm the signature and tests don't determine, or for exact copy the spec fixes.
- **Verification step:** the command and the output that means it passed.
- **Reference to another task:** point at that task's Interfaces block. Don't repeat its code.

**No placeholders, ever.** These lines decide nothing and are plan failures:
"TBD", "handle edge cases", "add appropriate validation", "write tests for the above", "similar to Task 3", a type or function no task defines.

A plan longer than the code it describes has written the code instead. That is the opposite failure.

## Self-review (run it yourself before dispatching)

1. **Spec coverage.** For each requirement in the spec, point to the task that implements it. Add a task for any gap.
2. **Step scan.** Every step lets the member write exactly one reasonable thing. Fix lines that decide nothing; cut function bodies the signature and tests already determine.
3. **Type consistency.** Names, signatures and fields in later tasks match what earlier tasks produce. `clearLayers()` in Task 3 and `clearFullLayers()` in Task 7 is a bug.
4. **Ownership.** Every task has one owner role in the pod. No two parallel tasks share a file. Nobody owns the checks that judge their own output.
5. **Done criteria.** Every task has observable done criteria and one done command that proves them.
6. **Review focus.** Each listed failure mode has a pinning test in its owning task. An empty section means you checked and found none.
7. **Proportion.** If code blocks are most of the document, replace bodies with signatures and assertions.

Fix issues inline. No second review pass — fix and move on.

## Dispatch

When the plan is saved and self-reviewed, hand each task to its owner using the handoff-contract format: objective, output, done criteria, done command, files you may touch, files you must not touch, and the plan path. Dispatch independent tasks in parallel; dispatch dependent tasks after their dependency reports `DONE`. Answer member questions with rulings (escalation-ladder). Don't ask the owner anything directly — anything you can't rule on goes to Rui.

## Red flags

| Thought | Reality |
|---|---|
| "The member will figure out the file layout" | Then two members will figure it out differently. Decide it. |
| "I'll leave tests to the end" | Then no task proves itself. TDD per task. |
| "This task is for whoever is free" | Unowned tasks get done twice or never. One owner role. |
| "BE can write the acceptance tests for BE's endpoint" | No agent writes the checks that judge its own output. |
| "TBD — decide during build" | That is a stall disguised as a plan. Decide now or ask Rui now. |

---
Adapted from obra/superpowers skills/writing-plans (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI: Lead-to-pod dispatch, one owner role per task, done criteria and done command per task.
