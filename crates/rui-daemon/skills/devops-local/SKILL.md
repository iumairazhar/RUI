---
name: devops-local
description: Use whenever a job mentions Docker, docker compose, containers, CI, GitHub Actions, health checks, ports or "make it run anywhere". Covers a small non-root multi-stage Dockerfile, a local compose stack with health checks, a CI workflow that installs, lints, tests and builds, and the $PORT and logging rules. Use it even for a "quick Dockerfile", because the defaults here prevent the common mistakes. Never deploys anything.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: ["docker *", "docker compose *", "npm *", "uv *", "curl *", "git status*"]
---
# DevOps, Local Only

You make the app easy to build, run and check on any machine. You do this inside the workspace only.

## Hard rules

- **Never deploy.** No production, staging, cloud or "just a test" deploys.
- **Never push** images to a registry, never `docker login`, never `git push`.
- **Never commit secrets.** Commit `.env.example` with placeholder values. `.env` is in `.gitignore` and `.dockerignore`.
- **Anything outside the workspace goes to `ask_lead` first**: cloud accounts, DNS, paid services, other repos, system settings.
- **Do not edit the app's tests or checks** to make a pipeline pass. Report the failure.

## The app contract

Every service you containerise must:

1. Listen on `0.0.0.0` (not `localhost`) and on the port in `$PORT`, with a sensible default.
2. Expose `GET /health` that returns 200 and `{"status": "ok"}` quickly, without calling slow dependencies.
3. Log to stdout and stderr, never to files inside the container.
4. Read all config from environment variables.
5. Stop cleanly on SIGTERM.

If the app breaks any of these, fix it if it is in your scope, or tell the owning member through your status note.

## Dockerfile

Rules:

- **Multi-stage.** Build in one stage, copy only what runs into a slim final stage.
- **Small, pinned base.** `python:3.12-slim`, `node:22-alpine` or similar, with a version tag, never `latest`.
- **Non-root.** Create or use an unprivileged user and switch with `USER`.
- **Cache-friendly order.** Copy lockfiles and install dependencies before copying source.
- **Exec-form `CMD`** (`["node", "src/server.js"]`) so signals reach the app.
- **`HEALTHCHECK`** that hits `/health`.
- **`.dockerignore`** for `.git`, `node_modules`, `.venv`, `.env`, build output and test caches.

### Python (uv) template

```dockerfile
FROM python:3.12-slim AS build
COPY --from=ghcr.io/astral-sh/uv:0.5 /uv /bin/uv
WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project
COPY app ./app
RUN uv sync --frozen --no-dev

FROM python:3.12-slim
RUN useradd --create-home --uid 10001 app
WORKDIR /app
COPY --from=build --chown=app:app /app /app
ENV PATH="/app/.venv/bin:$PATH" PORT=8000 PYTHONUNBUFFERED=1
USER app
EXPOSE 8000
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD python -c "import os,urllib.request;urllib.request.urlopen(f'http://127.0.0.1:{os.environ[\"PORT\"]}/health')"
CMD ["sh", "-c", "exec uvicorn app.main:app --host 0.0.0.0 --port ${PORT}"]
```

### Node template

```dockerfile
FROM node:22-alpine AS deps
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --omit=dev

FROM node:22-alpine
WORKDIR /app
ENV NODE_ENV=production PORT=3000
COPY --from=deps /app/node_modules ./node_modules
COPY --chown=node:node . .
USER node
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD wget -qO- http://127.0.0.1:${PORT}/health || exit 1
CMD ["node", "src/server.js"]
```

For a Vite frontend, build in a Node stage and serve `dist/` from a small static server stage (for example `nginx:alpine` with a `listen` on the port you choose).

### .dockerignore

```
.git
.env
.venv
node_modules
dist
__pycache__
.pytest_cache
coverage
```

## docker compose for local dev

Rules:

- One file, `compose.yaml`, at the repo root.
- Each service has a `healthcheck`. Dependants use `depends_on: {db: {condition: service_healthy}}`.
- Config comes from `.env` via `env_file`. Commit `.env.example`, never `.env`.
- Data lives in a named volume, so `docker compose down` keeps it and `down -v` clears it.
- Bind ports to `127.0.0.1` so local services are not exposed on the network.

```yaml
services:
  api:
    build: .
    env_file: .env
    environment:
      PORT: "8000"
      DATABASE_URL: postgresql://app:app@db:5432/app
    ports: ["127.0.0.1:8000:8000"]
    depends_on:
      db: {condition: service_healthy}
  db:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: app
      POSTGRES_PASSWORD: app
      POSTGRES_DB: app
    volumes: [db-data:/var/lib/postgresql/data]
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U app"]
      interval: 5s
      timeout: 3s
      retries: 10
volumes:
  db-data:
```

The local password `app` is fine for a dev-only database bound to 127.0.0.1. Never reuse it anywhere else.

## GitHub Actions CI

Rules:

- Trigger on `push` and `pull_request`.
- Steps: checkout, set up the toolchain with caching, install from the lockfile, lint, test, build.
- Pin action versions (`@v4`). For third-party actions, prefer a full commit SHA.
- Set `permissions: contents: read`.
- **No deploy, publish, release or registry steps.** No secrets needed.

```yaml
name: ci
on: [push, pull_request]
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
          cache: npm
      - run: npm ci
      - run: npm run lint --if-present
      - run: npm test
      - run: npm run build --if-present
```

Python variant: use `astral-sh/setup-uv` (pinned) with `enable-cache: true`, then `uv sync --frozen`, `uv run ruff check .` if ruff is a dev dependency, and `uv run pytest -q`.

You cannot run GitHub Actions locally. Check the YAML by reading it carefully, and make sure each `run` line works on its own in this workspace.

## How to verify

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.

1. `docker build -t app-local .` and read the output to the end.
2. `docker compose up -d --build`
3. `docker compose ps` and wait until every service shows `healthy`.
4. `curl -fsS http://127.0.0.1:8000/health`
5. `docker compose logs --tail 50 api` and check there are no errors.
6. `docker compose down`

If Docker is not installed or the daemon is not running, do not try to install it. Report `BLOCKED` with the exact error, or `DONE_WITH_CONCERNS` if you could verify everything else.

## Pitfalls

| Mistake | Fix |
|---|---|
| App binds `127.0.0.1` inside the container | Bind `0.0.0.0` |
| Port hardcoded in code | Read `$PORT` with a default |
| `CMD npm start` in shell form | Exec form, so SIGTERM reaches the app |
| `COPY . .` before installing deps | Copy lockfiles first for layer caching |
| `.env` copied into the image | List it in `.dockerignore` |
| `depends_on` without a health condition | Use `condition: service_healthy` |
| Health check calls the database | Keep `/health` cheap; add `/ready` if you need dependency checks |
| `latest` tags | Pin a version |

Done command: `docker compose ps` showing all services healthy, plus `curl -fsS http://127.0.0.1:<port>/health`.
