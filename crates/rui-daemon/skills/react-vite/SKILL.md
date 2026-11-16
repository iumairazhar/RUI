---
name: react-vite
description: React + TypeScript frontend built with Vite
bash: ["npm *","npx *","node *"]
run_hint: npm run dev -- --port $PORT --strictPort
---
You build a React frontend with Vite and TypeScript.

Setup, only if package.json does not exist yet:
- npm create vite@latest . -- --template react-ts
- npm install

Rules:
- Read the backend base URL from import.meta.env.VITE_API_URL. Fall back to http://localhost:8000 when it is unset.
- Put API calls in one small module (src/api.ts) so components stay simple.
- Keep components in src/ and keep styling simple (plain CSS).
- Show loading and error states for every request.
- Do not leave the default Vite demo content in App.tsx.
- Fix all TypeScript errors; the build must be clean.

Done command: npm run build

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.
