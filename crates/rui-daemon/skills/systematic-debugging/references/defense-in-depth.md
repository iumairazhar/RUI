# Defense in Depth

When you fix a bug caused by bad data, one check in one place feels like enough. But another code path, a later refactor or a mock in a test can bypass that single check.

Core rule: validate at every layer the data passes through. Make the bug structurally impossible.

One check says "we fixed the bug". Several layers say "this bug can't happen".

## The four layers

### Layer 1: entry point

Reject obviously bad input at the API boundary.

```python
class CreateProject(BaseModel):
    name: str = Field(min_length=1, max_length=100)
    workdir: str = Field(min_length=1)
```

```typescript
if (!workdir || workdir.trim() === '') {
  return res.status(400).json({ error: 'workdir is required' });
}
```

### Layer 2: business logic

Make sure the data makes sense for this operation, even when it arrived through a path that skipped layer 1 (a background job, an internal call, a test).

```python
def init_workspace(project_dir: str) -> None:
    if not project_dir:
        raise ValueError("project_dir is required to initialize a workspace")
```

### Layer 3: environment guards

Refuse dangerous operations in the wrong context.

```python
def wipe_data_dir(path: Path) -> None:
    if os.environ.get("APP_ENV") == "test" and not path.resolve().is_relative_to(Path(tempfile.gettempdir()).resolve()):
        raise RuntimeError(f"refusing to wipe {path} outside the temp dir during tests")
```

### Layer 4: debug instrumentation

Capture context for the next investigation.

```python
logger.debug("about to wipe data dir", extra={"path": str(path), "cwd": os.getcwd()})
```

## How to apply it

1. Trace the data flow: where does the bad value start, and where is it used?
2. List every checkpoint the data passes through.
3. Add a check at each layer: entry, business logic, environment, logging.
4. Test each layer: try to bypass layer 1 and confirm layer 2 catches it. Each layer gets its own test.

## Why all the layers matter

In practice each layer catches cases the others miss:
- Other code paths skip entry validation.
- Mocks in tests skip business logic checks.
- Different machines or environments need the environment guard.
- Logging shows structural misuse that no check anticipated.

Do not stop at one check. Keep the extra layers small and focused, though: a one-line guard per layer, not a validation framework.

---
Adapted from obra/superpowers skills/systematic-debugging/defense-in-depth.md (MIT, Copyright (c) 2025 Jesse Vincent). Modified for RUI.
