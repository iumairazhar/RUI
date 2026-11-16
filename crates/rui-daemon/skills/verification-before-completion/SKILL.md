---
name: verification-before-completion
description: Use whenever you are about to say work is done, fixed or passing, before every report_status call and before handing work to anyone else. Use it even when you are confident, because confidence is not evidence.
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/verification-before-completion/SKILL.md", adapted: true}
bash: ["uv *", "python3 *", "npm *", "npx *", "node *", "curl *", "git diff*", "git status*"]
---

# Verification Before Completion

Evidence before claims, always.

Breaking the letter of this rule is breaking its spirit.

## The Iron Law

```
NO COMPLETION CLAIMS WITHOUT FRESH VERIFICATION EVIDENCE
```

If you have not run the verification command in this turn, after your last change, you cannot claim it passes.

## The gate

```
BEFORE any status claim, or any sign of satisfaction:

1. IDENTIFY  Which command proves this claim? (Usually the job's done command.)
2. RUN       Run the FULL command, fresh, after your last edit.
3. READ      Read the full output. Check the exit code. Count the failures.
4. VERIFY    Does the output confirm the claim?
               No  -> report the real status, with the evidence.
               Yes -> make the claim, with the evidence.
5. ONLY THEN call report_status.

Skipping any step is not verifying. It is a false report.
```

## In RUI: the done command and report_status

Every job names a done command, for example `uv run pytest -q`, `npm test` or `npm run build`.

1. Run the done command fresh, from the repo root, after your last edit. A run from before your last edit doesn't count.
2. Read the result. Count passed and failed. Check that there are no errors or new warnings.
3. Pick the status code from the evidence, not from how you feel:

| Evidence | Code |
|---|---|
| Done command passes and every done criterion is met | `DONE` |
| Done command passes, but you have a doubt (an assumption you made, a criterion you couldn't check, a failure you didn't cause) | `DONE_WITH_CONCERNS` |
| You can't tell whether it's right without information you don't have | `NEEDS_CONTEXT` |
| Done command fails and you can't fix it within the job | `BLOCKED` |

4. Quote the result in your status note: the command and its summary line. For example:
   ```
   DONE. `uv run pytest -q` -> 14 passed in 0.82s. Criteria: POST /items 201, GET /items/{id} 404 on missing, /health ok.
   ```
   Not: `DONE. Tests should pass now.`

Never report `DONE` with a failing done command. Never edit, skip or weaken a check that judges your own work to make it pass.

## What each claim needs

| Claim | Requires | Not enough |
|---|---|---|
| Tests pass | Test command output with 0 failures | An earlier run, "should pass" |
| Linter clean | Linter output with 0 errors | A partial check, extrapolation |
| Build succeeds | Build command exits 0 | Linter passing, "logs look fine" |
| Bug fixed | The original symptom, re-tested, now passes | Code changed, assumed fixed |
| Regression test works | Red-green cycle seen (see below) | The test passes once |
| App runs | Started on `$PORT` and `curl` to `/health` (or the page) succeeds | "The server code looks right" |
| A member's work is done (for Leads) | You read the diff and ran the done command yourself | The member reported DONE |
| Requirements met | Line-by-line check against the done criteria | Tests passing |

## Red flags: stop

- Words like "should", "probably", "seems to", "looks good".
- Satisfaction before verification ("Great!", "Perfect!", "Done!").
- About to call `report_status` without having run the done command since your last edit.
- Trusting another agent's success report.
- Relying on a partial check (one test file instead of the suite).
- "Just this once."
- Wanting the loop to be over.
- Any wording that implies success without a fresh run behind it.

## Rationalizations

| Excuse | Reality |
|---|---|
| "It should work now" | Run the verification. |
| "I'm confident" | Confidence is not evidence. |
| "Just this once" | No exceptions. |
| "The linter passed" | A linter is not a compiler or a test suite. |
| "The member said it's done" | Verify independently. |
| "A partial check is enough" | A partial check proves nothing about the rest. |
| "I used different words, so the rule doesn't apply" | Spirit over letter. |

## Key patterns

Tests:
```
Good: run the done command, see "34 passed", then say "all 34 tests pass".
Bad:  "Should pass now." / "Looks correct."
```

Regression test (red-green):
```
Good: write the test, run it (pass), revert the fix, run it (MUST fail),
      restore the fix, run it (pass).
Bad:  "I added a regression test." (never seen red)
```

Build:
```
Good: run the build, see exit 0, then say "the build passes".
Bad:  "The linter passed, so the build is fine."
```

Requirements:
```
Good: re-read the done criteria, check each one, report gaps or completion.
Bad:  "Tests pass, so the job is done."
```

Delegated work (Leads):
```
Good: member reports DONE -> read the diff -> run the done command -> report the real state.
Bad:  pass the member's DONE up to Rui unchecked.
```

## When it applies

Always before:
- Any form of success or completion claim, including paraphrases and implications.
- Any `report_status` call.
- Answering "is it done?" from a Lead or Rui.
- Handing work to the next member or the Evaluator.

---
Adapted from obra/superpowers skills/verification-before-completion (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
