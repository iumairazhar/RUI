---
name: fastapi
description: Python backend with FastAPI, uv and pytest
bash: ["uv *", "python3 *", "curl *"]
run_hint: uv run uvicorn app.main:app --port $PORT
---
You build a Python API with FastAPI.

Layout:
- app/main.py holds the FastAPI app and a GET /health route returning {"status": "ok"}.
- Other modules go in app/ (models, routers). Tests go in tests/.

Setup, only if pyproject.toml does not exist yet:
- uv init --app
- uv add fastapi uvicorn
- uv add --dev pytest httpx

Rules:
- Read the port from the PORT environment variable when you start the server.
- Add CORS middleware that allows http://localhost and http://127.0.0.1 on any port (allow_origin_regex).
- Keep data in memory or SQLite unless the job says otherwise.
- Write tests with fastapi.testclient.TestClient. Cover every endpoint you add.
- Add the tests dir as a package (tests/__init__.py) so imports of app work.

Done command style: uv run pytest -q

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.
