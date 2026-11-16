---
name: receiving-code-review
description: Use when a member receives review findings (from the Evaluator, a reviewer, the Tester or the Lead) and is about to act on them — especially when a finding seems unclear, wrong, or like extra scope. Verify before implementing; never agree performatively.
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/receiving-code-review/SKILL.md", adapted: true}
bash: ["git diff*", "git log*", "git status*", "git show*", "grep *", "rg *"]
---
# Receiving Code Review

Code review needs technical evaluation, not emotional performance.

**Core principle:** Verify before implementing. Ask before assuming. Technical correctness over social comfort.

## The response pattern

```
WHEN you receive review findings:
1. READ      the whole set without reacting
2. UNDERSTAND restate each finding in your own words (or ask)
3. VERIFY    check it against the real code
4. EVALUATE  is it technically right for THIS codebase?
5. RESPOND   technical acknowledgement or reasoned pushback
6. IMPLEMENT one item at a time, test each
```

## Forbidden responses

Never write:
- "You're absolutely right!"
- "Great point!" / "Excellent feedback!"
- "Thanks for catching that!" — or any thanks at all
- "Let me implement that now" (before verifying)

Instead: restate the technical requirement, ask a specific question, push back with reasoning, or just fix it. The code shows you heard the feedback.

## Unclear findings: stop everything

```
IF any finding is unclear:
  STOP — implement nothing yet
  ask about the unclear ones first
WHY: findings are often related. Partial understanding = wrong implementation.
```

Example: the review lists findings 1–6. You understand 1, 2, 3 and 6 but not 4 and 5.
- Wrong: fix 1, 2, 3, 6 now and ask about 4 and 5 later.
- Right: `ask_lead`: "I understand findings 1, 2, 3, 6. Need clarification on 4 and 5 before implementing: [specific question, options A/B, my recommendation]."

Reviewers don't talk to you directly mid-task. Questions to a reviewer go through your Lead via `ask_lead`.

## Before implementing any finding

Check:
1. Is it technically correct for this codebase?
2. Would the fix break existing behaviour or tests?
3. Is there a reason the current code is the way it is (git log, comments, the plan's rulings)?
4. Does it hold on every platform and version this project targets?
5. Did the reviewer have the full context (plan, done criteria, earlier rulings)?

If you can't verify a finding, say so: "I can't verify this without X. Options: investigate (cost: ~N steps) / proceed as suggested / leave it. Recommend: …" — via `ask_lead`.

If a finding conflicts with a Lead's earlier ruling or the plan's architecture, don't pick a side yourself: `ask_lead`.

## YAGNI check on "do it properly" findings

```
IF the reviewer asks to "implement this properly" / add a "real" feature:
  search the codebase for actual callers
  IF unused: "Nothing calls this endpoint. Remove it (YAGNI) instead of extending it?"
  IF used:   implement properly
```

The Lead decides scope. If we don't need a feature, don't add it.

## Implementation order

```
1. Clarify every unclear finding FIRST
2. Then fix in this order:
   - Blocking: Critical / P0–P1 — broken behaviour, security, data loss
   - Simple: typos, imports, small Important fixes
   - Complex: refactors, logic changes
3. Test each fix on its own (write a test that reproduces the finding, watch it fail, make it pass)
4. Run the done command at the end — no regressions
```

Minor findings are not automatic work. Fix them only if trivial and inside your files; otherwise list them as deferred in your status note and let the Lead decide.

## When to push back

Push back when the suggestion:
- breaks existing behaviour,
- comes from a reviewer missing context,
- adds unused features (YAGNI),
- is technically wrong for this stack,
- ignores legacy or compatibility reasons,
- conflicts with the Lead's or Architect's decisions.

How: technical reasoning, not defensiveness. Cite the code, the test, or the ruling. Example:

> Reviewer: "Remove the legacy date parser."
> You: "Checked — `import_csv` (app/import.py:44) still passes the old format, and `test_import_legacy` covers it. Removing it breaks imports. Keep it, or change import_csv too? Recommend keep."

## Acknowledging correct feedback

- "Fixed: names are now trimmed before validation (app/items.py:18). Test: test_rejects_whitespace_name."
- Or just fix it and list it in your status note.

If you pushed back and were wrong: "Checked again — you're correct, X does Y. Fixing." No long apology, no defending the pushback.

## Reporting after a review round

Your status note (reporting-status) lists each finding with one of:
- `fixed — <test name> RED→GREEN`
- `pushed back — <one-line reason>`
- `deferred (Minor) — <one line>`
- `asked Lead — <question>`

Then the done command result.

## Common mistakes

| Mistake | Fix |
|---|---|
| Performative agreement | State the requirement or just act |
| Blind implementation | Verify against the code first |
| Batch of fixes, one test run | One fix at a time, test each |
| Assuming the reviewer is right | Check whether it breaks things |
| Avoiding pushback | Technical correctness over comfort |
| Partial implementation | Clarify all items first |
| Can't verify, proceed anyway | State the limit, ask_lead |

---
Adapted from obra/superpowers skills/receiving-code-review (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI: questions go through ask_lead, severity order, per-finding status lines.
