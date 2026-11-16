# Writing Good Tests

Read this when you write or change tests, add mocks, or add cleanup or helper methods for tests.

A test exists to catch a specific break. Two principles cover everything here:

```
1. Every test names the break it catches.
2. Every test exercises the real thing.
```

Strict TDD gives you both. A test written first and watched failing against real code has already proven it can fail. It only earns a mock when the real dependency turns out to be slow or external.

## Principle 1: name the break

Before writing the test body, answer this: what production change should make this test fail, and is that change a bug or a decision? A test earns its place by catching a wrong branch, a missing side effect, a wrong argument, a boundary case or a broken contract.

**Derive expectations independently.** Use literals and hand-checked fixtures. Table-driven tests with literal expected values are the preferred shape. An expected value computed by the code under test, or by its helpers, passes no matter what that code does.

```python
# Bad: the same builder computes both sides, so it is always true
expected = build_search_query(tag="urgent")
assert build_search_query(tag="urgent") == expected

# Good: a hand-derived literal
assert build_search_query(tag="urgent") == 'tag:"urgent"'
```

**No change detectors.** If only intentional decisions can fail a test (a constant's value, exact message wording, private structure), it fires on every redesign and sleeps through real bugs. Test the behavior that depends on the decision. Not `assert MAX_RETRIES == 5`, but "a failing call is retried 5 times and a 6th attempt never happens".

**Behavior, not text.** A test that checks a script or config contains an exact line only proves the source is the source. Run the script against controlled inputs and assert its outputs, side effects or exit code.

**Your code, not the framework.** Test the contract your code makes at its edges: the route you register, the query you emit, the payload you return. Do not test that FastAPI calls your handler or that Express parses JSON; that is the framework's job. When the framework genuinely surprised you, write one narrow test that names the assumption. The same rule applies inside your code: constructors, getters, constants and plain forwarding only earn tests when they validate, normalize, default, derive, enforce or cause side effects.

### Gate

```
BEFORE writing the test body:
  Name the production change that would make this test fail.

  Can't name one              -> redesign around an observable behavior
  "The source text changed"   -> run the artifact and assert its effects
  Only intentional decisions  -> change detector; test the behavior that depends on it

  Check the expected value is derived without the code under test.
  If it reuses the code's logic or helpers -> replace it with a literal or fixture.
```

## Principle 2: exercise the real thing

**The mock earns no assertions.** An assertion on a mock passes when the mock is there and fails when it isn't. It says nothing about your component. Assert the real behavior. If the mock is the thing you are checking, unmock it or delete the assertion.

```typescript
// Good: real behavior
expect(screen.getByRole('navigation')).toBeInTheDocument();

// Bad: mock existence
expect(screen.getByTestId('sidebar-mock')).toBeInTheDocument();
```

Ask yourself: "Am I testing the behavior of a mock?"

**Mock at the right level.** Learn every side effect of the real method before you replace it. Mock the slow or external part, and keep real whatever your test depends on. When unsure, run the test against the real code first and see what actually has to happen.

**Make doubles specific.** When arguments, call counts or ordering are part of the contract, assert them. A fake that accepts anything verifies nothing. Give each branch (success, error, malformed) its own fixture, so the wrong branch cannot satisfy the test.

**Mirror real data completely.** A mocked response must have the full real structure, every documented field, not just the ones your test reads. Partial mocks fail silently when code downstream reads a missing field: the test passes while the real integration breaks.

**Production classes carry production methods only.** Cleanup that only tests need lives in test utilities (fixtures, `conftest.py`, test helpers), never as a `destroy()` method on a production class.

**Prefer real components over complex mocks.** When mock setup outgrows the test logic, switch to an integration test with real parts. For HTTP APIs, prefer FastAPI's `TestClient` or starting the Express app on port 0 and calling it with `fetch`, over mocking the request layer. For data, prefer an in-memory SQLite database over a mocked repository.

### Gate

```
BEFORE adding a mock or test helper:
  List the real method's side effects. Keep the ones the test depends on real;
  mock the slow or external level below them.
  Mock responses mirror the complete real structure.
  A method only tests call lives in test utilities, not production code.
  About to assert on the mock itself? Unmock it or delete the assertion.
```

## Tests ship with the implementation

Failing test, minimal code, refactor: that cycle is what "complete" means. Ship the tests the behavior needs and only those. Trivial code earns none, and a test written only to satisfy process costs maintenance forever.

## The mutation check

Before finishing, mentally mutate the production code. At least one test should fail for each realistic mutation:

- Wrong constant or argument
- Wrong branch handler
- Missing state change or side effect
- Empty or default return value
- Missing validation for zero, empty, null, unauthorized or malformed input

A mutation nothing catches marks behavior that is unprotected, or a test that is a tautology.

## Quick reference

| When you... | Do |
|---|---|
| Write any test | Name the break it catches: a bug, not a decision |
| Build an expected value | Derive it by hand, never with the code under test |
| Test a script or config | Run it; never grep its text |
| Reach for a framework test | Test your boundary contract, not their mechanics |
| Want to assert on a mocked element | Test the real component, or unmock it |
| Are about to mock a method | Learn its side effects; mock the slow or external level |
| Build a mock response | Mirror the real structure completely |
| Need cleanup only tests use | Put it in test utilities |
| See mock setup balloon | Switch to an integration test with real components |
| Finish a test file | Run the mutation check |

## Warning signs

- Setup and assertion share the same object, so equality is guaranteed.
- The test can only fail through a crash or a missing selector.
- The test fails on every intentional change but never on an accidental break.
- Expected values hide behind loops, builders or helpers.
- The test greps source text.
- The test would still make sense if only the framework remained.
- The test exists for coverage and checks no outcome.
- An assertion checks a `*-mock` test id.
- A method is called only from test files.
- Mock setup is more than half the test, or you can't say why the mock is needed.

---
Adapted from obra/superpowers skills/test-driven-development/writing-good-tests.md (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
