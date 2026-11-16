---
name: react-vite
description: Use whenever the job builds or changes a web frontend, mentions React, Vite, TypeScript, components or pages. Covers setup, strict TypeScript, one API module, loading/error/empty states, accessible markup, env config and a clean build. Use it even for a small UI tweak, so the build stays green and states stay handled.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: ["npm *","npx *","node *"]
run_hint: npm run dev -- --port $PORT --strictPort
---
# React + Vite + TypeScript Frontend

You build a React frontend with Vite and TypeScript.

For visual direction, pair this with `frontend-design`. This skill covers structure and correctness.

## Setup (only if package.json does not exist yet)

- `npm create vite@latest . -- --template react-ts`
- `npm install`

## Layout

```
src/
  main.tsx        # mounts <App />
  App.tsx         # top-level layout and routing between views
  api.ts          # every backend call lives here
  types.ts        # shared types for API data
  components/     # one component per file, PascalCase names
  App.css         # plain CSS
```

## Rules

**Config**

- Read the backend base URL from `import.meta.env.VITE_API_URL`. Fall back to `http://localhost:8000` when it is unset.
- Only variables prefixed `VITE_` reach the browser. Never put secrets in them.
- Declare custom env vars in `src/vite-env.d.ts` so TypeScript knows them.

**API module**

- Put API calls in one small module (`src/api.ts`) so components stay simple.
- Each function is typed: `listNotes(): Promise<Note[]>`.
- Check `res.ok`. On failure, throw an `Error` with the server's message when there is one.

```ts
const BASE = import.meta.env.VITE_API_URL ?? 'http://localhost:8000';

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    headers: { 'Content-Type': 'application/json' }, ...init,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => null);
    throw new Error(body?.detail ?? body?.error ?? `Request failed (${res.status})`);
  }
  return res.status === 204 ? (undefined as T) : res.json();
}

export const listNotes = () => request<Note[]>('/notes');
```

**Components and state**

- Keep components in `src/` and keep styling simple (plain CSS).
- Show loading and error states for every request. Also show an empty state for lists.
- Model request state explicitly, not with three loose booleans:
  `type State<T> = { status: 'loading' } | { status: 'error'; message: string } | { status: 'ready'; data: T }`.
- Cancel or ignore stale responses in `useEffect` (an `AbortController` or an `ignore` flag in the cleanup).
- Disable submit buttons while a request is in flight, and refresh or update the list after a change.
- Do not leave the default Vite demo content in `App.tsx`, and remove unused demo assets.

**TypeScript**

- Keep `strict: true`. Do not use `any`; use `unknown` and narrow it.
- No `// @ts-ignore` to silence errors. Fix the type.
- Fix all TypeScript errors; the build must be clean.

**Accessibility**

- Use real elements: `<button>` for actions, `<a>` for navigation, `<form onSubmit>` for forms.
- Every input has a `<label>`. Images have `alt`.
- Error messages use `role="alert"` so screen readers announce them.
- Everything works with the keyboard, and focus is visible.

**Tests (only when the job asks for them)**

- `npm install -D vitest @testing-library/react @testing-library/jest-dom jsdom`
- Set `test: { environment: 'jsdom' }` in `vite.config.ts` and add `"test": "vitest run"` to scripts.
- Query by role and label (`getByRole('button', { name: /save/i })`), not by class names.
- Mock `fetch` or the `api.ts` module; do not call a real backend from unit tests.

## Pitfalls

| Mistake | Fix |
|---|---|
| `fetch` calls scattered in components | Move them into `api.ts` |
| No error state, so a failed request shows a blank page | Render the error with `role="alert"` |
| `process.env.X` | `import.meta.env.VITE_X` |
| Port fixed in vite.config | Use the run hint's `--port $PORT --strictPort` |
| `<div onClick>` as a button | Use `<button>` |
| Leftover Vite logo and counter | Delete them |
| Build passes in dev but `tsc` fails | Run `npm run build`, which type-checks |

## Verify

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.

- `npm run build`
- `npm test` if tests exist.

Done command: `npm run build`
