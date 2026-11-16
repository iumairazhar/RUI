---
name: fastapi
description: Use whenever the job builds or changes a Python API, mentions FastAPI, uvicorn, pydantic or pytest in a Python backend. Covers layout, Pydantic v2 models, routers, dependencies, lifespan, clear HTTP errors, CORS for localhost and TestClient tests. Use it even for a one-endpoint change, so the project stays consistent.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: ["uv *", "python3 *", "curl *"]
run_hint: uv run uvicorn app.main:app --port $PORT
---
# FastAPI Backend

You build a Python API with FastAPI, managed by uv, tested with pytest.

## Setup (only if pyproject.toml does not exist yet)

- `uv init --app`
- `uv add fastapi uvicorn`
- `uv add --dev pytest httpx`

Do not add other packages unless the job needs them. Prefer the standard library.

## Layout

```
app/
  __init__.py
  main.py          # create the app, lifespan, middleware, include routers, GET /health
  models.py        # Pydantic request/response models
  deps.py          # dependencies (db session, settings)
  routers/
    __init__.py
    notes.py       # one APIRouter per resource
tests/
  __init__.py      # makes tests a package so `import app` works
  conftest.py      # TestClient fixture
  test_notes.py
```

- `app/main.py` holds the FastAPI app and a `GET /health` route returning `{"status": "ok"}`.
- Other modules go in `app/`. Tests go in `tests/`.
- Add `tests/__init__.py` so imports of `app` work.

## Rules

**Server and config**

- Read the port from the `PORT` environment variable when you start the server (`--port $PORT`).
- Read other config from environment variables with defaults. Never hardcode secrets.
- Add CORS middleware that allows `http://localhost` and `http://127.0.0.1` on any port:
  `allow_origin_regex=r"https?://(localhost|127\.0\.0\.1)(:\d+)?"`.
- Keep data in memory or SQLite unless the job says otherwise.

**Models (Pydantic v2)**

- Separate input and output models: `NoteCreate` (what the client sends) and `Note` (what you return).
- Validate at the edge with `Field(min_length=1, max_length=...)` and types, not with `if` checks in handlers.
- Use `model_config = ConfigDict(from_attributes=True)` when returning ORM objects.
- Use `model_dump()` and `model_validate()`, not the v1 `.dict()` / `.parse_obj()`.

**Routes**

- One `APIRouter(prefix="/notes", tags=["notes"])` per resource, included in `main.py`.
- Set `response_model` and `status_code` on each route (`201` for create, `204` for delete).
- Missing resources raise `HTTPException(status_code=404, detail="Note 42 not found")`. The detail says what was missing and which id.
- Shared things (db session, settings, current user) come through `Depends(...)`, so tests can override them with `app.dependency_overrides`.
- Use plain `def` handlers for blocking work (SQLite, files). Use `async def` only when everything inside awaits.

**Startup and shutdown**

- Use a `lifespan` async context manager for setup (create tables, open pools). Do not use `@app.on_event`.

```python
from contextlib import asynccontextmanager
from fastapi import FastAPI

@asynccontextmanager
async def lifespan(app: FastAPI):
    init_db()
    yield

app = FastAPI(lifespan=lifespan)
```

**Tests**

- Write tests with `fastapi.testclient.TestClient`. Cover every endpoint you add.
- For each endpoint: the happy path, a validation error (422), and not-found (404) where it applies.
- Use `with TestClient(app) as client:` in a fixture so the lifespan runs.
- Give each test fresh state (a temp SQLite path via `tmp_path`, or a dependency override). Tests must pass in any order.
- Assert on status code and on the body fields that matter.

```python
import pytest
from fastapi.testclient import TestClient
from app.main import app

@pytest.fixture
def client():
    with TestClient(app) as c:
        yield c

def test_create_note(client):
    r = client.post("/notes", json={"text": "hi"})
    assert r.status_code == 201
    assert r.json()["text"] == "hi"
```

## Pitfalls

| Mistake | Fix |
|---|---|
| `uvicorn` bound to a fixed port | Use `$PORT` |
| Missing `tests/__init__.py` → `ModuleNotFoundError: app` | Add it |
| `async def` handler calling blocking sqlite | Use `def` |
| Mutable global list shared across tests | Reset state per test |
| Returning the input model as output (leaks fields, no id) | Separate output model with `response_model` |
| `detail="error"` | Say what failed and why |
| `@app.on_event("startup")` | Use `lifespan` |
| `allow_origins=["*"]` | Use the localhost regex above |

## Verify

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.

- `uv run pytest -q`
- Optional smoke test: start with the run hint, then `curl -s http://127.0.0.1:$PORT/health`.

Done command style: `uv run pytest -q`
