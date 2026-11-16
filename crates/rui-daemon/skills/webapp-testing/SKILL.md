---
name: webapp-testing
description: Use whenever you need to check a web app in a real browser: verifying a UI change works, reproducing a frontend bug, taking screenshots, reading console errors, or driving an end-to-end flow. Use it before claiming any UI work is done, even if the build passes.
license: Apache-2.0
metadata: {source: "https://github.com/anthropics/skills/blob/main/skills/webapp-testing/SKILL.md", adapted: true}
bash: ["python3 *", "uv *", "npx *", "npm *", "curl *"]
---

# Web App Testing

Test local web apps by writing small Python Playwright scripts. A passing build proves the code compiles; only a browser proves the screen works.

## Setup (once per worktree)

Prefer `uv` so nothing leaks into the system Python:

```
uv run --with playwright python3 -m playwright install chromium
```

Then run each script with `uv run --with playwright python3 <script>.py`. If `uv` is missing, use `python3 -m pip install --user playwright` and `python3 -m playwright install chromium`.

Put scripts and screenshots under `.rui/e2e/` (or the project's existing e2e folder). Never write into `/tmp` paths other agents may share.

## The helper: `scripts/with_server.py`

It starts one or more servers, waits until each port accepts connections, runs your command, then stops the servers, even when the command fails.

**Treat it as a black box.** Run `python3 .claude/skills/webapp-testing/scripts/with_server.py --help` first and use it as documented. Don't read its source unless `--help` shows it can't do what you need.

Servers listen on `$PORT` in our setup. Pass the port you were given, never a hard-coded one:

```
python3 .claude/skills/webapp-testing/scripts/with_server.py --server "npm run dev -- --port $PORT --strictPort" --port $PORT -- uv run --with playwright python3 .rui/e2e/check_home.py
```

Backend plus frontend: repeat `--server` / `--port` pairs, backend first. Pass the base URL to your script through an environment variable or an argument rather than baking it in.

If the app is already running (the job says so, or `curl -s localhost:$PORT` answers), skip the helper and point the script at it.

## Choose the approach

```
Is it a static HTML file?
  yes -> read the file to find selectors -> script with file:// URL
         (if selectors are unclear, treat it as dynamic)
  no  -> is a server already running?
           no  -> with_server.py + a script with only Playwright logic
           yes -> reconnaissance-then-action (below)
```

## Reconnaissance, then action

Never guess selectors from source code for a dynamic app. Look at what actually rendered.

1. **Navigate and wait for `networkidle`.** This is the most common mistake: inspecting the DOM before the JavaScript has run.
2. **Inspect:** take a full-page screenshot, dump `page.content()`, list buttons, links and inputs.
3. **Pick selectors from the rendered state.** Prefer role and text: `get_by_role("button", name="Save")`, `get_by_label("Email")`, then IDs, then CSS.
4. **Act and assert.** Every check ends in an assertion or an explicit printed PASS/FAIL, not just "it didn't crash".

```python
import os, sys
from playwright.sync_api import sync_playwright

BASE = os.environ.get("BASE_URL", f"http://localhost:{os.environ['PORT']}")
errors = []

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)  # always headless
    page = browser.new_page()
    page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
    page.goto(BASE)
    page.wait_for_load_state("networkidle")  # wait for JS before inspecting
    page.screenshot(path=".rui/e2e/home.png", full_page=True)

    page.get_by_label("Title").fill("Buy milk")
    page.get_by_role("button", name="Add").click()
    page.get_by_text("Buy milk").wait_for()
    browser.close()

if errors:
    print("FAIL console errors:", errors)
    sys.exit(1)
print("PASS add item")
```

## Rules

- Always `headless=True` and always close the browser.
- Capture console errors and failed network requests on every run. A screen that "looks fine" with console errors is not fine.
- Use explicit waits (`wait_for`, `wait_for_selector`, `expect(...).to_be_visible()`). Use `wait_for_timeout` only as a last resort, and say why.
- Check the states, not only the happy path: loading, empty, error (stop the backend or send a bad request), and a narrow viewport (`page.set_viewport_size({"width": 375, "height": 812})`).
- Look at your screenshots. Read the PNG back and describe what you see in one line each.
- Keep each script to one flow. Several small scripts beat one long one when something fails.
- Don't edit app code from a test-only job. If the app is wrong, report it.

## Reporting

Run the job's done command, then `report_status`:
- `DONE`: list each flow checked with PASS, the viewport sizes, and screenshot paths.
- `DONE_WITH_CONCERNS`: flows pass but you saw console warnings, slow loads, or untested states.
- `BLOCKED`: the server won't start or Playwright can't install. Paste the exact error.
- A failing flow caused by an app bug is a finding, not a blocker: report the failing step, expected vs actual, and the screenshot path.

---
Adapted from anthropics/skills skills/webapp-testing (Apache-2.0, Copyright Anthropic, PBC). This file was modified by RUI: `$PORT`-based servers, uv setup, console-error capture, state coverage and RUI status reporting. `scripts/with_server.py` is redistributed unchanged apart from a header comment.
