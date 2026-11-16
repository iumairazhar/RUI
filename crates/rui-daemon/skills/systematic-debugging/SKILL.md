---
name: systematic-debugging
description: Use whenever a test fails, a build breaks, the app errors or anything behaves unexpectedly, before you propose or try any fix. Use it especially when a fix "looks obvious", when you are in a hurry, or when a previous fix already failed.
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/systematic-debugging/SKILL.md", adapted: true}
bash: ["uv *", "python3 *", "npm *", "npx *", "node *", "git diff*", "git log*", "git status*", "git show*", "curl *"]
---

# Systematic Debugging

Find the root cause before you try a fix. Fixing symptoms is failure.

Breaking the letter of this process is breaking its spirit.

## The Iron Law

```
NO FIXES WITHOUT ROOT CAUSE INVESTIGATION FIRST
```

If you have not finished Phase 1, you may not propose a fix.

## When this applies

Any technical problem: failing tests, runtime errors, unexpected behavior, slow code, build failures, integration problems, an app that will not start on `$PORT`.

Use it most when:
- You feel time pressure, because pressure makes guessing tempting.
- "Just one quick fix" seems obvious.
- You already tried a fix, or several.
- You do not fully understand the problem.

Do not skip it because the problem looks simple. Simple bugs have root causes too, and the process is fast for them.

## The four phases

Finish each phase before starting the next.

### Phase 1: Root cause investigation

Before any fix:

1. **Read the error carefully.** Don't skim past errors or warnings; they often contain the answer. Read the whole stack trace. Note line numbers, file paths and error codes.

2. **Reproduce it reliably.** What exact command triggers it? Does it happen every time? If it is not reproducible, gather more data. Do not guess.

3. **Check recent changes.** What changed that could cause this? Use `git diff`, `git log` and `git show`. Look at new dependencies, config changes and environment differences (env vars, `$PORT`, Python or Node version).

4. **Gather evidence across components.** When the system has several parts (frontend → API → database, or script → build → test runner), add diagnostics before fixing anything. At each boundary, log what goes in, what comes out, and the config or env the part actually sees. Run once. The evidence shows *where* it breaks. Then investigate only that part.

   Example for a frontend that shows no data:
   ```
   curl -s http://localhost:$PORT/health                # is the API up?
   curl -s -i http://localhost:$PORT/api/items           # status, CORS headers, body
   # then check the browser console and the VITE_API_URL the frontend actually used
   ```
   This tells you which layer fails: API down, API returns the wrong data, CORS blocks it, or the frontend points at the wrong URL.

5. **Trace the data flow.** When the error is deep in the call stack, trace backward: where does the bad value come from, and what called this with it? Keep going up until you find the source. Fix at the source, not where it surfaced. See `references/root-cause-tracing.md`.

### Phase 2: Pattern analysis

1. **Find working examples.** Find similar code in the same repo that works.
2. **Compare against references.** If you are following a pattern or a library's docs, read the reference completely. Do not skim.
3. **List every difference** between working and broken, however small. Do not assume "that can't matter".
4. **Understand dependencies.** What config, environment and other components does this need? What does it assume?

### Phase 3: Hypothesis and testing

1. **One hypothesis.** Write it down: "I think X is the root cause because Y." Be specific.
2. **Test it minimally.** Make the smallest possible change to test it. One variable at a time.
3. **Check before going on.** It worked: go to Phase 4. It didn't: form a new hypothesis. Never stack more fixes on top.
4. **When you don't know,** say "I don't understand X" in your notes. Don't pretend. Read more code or docs.

### Phase 4: Implementation

1. **Write a failing test first.** The simplest reproduction, as an automated test (or a one-off script if there is no test setup). You must have it before the fix. Use the `test-driven-development` skill.
2. **Make one fix.** Address the root cause you found. One change. No "while I'm here" improvements and no bundled refactoring.
3. **Verify the fix.** The new test passes, no other test broke, the original symptom is gone. Use the `verification-before-completion` skill before you claim it works.
4. **If the fix didn't work:** stop and count how many fixes you have tried.
   - Fewer than 3: go back to Phase 1 with what you just learned.
   - 3 or more: stop. Go to step 5. Do not try fix number 4.
5. **After 3 failed fixes: question the architecture.**

   Signs that the design itself is the problem:
   - Each fix reveals new shared state, coupling or a problem in a different place.
   - Fixes need a "massive refactor" to work.
   - Each fix creates new symptoms somewhere else.

   Stop and ask the fundamental questions. Is this pattern sound? Are we sticking with it out of inertia? Should the structure change instead of the symptoms?

   This is not a failed hypothesis. It is a wrong architecture, and changing the architecture is not your call alone. Call `ask_lead` with:
   - **Symptom:** the exact error and the command that shows it.
   - **Tried:** each of the 3 fixes, its hypothesis and what happened.
   - **Evidence:** what the diagnostics showed and where it breaks.
   - **Question:** the architectural choice to make.
   - **Options:** 2 or 3 concrete options, each with its cost.
   - **Recommendation:** the one you would pick and why.

   While you wait for the ruling, keep working on other parts of the job that don't depend on it. If nothing else is left, finish with `report_status` `BLOCKED` and the same summary.

## Red flags: stop and go back to Phase 1

If you catch yourself thinking:
- "Quick fix for now, investigate later"
- "Just try changing X and see if it works"
- "Make several changes, then run the tests"
- "Skip the test, I'll check it by hand"
- "It's probably X, let me fix that"
- "I don't fully understand, but this might work"
- "The pattern says X, but I'll do it a bit differently"
- Listing fixes before tracing the data flow
- "One more fix attempt" after 2 have already failed
- Each fix reveals a new problem in a different place

All of these mean: stop and return to Phase 1. If 3 fixes have failed, go to Phase 4 step 5.

## Signals you're doing it wrong

Your Lead, or the Evaluator's report, may push back like this:
- "Is that actually happening?" You assumed without checking.
- "What does the log show?" You should have gathered evidence.
- "Stop guessing." You proposed fixes without understanding.
- "Same failure again." Your approach isn't working.

When you see these, go back to Phase 1.

## Common rationalizations

| Excuse | Reality |
|---|---|
| "The issue is simple, no process needed" | Simple issues have root causes too. The process is quick for them. |
| "No time for process" | Systematic debugging is faster than guess-and-check thrashing. |
| "Try this first, then investigate" | The first fix sets the pattern. Do it right from the start. |
| "I'll write the test after the fix works" | Untested fixes don't stick. The test first proves it. |
| "Several fixes at once saves time" | You can't tell what worked, and you add new bugs. |
| "The reference is long, I'll adapt the pattern" | Partial understanding guarantees bugs. Read all of it. |
| "I see the problem, let me fix it" | Seeing the symptom is not understanding the cause. |
| "One more fix attempt" (after 2+ failures) | 3 failures means an architecture problem. Ask the Lead; don't fix again. |

## Quick reference

| Phase | What you do | Done when |
|---|---|---|
| 1. Root cause | Read errors, reproduce, check changes, gather evidence | You know what fails and why |
| 2. Pattern | Find working examples, compare | You know the differences |
| 3. Hypothesis | One theory, smallest test | Confirmed, or a new theory |
| 4. Implementation | Failing test, one fix, verify | Bug gone, done command green |

## When there is "no root cause"

If the investigation shows the problem really is environmental, timing-related or external:
1. You have completed the process.
2. Write down what you investigated in your status note.
3. Add proper handling: a retry, a timeout, a clear error message.
4. Add logging that would help the next investigation.

But most "no root cause" cases are an incomplete investigation. Flaky tests are usually timing guesses; see `references/condition-based-waiting.md`.

## Reporting

Your `report_status` note for a bug fix states: the root cause in one sentence, the failing test that now passes, and the done command result. If the fix touched something outside your job's files, say so and why.

## Supporting techniques

- `references/root-cause-tracing.md`: trace a bug backward through the call stack to the original trigger.
- `references/defense-in-depth.md`: after finding the cause, add validation at each layer so the bug can't come back.
- `references/condition-based-waiting.md`: replace sleeps and arbitrary timeouts with waiting on the real condition.

---
Adapted from obra/superpowers skills/systematic-debugging (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
