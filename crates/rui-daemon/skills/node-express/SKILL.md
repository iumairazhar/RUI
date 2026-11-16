---
name: node-express
description: Use whenever the job builds or changes a Node.js API, mentions Express, routes, middleware or node --test. Covers ESM layout, router modules, input validation, a central JSON error handler, localhost CORS and tests that start the app on port 0. Use it even for a single new route, so errors and tests stay consistent.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: ["npm *","npx *","node *","curl *"]
run_hint: npm start
---
# Node + Express Backend

You build a Node.js API with Express (ESM) and the built-in test runner.

## Setup (only if package.json does not exist yet)

- `npm init -y`
- `npm install express`
- Set `"type": "module"` in package.json.
- Set scripts: `"start": "node src/server.js"` and `"test": "node --test"`.

`npm install express` installs Express 5. It forwards errors thrown in `async` handlers to the error middleware by itself. Do not add `express-async-handler` or similar.

## Layout

```
src/
  app.js            # creates the app, middleware, routers, /health, error handler; exports app; never listens
  server.js         # imports app, listens on process.env.PORT (default 3000)
  routes/
    notes.js        # one express.Router() per resource
  errors.js         # HttpError class
  store.js          # data access (in memory or SQLite)
test/
  notes.test.js
```

- `src/app.js` creates the express app, adds a `GET /health` route returning `{ "status": "ok" }`, and exports the app. It never calls `listen`.
- `src/server.js` imports the app and listens on `process.env.PORT` (default 3000).
- Tests go in `test/*.test.js`.

## Rules

**App setup**

- Allow CORS for localhost origins with a tiny middleware (no extra package):

```js
const LOCAL = /^https?:\/\/(localhost|127\.0\.0\.1)(:\d+)?$/;
app.use((req, res, next) => {
  const origin = req.headers.origin;
  if (origin && LOCAL.test(origin)) {
    res.set('Access-Control-Allow-Origin', origin);
    res.set('Access-Control-Allow-Headers', 'Content-Type');
    res.set('Access-Control-Allow-Methods', 'GET,POST,PUT,PATCH,DELETE');
  }
  if (req.method === 'OPTIONS') return res.sendStatus(204);
  next();
});
```

- Parse JSON bodies with `express.json()`.
- Read config from `process.env` with defaults. Never hardcode secrets.

**Routes**

- One `express.Router()` per resource in `src/routes/`, mounted with `app.use('/notes', notesRouter)`.
- Validate input at the top of each handler. On bad input, throw `new HttpError(400, 'text must be a non-empty string')`. Say which field and why.
- Use correct status codes: `201` + body for create, `204` for delete, `404` with a message naming the missing id.
- Keep handlers thin. Data access goes in `store.js`.

**Errors**

- One error class and one error handler, registered last:

```js
// src/errors.js
export class HttpError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

// end of src/app.js
app.use((req, res) => res.status(404).json({ error: `No route for ${req.method} ${req.path}` }));
app.use((err, req, res, next) => {
  const status = err.status ?? 500;
  if (status >= 500) console.error(err);
  res.status(status).json({ error: status >= 500 ? 'Internal server error' : err.message });
});
```

- All errors are JSON `{ "error": "..." }`. Never send HTML error pages or stack traces to the client.
- Malformed JSON bodies from `express.json()` arrive with `err.status = 400`; the handler above covers them.

**Tests**

- Tests use `node --test`. Start the app on port 0 inside the test and call it with `fetch`.
- Cover every endpoint you add: happy path, bad input (400), not found (404).
- Reset the store between tests so they pass in any order.

```js
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { app } from '../src/app.js';

let server, base;
before(() => new Promise((ok) => {
  server = app.listen(0, () => { base = `http://127.0.0.1:${server.address().port}`; ok(); });
}));
after(() => new Promise((ok) => server.close(ok)));

test('POST /notes creates a note', async () => {
  const r = await fetch(`${base}/notes`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text: 'hi' }),
  });
  assert.equal(r.status, 201);
  assert.equal((await r.json()).text, 'hi');
});
```

## Pitfalls

| Mistake | Fix |
|---|---|
| `app.listen` inside app.js | Only server.js listens; tests use port 0 |
| `require(...)` in an ESM project | Use `import`, include the `.js` extension in relative paths |
| Error handler registered before routes | Register it last, with 4 arguments |
| `res.send(err)` | `res.status(n).json({ error })` |
| Hardcoded port | `process.env.PORT ?? 3000` |
| Tests share state | Reset the store in `beforeEach` |
| Adding `cors`, `body-parser`, `nodemon` | Not needed; built-ins cover it |

## Verify

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.

- `npm test`
- Optional smoke test: `npm start`, then `curl -s http://127.0.0.1:$PORT/health`.

Done command style: `npm test`
