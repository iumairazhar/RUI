---
name: frontend-design
description: Use whenever you build a new screen, page or component, or reshape the look of an existing one, even a "quick" UI. It makes you commit to a deliberate visual direction grounded in the product instead of shipping templated defaults that read as AI-generated.
license: Apache-2.0
metadata: {source: "https://github.com/anthropics/skills/blob/main/skills/frontend-design/SKILL.md", adapted: true}
bash: ["npm *", "npx *", "node *"]
---

# Frontend Design

Work like the design lead at a studio known for giving every client a look nobody else has. The client already rejected proposals that felt templated. They are paying for a point of view: make deliberate, opinionated choices about palette, type and layout that fit this brief, and take aesthetic risk when the brief allows it.

There is no human to confirm the direction with mid-task. You pick it, write it down, and state it in your report so the Lead can overrule it cheaply.

## 1. Ground the design in the subject

If the job does not say what the product is, who uses it and what the screen's main job is, work it out from the repo (README, routes, data models, copy) and write a one-line answer to each. If it truly can't be inferred and the look matters, use `ask_lead` with two or three concrete directions and your pick. Otherwise choose and continue.

The subject's industry, materials and vocabulary are where distinctive choices come from. A tool for warehouse pickers looks nothing like a reading app for kids. Build with the job's real content and names throughout, never lorem ipsum.

## 2. Principles

- **First view.** Open with the most characteristic thing in the subject's world, in the form that fits: a headline, an image, a live demo, an interactive moment. "Big number, small label, supporting stats, gradient accent" is the default; use it only if it is truly best.
- **Typography carries the personality.** One family or two. If two, make them clearly different. Choose typefaces on purpose, not the ones you reach for on every project. Set a real type scale with intentional weights and spacing. Keep line length under about 80 characters; give serif body text a bit more line-height.
- **Avoid the common tells:** accenting one word in a headline with italic, bold or color; ALL-CAPS labels; decorative labels above every block.
- **Structure is information.** Borders, numbering, dividers and eyebrows must encode something true about the content. Numbered markers (01 / 02 / 03) only when the content really is a sequence.
- **Motion is rare and deliberate.** One orchestrated moment (a load sequence or one reveal) beats scattered effects. Fade-and-slide on every section and hover lifts on every card read as generated. Motion that answers a user action (open, expand, confirm) is welcome when it shows what changed.
- **Copy is design.** Generic copy makes a page feel as templated as generic layout. See section 5.

## 3. Calibrate against the current AI defaults

Generated design right now clusters around these looks. Each is fine for some briefs, but they are defaults, not choices:

1. Warm cream background (near #F4F1EA), high-contrast serif display, terracotta or clay accent (near #D97757).
2. Near-black background with one acid-green or vermilion accent.
3. Broadsheet layout: hairline rules, zero radius, dense newspaper columns.
4. The SaaS card kit: everything in identical rounded cards, one radius everywhere, the same soft grey shadow, gradient washes as decoration.
5. Template chrome on any subject: tracked-out caps eyebrow over every heading, "A · B · C" meta strings, "WORD — fragment" labels, #111 standing in for black, monospace for small data labels, "→" appended to buttons.

Where the job or an existing design system pins a direction, follow it exactly, even if it is one of these. Where an axis is free, don't spend that freedom on a default.

## 4. Process: plan, check, build, critique

**Plan first** (write it in your working notes, and summarize it in the report):
- **Color:** 4–6 named hex values for the base palette.
- **Type:** typefaces and their roles, plus the scale.
- **Layout:** a one-sentence concept and a small ASCII wireframe. Say how content aligns.
- **Principles:** two or three lines on what makes this screen specific to its subject.

**Check the plan against the brief.** For each part, ask: would I produce this for any similar page? If yes, revise it and note what you changed and why. Only then write code.

**Build in our stack (React + Vite + TypeScript):**
- Put tokens in one place: CSS custom properties in `src/index.css` (or the project's existing token file). Components use tokens, never raw hex.
- Load web fonts with a `<link>` in `index.html` or `@font-face` with `font-display: swap`. Always give a real fallback stack.
- Plain CSS or CSS modules unless the project already uses something else. Don't add a UI kit to get "a look".
- Watch selector specificity. A type selector (`.section`) and an element selector (`.cta`) can silently cancel each other's padding and margin.
- Match implementation effort to the vision: a maximalist direction needs real craft (layered composition, custom details); a minimal one needs precise spacing and type, not fewer hours.

**Quality floor, built in without announcing it:**
- Responsive down to 360px wide, no horizontal scroll.
- Visible keyboard focus on every interactive element.
- `prefers-reduced-motion` respected with a real alternative, not a blanket kill.
- Text contrast at least 4.5:1 (3:1 for large text).
- Loading, empty and error states for every data view.

**Critique your own work.** If a browser tool or the `webapp-testing` skill is available, screenshot the screen at desktop and mobile widths and look at it. A picture beats reading CSS. Then apply the mirror rule: remove one accessory. Spend boldness in one place and keep everything around it quiet.

## 5. Words in the interface

- Words exist to make the screen easier to understand and use. Decide what the screen needs to say before writing anything.
- Write from the user's side: they "manage notifications", not "configure webhooks". Describe what a thing does in plain terms instead of selling it.
- Active voice. A button says exactly what happens: "Save changes", not "Submit". Keep the same verb through the flow: "Publish" button, "Published" toast.
- Errors say what went wrong and how to fix it. They don't apologize and are never vague.
- An empty screen is an invitation to act: say what to do first.
- Sentence case, plain verbs, no filler. Each piece of text does one job.

## 6. Done and reporting

1. Run the job's done command (usually `npm run build`) and read the output. A clean TypeScript build is required.
2. If you could view the screen, say at which widths. If you could not, say so.
3. Call `report_status`:
   - `DONE` with the direction in two lines (palette, type, the one bold move) and files touched.
   - `DONE_WITH_CONCERNS` if you made a direction call the Lead may want to revisit, or skipped part of the quality floor.
   - `NEEDS_CONTEXT` only if brand rules or content you were told exist are missing.
   - `BLOCKED` if the build can't pass for reasons outside your files.

Never invent brand assets (logos, trademarks) for a real company. Use a neutral wordmark in the chosen type instead.

---
Adapted from anthropics/skills skills/frontend-design (Apache-2.0, Copyright Anthropic, PBC). This file was modified by RUI: rewritten for autonomous agents (no client to confirm with), a React + Vite build section, a quality-floor checklist and RUI status reporting.
