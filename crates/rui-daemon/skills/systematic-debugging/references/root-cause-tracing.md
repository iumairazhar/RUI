# Root Cause Tracing

Bugs often show up deep in the call stack: a file written to the wrong folder, a database opened with the wrong path, a request sent to the wrong URL. Your instinct is to fix the line where the error appears. That treats the symptom.

Core rule: trace backward through the call chain until you find the original trigger, then fix it there.

## When to use it

- The error happens deep in execution, not at the entry point.
- The stack trace shows a long call chain.
- It is unclear where the bad data came from.
- You need to find which test or code path triggers the problem.

If you can trace back, trace to the original trigger, then also add defense in depth (see `defense-in-depth.md`). Only if you truly hit a dead end do you fix at the symptom point, and you say so in your status note.

## The process

1. **Observe the symptom.**
   ```
   sqlite3.OperationalError: unable to open database file
   ```
2. **Find the immediate cause.** What code directly does this?
   ```python
   engine = create_engine(f"sqlite:///{db_path}")
   ```
3. **Ask what called it.**
   ```
   get_engine(db_path)
     <- called by init_db(settings.data_dir)
     <- called by app startup
     <- called by the test client fixture
   ```
4. **Keep tracing up. What value was passed?** `settings.data_dir` is `""`. An empty data dir makes the path `/app.db` at the filesystem root, which is not writable.
5. **Find the original trigger.** Where did the empty string come from? The test fixture builds `Settings()` before it sets the `DATA_DIR` environment variable, so the default `""` is used.

Root cause: the fixture order. Fix: set the env var before building settings, and make `Settings` reject an empty `data_dir` at load time.

## Adding instrumentation

When you can't trace by reading, instrument the dangerous operation:

```python
import sys, traceback, os

def get_engine(db_path):
    print("DEBUG get_engine", {"db_path": db_path, "cwd": os.getcwd()}, file=sys.stderr)
    traceback.print_stack(file=sys.stderr)
    ...
```

```typescript
console.error('DEBUG openDb', { dbPath, cwd: process.cwd(), stack: new Error().stack });
```

Tips:
- In tests, print to stderr (`console.error`, `print(..., file=sys.stderr)`, or `pytest -s`). A logger may be silenced.
- Log *before* the dangerous operation, not after it fails.
- Include context: paths, cwd, relevant env vars.
- Capture the stack: it shows the whole call chain.

Then run the test and read the output. Look for test file names, the line that triggers the call, and patterns (always the same test, always the same parameter?).

Remove the instrumentation before you report, unless the Lead asked for it to stay.

## Finding which test causes pollution

If something appears during the test run (a stray file, leftover state) and you don't know which test causes it, run the test files one at a time and check after each one. Stop at the first file that leaves the pollution behind, then bisect inside that file.

## The key rule

```
found immediate cause
  -> can you trace one level up?  no  -> dead end: fix at the symptom, say so in the report
  -> yes: trace back
  -> is this the source?  no -> keep tracing
  -> yes: fix at the source
  -> add validation at each layer (defense in depth)
  -> the bug is now impossible
```

Never fix only where the error appears.

---
Adapted from obra/superpowers skills/systematic-debugging/root-cause-tracing.md (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
