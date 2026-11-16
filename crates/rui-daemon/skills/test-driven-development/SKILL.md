---
name: test-driven-development
description: Use whenever you are about to write or change production code for a feature, bugfix, refactor or behavior change, before writing a single line of it. Use it even when the change looks too small to test or you plan to "add tests after".
license: MIT
metadata: {source: "https://github.com/obra/superpowers/blob/main/skills/test-driven-development/SKILL.md", adapted: true}
bash: ["uv *", "python3 *", "npm *", "npx *", "node *"]
---

# Test-Driven Development

Write the test first. Watch it fail. Write the least code that makes it pass.

If you never watched the test fail, you do not know whether it tests the right thing.

Breaking the letter of these rules is breaking their spirit. There is no clever reading that lets you skip a step.

## When this applies

Always:
- New features and new endpoints
- Bug fixes
- Refactoring
- Any change in behavior

The only exceptions are throwaway spikes, generated code and pure configuration files. If you think your case is another exception, it is not one. If you truly believe the job cannot be test-driven (for example, the job is a config-only change but the brief also asks for behavior), use `ask_lead` with your reasoning before you skip anything.

Thinking "skip TDD just this once"? Stop. That thought is the rationalization.

## The Iron Law

```
NO PRODUCTION CODE WITHOUT A FAILING TEST FIRST
```

Wrote code before the test? Delete it and start over.

No exceptions:
- Do not keep it as a "reference".
- Do not "adapt" it while writing the tests.
- Do not look at it.
- Delete means delete.

Implement fresh, driven by the tests.

## Red, Green, Refactor

```
RED       write one failing test
VERIFY    run it; it must FAIL, for the expected reason      (wrong failure -> back to RED)
GREEN     write the minimum code to pass
VERIFY    run it and the whole suite; all green               (still red -> fix code, not test)
REFACTOR  clean up while staying green
NEXT      next behavior -> RED
```

### RED: write one failing test

One minimal test that shows what should happen.

Good:
```python
def test_retries_failed_operation_three_times():
    attempts = 0
    def op():
        nonlocal attempts
        attempts += 1
        if attempts < 3:
            raise RuntimeError("fail")
        return "success"

    assert retry_operation(op) == "success"
    assert attempts == 3
```
Clear name, real behavior, one thing.

Bad:
```typescript
test('retry works', async () => {
  const mock = jest.fn()
    .mockRejectedValueOnce(new Error())
    .mockRejectedValueOnce(new Error())
    .mockResolvedValueOnce('success');
  await retryOperation(mock);
  expect(mock).toHaveBeenCalledTimes(3);
});
```
Vague name, and it tests the mock, not the code.

Requirements:
- One behavior per test. An "and" in the name means split it.
- A name that describes the behavior.
- Real code. Mocks only when the real thing is slow or external.

### Verify RED: watch it fail

Mandatory. Never skip. Run just that test:

```
uv run pytest -q tests/test_retry.py
npm test -- test/retry.test.js
```

Confirm all three:
- The test fails (an assertion fails; it does not crash with an import or syntax error).
- The failure message is the one you expected.
- It fails because the feature is missing, not because of a typo.

Test passes right away? You are testing behavior that already exists. Fix the test.

Test errors out? Fix the error and re-run until it fails for the right reason.

### GREEN: minimal code

Write the simplest code that passes the test.

Good:
```typescript
async function retryOperation<T>(fn: () => Promise<T>): Promise<T> {
  for (let i = 0; i < 3; i++) {
    try { return await fn(); } catch (e) { if (i === 2) throw e; }
  }
  throw new Error('unreachable');
}
```

Bad: adding `options` for max retries, backoff strategies and callbacks that no test asked for. That is YAGNI.

Do not add features, refactor other code or "improve" anything beyond what the test demands.

### Verify GREEN: watch it pass

Mandatory. Run the test, then run the project's whole suite (the job's done command).

Confirm:
- The new test passes.
- Every other test still passes.
- The output is clean: no errors, no new warnings.

New test fails? Fix the code, not the test.

Other tests fail? Fix them now.

A green run of your one file is not a green suite. Run the full done command even when the job named only one file. The job's scope bounds what you build, not what you verify. Any failure that run shows, including one you did not cause, goes into your `report_status` note by name. A red test you saw and did not mention is a false report.

### REFACTOR: clean up

Only after green:
- Remove duplication.
- Improve names.
- Extract helpers.

Keep the tests green. Do not add behavior while refactoring.

### Repeat

Next failing test for the next behavior.

## Good tests

| Quality | Good | Bad |
|---|---|---|
| Minimal | One thing. "and" in the name? Split it. | `test_validates_email_and_domain_and_whitespace` |
| Clear | Name describes the behavior | `test1` |
| Shows intent | Shows the API you want | Hides what the code should do |

Before writing or changing any test, read `references/writing-good-tests.md`. Its core rules:
- Name the production change that would make the test fail, before you write it.
- Assert on real behavior, never on a mock.
- Keep test-only code in test utilities, out of production classes.
- Learn a dependency's side effects before you mock it.

## Common rationalizations

| Excuse | Reality |
|---|---|
| "Too simple to test" | Simple code breaks. The test takes 30 seconds. |
| "I'll test after" | Tests written after pass immediately, which proves nothing. You never saw them catch anything. |
| "Tests after achieve the same goal" | Tests-after answer "what does this do?". Tests-first answer "what should this do?". Tests-after only check the cases you remembered. |
| "I already tested it manually" | Manual testing leaves no record and cannot be re-run. "It worked when I tried it" is not coverage. |
| "Deleting this much work is wasteful" | Sunk cost. The real choice is rewrite with confidence, or keep code you cannot trust. |
| "Keep it as reference, write tests first" | You will adapt it. That is testing after. Delete means delete. |
| "I need to explore first" | Fine. Throw the spike away, then start with a test. |
| "Hard to test means the test is the problem" | Hard to test means hard to use. Listen to the test and simplify the design. |
| "TDD slows me down" | Debugging in a later loop, or in the Evaluator's report, is slower. |
| "The existing code has no tests" | You are improving it. Add tests for the code you touch. |

## Red flags: stop and start over

- Code written before its test
- A test that passed the first time you ran it
- You cannot explain why the test failed
- Tests planned for "later"
- "Just this once"
- "I already manually tested it"
- "It's about the spirit, not the ritual"
- "Keep it as reference" or "adapt the existing code"
- "Already spent an hour on it, deleting is wasteful"
- "This case is different because..."

All of these mean: delete the code and start over with a failing test.

## Example: bug fix

Bug: an empty email is accepted.

RED:
```python
def test_rejects_empty_email(client):
    r = client.post("/signup", json={"email": ""})
    assert r.status_code == 422
```

Verify RED:
```
uv run pytest -q tests/test_signup.py
FAILED tests/test_signup.py::test_rejects_empty_email - assert 201 == 422
```

GREEN: add the validation (for example `EmailStr` or a `min_length` check) on the request model.

Verify GREEN: the test passes, then the full done command passes.

REFACTOR: share the validation with other fields if that is now duplicated.

## Checks before you report

- [ ] Every new function or endpoint has a test.
- [ ] You watched each test fail before implementing.
- [ ] Each failed for the expected reason (feature missing, not a typo).
- [ ] You wrote minimal code to pass each one.
- [ ] The full done command passes, with clean output.
- [ ] Tests use real code; mocks only where unavoidable.
- [ ] Edge cases and error paths are covered.

Cannot tick every box? You skipped TDD. Start over on the parts you skipped.

## When stuck

| Problem | What to do |
|---|---|
| Don't know how to test it | Write the API you wish you had. Write the assertion first. Still stuck: `ask_lead` with the behavior and two ways you could test it. |
| Test is too complicated | The design is too complicated. Simplify the interface. |
| Must mock everything | The code is too coupled. Use dependency injection. |
| Test setup is huge | Extract helpers. Still huge? Simplify the design. |

## Tests you must not touch

Some jobs come with checks written by the Tester. Those checks judge your output, so you never edit, weaken, skip or delete them. If you believe one is wrong, `ask_lead` with the test name, why you think it is wrong, and what you would change. Keep working on everything else while you wait.

## Debugging

Found a bug? Write a failing test that reproduces it, then follow the cycle. The test proves the fix and stops the bug from coming back. Never fix a bug without a test. For finding the cause, use the `systematic-debugging` skill.

## Final rule

```
Production code -> a test exists and failed first
Otherwise       -> not TDD
```

Then run the `verification-before-completion` gate before you call `report_status`.

---
Adapted from obra/superpowers skills/test-driven-development (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
