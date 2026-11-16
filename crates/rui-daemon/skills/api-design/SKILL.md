---
name: api-design
description: Use whenever you design, add or change an HTTP/JSON endpoint, before writing the handler. Covers naming, response shapes, pagination, status codes, idempotency, versioning and error messages that tell the caller how to fix the request, plus task-style evals that prove the API is usable end to end.
license: Apache-2.0
metadata: {source: "https://github.com/anthropics/skills/blob/main/skills/mcp-builder/SKILL.md", adapted: true}
bash: ["curl *", "uv *", "python3 *", "npm *", "npx *", "node *"]
---

# API Design

An API is good when a caller with no other context can finish real tasks with it. That caller is often another agent: a frontend member reading only your OpenAPI schema, or a tester writing checks from the brief. Design for them.

## 1. Before writing code

Write the contract first, in the job notes or as the framework's schema (Pydantic models, zod, TypeScript types):

- The resources and their fields, with types and which are required.
- Each endpoint: method, path, request body, success response, error responses.
- Which operations are read-only, which change state, and which are destructive.

If the Architect or the job already defined the contract, follow it exactly. If you must change it, `ask_lead` first: other members are building against it.

## 2. Naming

- Resources are plural nouns: `/tasks`, `/tasks/{id}`, `/tasks/{id}/comments`. No verbs in paths.
- Actions that don't fit CRUD become sub-resources with POST: `POST /tasks/{id}/archive`.
- One case style for JSON fields across the whole API. Match the project (`snake_case` for Python, `camelCase` for Node unless it already uses snake). Never mix.
- IDs are strings in JSON even if numeric in the DB, so clients never lose precision.
- Timestamps are ISO 8601 in UTC with a `Z`: `"2026-11-16T21:00:00Z"`.
- Names say what things are for the caller, not how they are stored.

## 3. Consistent shapes

- A single resource returns the object itself. Don't wrap it in `{"data": ...}` unless the whole API does.
- Lists always return an envelope so pagination can be added without breaking clients:

```json
{"items": [...], "total": 150, "limit": 20, "offset": 0, "has_more": true, "next_offset": 20}
```

- Create returns `201` with the created object (including its `id`) and a `Location` header.
- Update returns the full updated object, so the client never has to refetch.
- The same field has the same name and type everywhere it appears.

## 4. Pagination and filtering

- Every list endpoint takes `limit` and `offset` (or `cursor` for feeds that change fast). Default `limit` 20–50, enforce a max (100), and reject larger values with a guiding error.
- Never load a whole table to return a page.
- Filters are query params named after fields: `?status=open&assignee=7`. Sorting: `?sort=created_at` / `?sort=-created_at`.
- Return `has_more` and the next offset/cursor. Clients must never guess.

## 5. Status codes

| Situation | Code |
|---|---|
| Read OK / update OK | 200 |
| Created | 201 |
| Deleted, nothing to return | 204 |
| Malformed body or bad param value | 400 (or 422 if the framework uses it for validation; be consistent) |
| Not logged in / bad token | 401 |
| Logged in but not allowed | 403 |
| Resource doesn't exist | 404 |
| Conflicts with current state (duplicate, version mismatch) | 409 |
| Rate limited | 429 with `Retry-After` |
| Our bug | 500, never with a stack trace in the body |

Never return 200 with an error inside the body.

## 6. Error messages that guide

An error must answer three things: what was wrong, which input caused it, and what to do instead. The caller should be able to fix the request from the message alone.

One error shape for the whole API:

```json
{"error": {"code": "limit_too_large", "message": "limit must be between 1 and 100; you sent 500. Use offset to page through more results.", "field": "limit"}}
```

- `code` is a stable machine string. `message` is a plain sentence for whoever reads it.
- Bad: `"Invalid input"`, `"Validation error"`, `"Something went wrong"`.
- Good: `"status must be one of: open, done, archived. Got 'closed'."`
- Good: `"Task 42 not found. List tasks with GET /tasks to see valid ids."`
- Validation errors list every bad field at once, not just the first.
- Don't leak internals: no stack traces, SQL, file paths or library names in responses. Log those server-side.

## 7. Idempotency and safety

- GET, PUT and DELETE are idempotent: calling twice has the same effect as once. Deleting something already gone returns 404 or 204 consistently; pick one and document it.
- POST that creates or charges something should accept an `Idempotency-Key` header when retries are likely (payments, external side effects). Same key, same result.
- Destructive endpoints never hide behind GET.
- Validate every input with the framework's schema layer. Check sizes and ranges. Never build SQL or shell commands from strings.
- Secrets come from environment variables, never from code or responses.
- CORS: allow only the origins the job needs (localhost during development).

## 8. Versioning and change

- Additive changes (new optional field, new endpoint) are safe.
- Removing or renaming a field, changing a type, or making an optional field required breaks clients. Don't do it inside a job without `ask_lead`. If needed, add the new field alongside the old one, or add `/v2/...`.
- Keep the OpenAPI/schema output accurate. It is the contract other members read.

## 9. Evals: prove a stranger can use it

Unit tests prove handlers work. Evals prove the API is usable for real tasks. After the endpoints pass their tests, write 5–10 task-style checks in `tests/evals/` (or `test/evals/`).

Each eval:
- Is a **realistic task** a client would do, phrased as a goal: "Create a project, add three tasks, complete one, and get the count of open tasks."
- Needs **several calls** in sequence, using only the documented API (no DB access, no imports from app code).
- Has **one verifiable answer** decided in advance and compared exactly (a count, a name, a status).
- Is **independent**: sets up its own data, depends on no other eval, leaves no shared state.
- Includes at least **two error-path tasks**: send a bad request, read the error, and confirm the message alone tells you how to fix it (then fix it and succeed).
- Includes at least **one pagination task** that must page past the first page to get the answer.

Example in Python with `httpx`/TestClient:

```python
def test_eval_complete_one_of_three(client):
    p = client.post("/projects", json={"name": "Launch"}).json()
    ids = [client.post(f"/projects/{p['id']}/tasks", json={"title": t}).json()["id"] for t in "abc"]
    client.post(f"/tasks/{ids[0]}/complete")
    open_tasks = client.get(f"/projects/{p['id']}/tasks", params={"status": "open"}).json()
    assert open_tasks["total"] == 2
```

When an eval is awkward to write, that is a design finding. Fix the API (a missing filter, a confusing name, an unhelpful error), not the eval.

## 10. Done and reporting

1. Run the done command (e.g. `uv run pytest -q` or `npm test`) including the evals. Read the output.
2. Hit one endpoint for real with `curl -s localhost:$PORT/...` if the server is running.
3. `report_status`:
   - `DONE`: endpoints added (method + path), eval count passing, any contract decisions you made.
   - `DONE_WITH_CONCERNS`: a contract call others may need to know about, or a breaking change you avoided with a workaround.
   - `NEEDS_CONTEXT`: the contract you were told to follow is missing or contradicts the job.
   - `BLOCKED`: something outside your files stops the tests.

---
Adapted from ideas in anthropics/skills skills/mcp-builder (SKILL.md, reference/mcp_best_practices.md, reference/evaluation.md; Apache-2.0, Copyright Anthropic, PBC). This file was modified by RUI: rewritten for REST/JSON APIs instead of MCP servers, with RUI conventions and status reporting.
