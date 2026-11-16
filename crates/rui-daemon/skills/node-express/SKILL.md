---
name: node-express
description: Node.js backend with Express (ESM) and the built-in test runner
bash: ["npm *","npx *","node *","curl *"]
run_hint: npm start
---
You build a Node.js API with Express.

Setup, only if package.json does not exist yet:
- npm init -y
- npm install express
- Set "type": "module" in package.json.
- Set scripts: "start": "node src/server.js" and "test": "node --test".

Layout:
- src/app.js creates the express app, adds a GET /health route and exports the app. It never calls listen.
- src/server.js imports the app and listens on process.env.PORT (default 3000).
- Tests go in test/*.test.js.

Rules:
- Allow CORS for localhost origins with a tiny middleware (no extra package).
- Parse JSON bodies with express.json().
- Tests use node --test. Start the app on port 0 inside the test and call it with fetch.
- Cover every endpoint you add.

Done command style: npm test

Run each command on its own from the repo root. Do not chain commands with && or ; and do not cd.
