# Critique rubric

Used by Assessment A in `../SKILL.md`: cognitive load, heuristic scoring and personas.

## Cognitive load

Three kinds:
- **Intrinsic:** the task's own complexity. Can't remove it; structure it (steps, defaults, progressive disclosure, grouped decisions).
- **Extraneous:** caused by bad design (confusing navigation, unclear labels, clutter, inconsistent patterns, needless steps). Remove it ruthlessly.
- **Germane:** effort spent learning. Good; support it with consistent patterns and feedback that confirms understanding.

### Checklist (count failures)

- [ ] **Single focus:** the primary task can be done without competing elements.
- [ ] **Chunking:** information comes in groups of 4 or fewer.
- [ ] **Grouping:** related items are visually together (proximity, border, shared background).
- [ ] **Hierarchy:** it's immediately clear what matters most.
- [ ] **One thing at a time:** one decision before the next.
- [ ] **Minimal choices:** 4 or fewer visible options at each decision point.
- [ ] **Working memory:** the user doesn't need to remember something from a previous screen.
- [ ] **Progressive disclosure:** complexity appears only when needed.

0–1 failures: low load. 2–3: moderate, address soon. 4+: high, critical.

### Working memory rule

People hold about 4 items at once (Cowan, 2001). At each decision point count options, actions and facts the user must weigh together: up to 4 is fine, 5–7 is pushing it, 8+ means skips, misclicks and abandonment.
- Buttons: 1 primary, 1–2 secondary, the rest in a menu.
- Top navigation: 5 or fewer items.
- Gallery or index pages: one decision per screen.

### Common violations

1. **Wall of options:** 10+ choices, no hierarchy. Group, highlight a recommended one, disclose progressively.
2. **Memory bridge:** step 3 needs info from step 1. Keep it visible or repeat it.
3. **Hidden navigation:** the user must build a mental map. Show location (active states, breadcrumbs, progress).
4. **Jargon barrier:** system words instead of user words. Use plain language; define unavoidable terms inline.
5. **Noise floor:** everything has the same weight. One primary, 2–3 secondary, the rest muted.
6. **Inconsistent pattern:** same action works differently in different places. Standardize.
7. **Multi-task demand:** read, decide and navigate at once. Sequence it.
8. **Context switch:** info for one decision spread across screens. Co-locate it.

## Nielsen's 10 heuristics, scored 0–4

Be honest: 4 means genuinely excellent, not "good enough". Generic scale per heuristic: 0 absent or hostile, 1 mostly failing, 2 partial with major gaps, 3 good with minor gaps, 4 excellent throughout.

1. **Visibility of system status.** Loading indicators, confirmation of save/submit/delete, progress in multi-step flows, current location, inline validation. 0 = user guesses what happened; 4 = every action confirms.
2. **Match with the real world.** User's vocabulary, logical order, recognizable icons. 0 = pure tech jargon; 4 = fluent in the user's language.
3. **User control and freedom.** Undo, cancel on forms and modals, a way back, clearing filters, escaping long flows. 0 = trapped without refresh; 4 = undo, cancel, back and escape everywhere.
4. **Consistency and standards.** Same words, same results, platform conventions, consistent visuals. 0 = feels like several products stitched together; 4 = predictable system.
5. **Error prevention.** Confirm destructive actions, constrain inputs, smart defaults, autosave. 0 = no guardrails; 4 = errors nearly impossible.
6. **Recognition over recall.** Visible options, inline hints, recent items, labels on icons. 0 = heavy memorization; 4 = everything discoverable.
7. **Flexibility and efficiency.** Keyboard shortcuts, bulk actions, favorites, power features that don't clutter basics. 0 = one rigid path; 4 = multiple paths and accelerators. (May be n/a on landing pages.)
8. **Aesthetic and minimalist design.** Only what's needed, clear hierarchy, purposeful color. 0 = everything competes; 4 = every element earns its place.
9. **Recognize, diagnose, recover from errors.** Plain language, specific ("Email is missing @"), a fix, shown near the field, form not wiped. 0 = cryptic or no message; 4 = pinpoints, suggests fix, keeps work.
10. **Help and documentation.** Contextual hints, task-focused, findable without leaving. 0 = none; 4 = right info at the right moment. (May be n/a on landing pages.)

## Personas

Pick 2–3 by interface type, walk each through the primary action, and report specific red flags (what broke), not generic descriptions.

| Interface | Personas |
|---|---|
| Landing / marketing | Jordan, Riley, Casey |
| Dashboard / admin | Alex, Sam |
| Checkout / commerce | Casey, Riley, Jordan |
| Onboarding | Jordan, Casey |
| Data-heavy | Alex, Sam |
| Forms / wizards | Jordan, Sam, Casey |

- **Alex, impatient power user.** Skips onboarding, wants shortcuts and bulk actions, hates needless confirmations. Red flags: forced tutorials, no keyboard path for the main action, slow unskippable animation, one-at-a-time where batch is natural.
- **Jordan, confused first-timer.** Reads everything, takes labels literally, hesitates on the unfamiliar. Red flags: icon-only navigation, unexplained jargon, no visible help, unclear next step, no success confirmation.
- **Sam, accessibility-dependent.** Screen reader, keyboard only, maybe 200% zoom. Red flags: click-only controls, invisible focus, meaning by color alone, unlabeled fields, time limits, custom widgets that break reader flow.
- **Riley, stress tester.** Empty states, 1000 items, very long text, emoji, refresh mid-flow, two tabs. Red flags: silent failures, broken UI after errors, useless empty states, data lost on refresh, inconsistent similar interactions.
- **Casey, distracted mobile user.** One thumb, interrupted, slow network. Red flags: key actions at the top, progress lost on app switch, typing where a choice would do, heavy assets, tap targets under 44×44.

If the project describes its real users (README, PRODUCT.md, the job), add one persona built from that description. Don't invent audience details.

---
Adapted from pbakaus/impeccable skill/reference/critique-rubric.md (Apache-2.0, Copyright 2025 Paul Bakaus). Modified by RUI: condensed, scoring tables summarized, impeccable-specific steps removed.
