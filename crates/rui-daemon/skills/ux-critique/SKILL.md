---
name: ux-critique
description: Use whenever you are asked to review, critique, audit or judge a UI, screen or frontend change, and before calling any user-facing work done. Produces a scored design critique plus a technical audit with P0-P3 findings and concrete fixes. Read-only: it reports, it never edits.
license: Apache-2.0
metadata: {source: "https://github.com/pbakaus/impeccable/blob/main/skill/reference/critique.md", adapted: true}
bash: ["npx *", "npm *", "python3 *", "uv *", "curl *"]
---

# UX Critique and Audit

You judge a UI the way a design director and a front-end auditor would, together. You **change nothing**: no edits to app code, styles or tests. Your output is a report that the builders act on.

Two lenses, kept separate until the end:
- **Critique (A):** design and UX judgment. Hierarchy, specificity, cognitive load, heuristics, personas.
- **Audit (B):** measurable technical checks. Accessibility, performance, theming, responsive, implementation integrity.

Do A before you look at B's evidence. Tool output anchors judgment; form your own view first.

## 1. Set up

1. **Resolve the target** to concrete files and, if it runs, a URL. Prefer source paths over URLs; ports change, paths don't.
2. **Read the job and done criteria.** The critique is against what this screen is for, not against your taste.
3. **See it.** If the app runs, look at it in a browser (use the `webapp-testing` skill: screenshots at 1280px and 375px, plus each state you can reach: loading, empty, error, filled). A critique from source alone is a degraded run; say so in the first line of the report: `DEGRADED: source-only (<reason>)`.
4. **Independence.** If you have a subagent/Task tool, run A and B as two isolated subagents that don't see each other's output, then synthesize. If not, run A fully, write it down, then run B. Say which you did in the report header.

## 2. Assessment A: design critique

Score and judge using `references/rubric.md`. Cover:

- **Design specificity (start here).** Is this composition grounded in this product, or could an unrelated product use it unchanged? Name the category-interchangeable choices (generic hero, identical card grid, template chrome, default palette).
- **Hierarchy and composition.** Is it obvious within 5 seconds what matters most and what to do first?
- **Cognitive load.** Run the 8-item checklist. Flag any decision point with more than 4 visible options.
- **Nielsen's 10 heuristics.** Score each 0–4. A 4 means genuinely excellent; most real interfaces land 20–32/40. Mark a heuristic `n/a` only when it truly can't apply (e.g. help docs on a landing page) and renormalize the total.
- **Emotional journey.** Peak and end moments; reassurance at high-stakes steps (delete, pay, submit).
- **Personas.** Pick 2–3 from the rubric's table by interface type. Walk each through the primary action and list exactly what broke for them.
- **Copy.** Do labels, buttons and errors say what happens in the user's words?

## 3. Assessment B: technical audit

Run the checks in `references/audit.md` and score five dimensions 0–4: Accessibility, Performance, Theming, Responsive, Implementation Integrity. Use real evidence where you can:

- `npx --yes @axe-core/cli <url>` or an axe/Playwright script for accessibility, if it can install.
- `npx --yes lighthouse <url> --quiet --chrome-flags="--headless" --output=json` for performance, if available.
- Keyboard pass: tab through the primary flow and note focus visibility and traps.
- Grep the source for hard-coded colors, fixed widths, missing labels, `outline: none` without replacement.

Verify each automated finding in context before reporting it. Name false positives explicitly.

## 4. Severity: P0–P3

| | Name | Meaning |
|---|---|---|
| **P0** | Blocking | User can't complete the task. Fix now. |
| **P1** | Major | Significant difficulty or confusion, or a WCAG AA failure. Fix before release. |
| **P2** | Minor | Annoying, workaround exists. Fix next pass. |
| **P3** | Polish | No real user impact. Fix if time permits. |

Unsure between two levels? Ask: "Would a user contact support about this?" If yes, at least P1. Keep P3s few; a long tail buries what matters. Not everything can be P0.

## 5. The report

Write it in this order, in your `report_status` note (or a file the job names, e.g. `.rui/reviews/ux-<target>.md`):

1. **Header:** method (dual-agent / single-context / DEGRADED: reason), target, viewports and states seen.
2. **Specificity verdict:** authored for this product, or interchangeable? Two or three sentences with evidence.
3. **Scores:** heuristics table (`n/40` or renormalized with the n/a reasons) and audit table (`n/20`), each with its band.
4. **What's working:** 2–3 specific strengths and why they work.
5. **Priority issues:** the 3–7 most important, ordered by severity. Each one:
   - `[P?] What` — the exact element ("the Save button in the settings drawer", not "some buttons").
   - **Why it matters** — the user impact (and WCAG criterion if any).
   - **Where** — file and line, or component.
   - **Fix** — concrete and specific enough for a builder to do without asking you.
6. **Persona red flags:** per persona, what broke.
7. **Patterns:** systemic issues ("hard-coded colors in 12 components").
8. **Minor observations:** short list.
9. **Open questions for the Lead:** only where a fix depends on intent you can't infer (e.g. "Is the clinical tone intended?"), each with 2–3 options and your recommendation.

Score bands. Heuristics /40: 36–40 Excellent, 28–35 Good, 20–27 Acceptable, 12–19 Poor, 0–11 Critical (use percentages if renormalized: 90/70/50/30). Audit /20: 18–20 Excellent, 14–17 Good, 10–13 Acceptable, 6–9 Poor, 0–5 Critical.

## 6. How to write findings

- Be direct. Vague feedback wastes everyone's time.
- Be specific: name the element, the screen, the file.
- Say what is wrong **and** why it matters to users.
- Give a concrete fix. Cut "consider exploring" entirely.
- Prioritize ruthlessly. If everything is important, nothing is.
- Don't soften criticism, and don't invent problems to look thorough. Only report what you verified.

## 7. Reporting status

Read-only means your done command is the check you ran (e.g. the build or a Playwright script), not a fix. Then `report_status`:
- `DONE`: report delivered, counts by severity, both totals.
- `DONE_WITH_CONCERNS`: degraded run (no browser, tools wouldn't install) or states you couldn't reach.
- `NEEDS_CONTEXT`: you don't know what the screen is for and can't infer it.
- `BLOCKED`: the app won't run and there is no source to read.

---
Adapted from pbakaus/impeccable skill/reference/critique.md, critique-rubric.md and audit.md (Apache-2.0, Copyright 2025 Paul Bakaus). Impeccable itself started from Anthropic's frontend-design skill (anthropics/skills, Apache-2.0). This file was modified by RUI: critique and audit merged into one read-only skill, impeccable's own CLI, detector, storage and user-question steps removed, RUI status reporting added.
