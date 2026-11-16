# Condition-Based Waiting

Flaky tests often guess at timing with fixed delays. That creates race conditions: the test passes on a fast machine and fails under load or in CI.

Core rule: wait for the condition you actually care about, not for a guess about how long it takes.

## When to use it

- A test has fixed delays (`setTimeout`, `sleep`, `time.sleep()`, `page.wait_for_timeout()`).
- A test is flaky: it passes sometimes and fails under load.
- Tests time out when run in parallel.
- You wait for an async operation, a server to start or a file to appear.

Don't use it when you are testing timing itself (debounce, throttle, intervals). Even then, explain in a comment why the delay is needed.

## Core pattern

```typescript
// Before: guessing
await new Promise(r => setTimeout(r, 50));
expect(getResult()).toBeDefined();

// After: waiting for the condition
await waitFor(() => getResult() !== undefined, 'result to be set');
expect(getResult()).toBeDefined();
```

## Quick patterns

| Scenario | Pattern |
|---|---|
| Wait for an event | `waitFor(() => events.find(e => e.type === 'DONE'))` |
| Wait for a state | `waitFor(() => machine.state === 'ready')` |
| Wait for a count | `waitFor(() => items.length >= 5)` |
| Wait for a file | `waitFor(() => fs.existsSync(path))` |
| Wait for a server | poll `GET /health` until it returns 200 |
| Browser element | Playwright `expect(locator).toBeVisible()` or `page.wait_for_selector()` |

## Implementation

TypeScript:
```typescript
async function waitFor<T>(
  condition: () => T | undefined | null | false,
  description: string,
  timeoutMs = 5000,
): Promise<T> {
  const start = Date.now();
  while (true) {
    const result = condition();
    if (result) return result;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`Timeout waiting for ${description} after ${timeoutMs}ms`);
    }
    await new Promise(r => setTimeout(r, 10));
  }
}
```

Python:
```python
import time

def wait_for(condition, description, timeout=5.0, interval=0.01):
    start = time.monotonic()
    while True:
        result = condition()
        if result:
            return result
        if time.monotonic() - start > timeout:
            raise TimeoutError(f"Timeout waiting for {description} after {timeout}s")
        time.sleep(interval)
```

## Common mistakes

- **Polling too fast** (every 1 ms) wastes CPU. Poll about every 10 ms; for a server health check, every 100 to 250 ms.
- **No timeout** loops forever if the condition never comes true. Always set a timeout with a clear error message.
- **Stale data**: reading state once before the loop. Call the getter inside the loop.

## When a fixed delay is correct

```typescript
// The tool ticks every 100ms; we need 2 ticks to see partial output.
await waitForEvent(manager, 'TOOL_STARTED');   // first: wait for the trigger
await new Promise(r => setTimeout(r, 200));     // then: 2 ticks at 100ms, by design
```

Requirements:
1. First wait for the triggering condition.
2. Base the delay on known timing, not a guess.
3. Add a comment explaining why.

---
Adapted from obra/superpowers skills/systematic-debugging/condition-based-waiting.md (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
