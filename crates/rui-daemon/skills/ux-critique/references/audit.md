# Technical audit checks

Used by Assessment B in `../SKILL.md`. Document issues; don't fix them. Score each dimension 0–4.

## 1. Accessibility

Check:
- **Contrast:** text below 4.5:1, or below 3:1 for large text.
- **Motion:** `prefers-reduced-motion` gets an intentional alternative that keeps state changes understandable. Flag a global `0.01ms` kill that removes useful feedback, flashing content, and motion that blocks reading or focus.
- **ARIA and semantics:** interactive elements without role, name or state; divs used as buttons; skipped heading levels; missing landmarks.
- **Keyboard:** missing focus indicator, illogical tab order, keyboard traps, actions only reachable by mouse.
- **Images:** missing or useless alt text.
- **Forms:** inputs without labels, errors not tied to fields, required fields not marked.

Score: 0 fails WCAG A, 1 major gaps, 2 partial, 3 WCAG AA mostly met, 4 AA fully met.

## 2. Performance

Check:
- Layout thrashing (reading then writing layout in loops).
- Animating layout properties (width, top) instead of transform/opacity; unbounded blur and shadow effects.
- Images without lazy loading or sizing; oversized assets.
- `will-change` applied broadly or left on.
- Heavy or unused dependencies in the bundle.
- Needless re-renders (React: unstable props, missing memo where lists are large).

Score: 0 severe, 1 major problems, 2 partial, 3 mostly optimized, 4 fast and lean.

## 3. Theming

Check:
- Hard-coded colors instead of tokens/CSS variables.
- Dark mode missing or low-contrast (if the product has one).
- Wrong or mixed tokens.
- Values that don't update when the theme switches.

Score: 0 everything hard-coded, 1 minimal tokens, 2 tokens used inconsistently, 3 tokens with minor leaks, 4 full token system.

## 4. Responsive

Check:
- Fixed widths that break below 400px; horizontal scroll.
- Touch targets: aim for 44×44px. WCAG 2.2 AA (2.5.8) requires at least 24×24 CSS px unless spacing or an equivalent control covers it.
- Touch gestures: custom sliders and drag areas that fail under touch, swallow page scroll, or stay stuck after an interrupted drag (mouse-only handlers, no `touch-action`, no cancel cleanup).
- Text scaling to 200% breaks layout.
- No mobile or tablet layout at all.
- RTL pages only (`dir="rtl"` on html/body): verify the rendered result where physical properties (`margin-left`, `left:`) are used; flag only what actually renders wrong.

Say what produced the evidence (real browser at a set viewport, emulated touch, source reading) and what stayed untested.

Score: 0 desktop-only, 1 major failures, 2 works with rough edges, 3 minor issues, 4 fluid everywhere.

## 5. Implementation integrity

Look for repeated shortcuts, design-system drift (one-off spacing, colors and components that duplicate existing ones), placeholder or misleading content left in, decorative structure that encodes nothing, and layouts interchangeable with an unrelated product. Keep tool findings separate from visual judgment and call out false positives.

Score: 0 systemic drift, 1 major repeated failures, 2 several verified issues, 3 minor isolated issues, 4 coherent and intentional.

## Audit table

| # | Dimension | Score | Key finding |
|---|---|---|---|
| 1 | Accessibility | ? | |
| 2 | Performance | ? | |
| 3 | Theming | ? | |
| 4 | Responsive | ? | |
| 5 | Implementation integrity | ? | |
| | **Total** | **?/20** | **band** |

Bands: 18–20 Excellent, 14–17 Good, 10–13 Acceptable, 6–9 Poor, 0–5 Critical.

For each issue record: `[P?] name`, location (file:line or component), category, user impact, standard violated (e.g. WCAG 1.4.3), and the fix.

Avoid: issues without impact, generic advice, skipping what works, everything at one severity, unverified tool output.

---
Adapted from pbakaus/impeccable skill/reference/audit.md (Apache-2.0, Copyright 2025 Paul Bakaus). Modified by RUI: impeccable's detector, CDP and storage commands removed; checks condensed.
