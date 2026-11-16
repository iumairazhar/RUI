---
name: escalation-ladder
description: Use whenever a Lead or Rui receives a question from below, or is about to pass a question up. Covers when to rule yourself versus escalate, how to write a ruling with "cost if wrong", and how to write a question for the level above with options and a default. Use it before every ask_lead answer and before every escalation, even when the answer feels obvious.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: []
---
# Escalation Ladder

Questions climb one rung at a time: **member → Lead → Rui → owner**.

Each rung answers everything it safely can and passes up only what it cannot.

The ladder exists so work never stalls and the owner is only asked about things that are truly theirs.

## Who talks to whom

- A member asks only its own Lead, through `ask_lead`.
- A Lead answers its members and asks only Rui.
- Only Rui talks to the owner.
- Never skip a rung. A Lead never asks the owner directly, even when the owner "obviously" knows. Rui may already know the answer, or may batch your question with others.

## The core test: rule or pass up?

Rule yourself when **all** of these hold:

1. **Reversible.** If you are wrong, the fix is a code change in this workspace, not a lost database, sent email or spent money.
2. **Inside the task's scope.** The answer does not change what the owner asked for. It only changes how.
3. **Cost if wrong is small.** Roughly: under an hour of rework for the pod, and nothing the owner would notice in the final result.
4. **You have the facts.** You can name the evidence (code, docs, the brief) the ruling rests on.

Pass it up when **any** of these hold:

| Trigger | Example |
|---|---|
| Changes the owner's goal or scope | "Should we also add login?" when the brief never mentioned users |
| Spends money or creates accounts | A paid API key, a cloud resource, a new SaaS signup |
| Irreversible | Dropping data, rewriting git history on a shared branch, deleting files outside the task |
| Security-sensitive | Auth design, storing secrets, opening CORS widely, handling personal data |
| Side effects outside the workspace | Pushing, deploying, publishing, messaging anyone, touching other repos |
| Conflict beyond your authority | Two members need opposite things and the brief does not settle it |
| Every path is a guess | You cannot name evidence for any option |

When in doubt about reversibility, treat it as irreversible and pass it up.

When in doubt about anything else, rule conservatively and record the ruling. A slow answer costs the whole pod; a wrong reversible ruling costs one rework.

## Rulings, not stalls

A member who asks is waiting. Answer fast.

- Answer within the same turn you read the question. Do not "think about it later".
- If you need one fact, look it up yourself (read the file, run the command) instead of bouncing the question back.
- If you must pass it up, tell the member what to do meanwhile, so they keep moving on the parts that do not depend on the answer.
- Never answer "it depends" without saying what it depends on and which branch to take now.

## How to write a ruling

Keep it to five short lines.

```
RULING: <the decision, one sentence, imperative>
WHY: <the evidence or reason, one or two sentences>
COST IF WRONG: <what breaks and how much rework, honestly>
WOULD CHANGE IF: <the fact that would flip this ruling>
SCOPE: <this job only | whole pod | until Rui says otherwise>
```

Rules for rulings:

- **Decide one thing.** If the question bundled three, rule on each separately.
- **Name the cost if wrong.** If you cannot estimate it, you do not understand the question well enough to rule, so pass it up.
- **Write what would change it.** This lets the member spot the moment the ruling stops applying, without asking again.
- **Record pod-wide rulings** in your plan or status note, so other members do not ask the same thing.
- **Do not rewrite the member's job inside a ruling.** If the job itself was wrong, reissue the job (see `handoff-contract`).

## How to write a question for the level above

The reader is busy and has none of your context. Make the answer one word.

```
QUESTION: <one sentence, ends with ?>
CONTEXT: <two lines max: what we are building and what just came up>
OPTIONS:
  A) <option> — <gain> / <cost>
  B) <option> — <gain> / <cost>
  C) <optional third>
RECOMMEND: <letter> because <one reason>
MEANWHILE: <what the pod keeps doing while waiting>
DEFAULT: <what we do if no answer arrives by <point in the work>>
```

Rules for questions:

- **Two or three options.** One option is not a question. Five is a dump.
- **Always recommend.** You are closest to the work. "No preference" passes your job upward.
- **Say what you will do meanwhile.** The pod never idles on a question.
- **Give a default** only when the default is safe. For the hard stops (irreversible, security, outside effects, money), the default is "we do not do it".
- **No jargon the owner would not know**, if Rui may forward it. Say "the app would lose saved notes" rather than "migration drops the table".

## Rui's extra duties

Rui is the last rung before the owner.

- Before asking the owner, check whether the brief, earlier owner answers, or another pod's ruling already settles it.
- Batch questions. One message with three clear questions beats three interruptions.
- Translate. Rewrite the Lead's question in the owner's words, keeping the options and the recommendation.
- After the owner answers, send the answer back down as a ruling, so the Lead can record it.

## Worked examples

### 1. Lead rules (reversible, in scope)

Member asks: "The brief says 'list notes'. Should the list be newest first or alphabetical?"

```
RULING: Sort newest first by created_at.
WHY: The brief calls it a journal; journals read most recent first.
COST IF WRONG: One-line change to the query and one test; about 10 minutes.
WOULD CHANGE IF: The brief or Rui mentions searching or browsing by title.
SCOPE: Whole pod (frontend shows the same order).
```

### 2. Lead passes up (security-sensitive)

Member asks: "Should we store the user's OpenAI key in the SQLite file so they don't re-enter it?"

The Lead cannot rule: storing a secret is security-sensitive. It asks Rui:

```
QUESTION: May the app save the user's API key on disk?
CONTEXT: We are building a local notes app with AI summaries. Re-typing the key each launch is annoying.
OPTIONS:
  A) Keep the key in memory only — safest / user re-enters it each launch
  B) Save it in the OS keychain — safe and convenient / adds a native dependency
  C) Save it in the SQLite file — simplest / key sits in plain text on disk
RECOMMEND: A for this task, because it ships today with no new risk; B can be a follow-up.
MEANWHILE: Backend builds the summary endpoint with the key passed per request.
DEFAULT: A. We never write the key to disk without an answer.
```

### 3. Lead resolves a conflict inside its authority

Backend wants `snake_case` JSON; Frontend wants `camelCase`. The brief does not say.

```
RULING: API returns snake_case; the frontend maps it in src/api.ts.
WHY: The backend is Python and its models already use snake_case; one mapping layer is cheaper than aliasing every model.
COST IF WRONG: Rename in one module; under 30 minutes.
WOULD CHANGE IF: Another client (mobile, a third party) will call this API.
SCOPE: Whole pod.
```

### 4. Lead passes up (scope change)

Tester notices the brief's "export to CSV" would be far more useful as "export to CSV and JSON". That is a scope change, so the Lead asks Rui with options (CSV only / both), recommends CSV only to finish on time, keeps the pod on CSV meanwhile, and sets the default to CSV only.

## Red flags

| Thought | Reality |
|---|---|
| "I'll just ask the owner, it's faster" | Only Rui talks to the owner. Ask Rui. |
| "I'm not sure, so I'll pass it up" | Unsure but reversible means rule conservatively and record it. |
| "It's small, I'll push to prod / delete it myself" | Irreversible and outside effects always go up, however small. |
| "I'll answer once the member has tried more" | A waiting member is a stalled pod. Answer now. |
| "The options are obvious, no need to list them" | The reader has none of your context. List them. |
