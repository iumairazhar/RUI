---
name: doc-coauthoring
description: Use whenever you write or substantially revise a document someone else will read without you there: a README, spec, design doc, decision record, runbook, guide or hand-back. Use it even for a "short" doc, because the reader test at the end is what catches the gaps you can't see.
license: Proprietary-RUI
metadata: {source: "Proprietary-RUI", adapted: false}
bash: []
---

# Writing Docs That Work Without You

A document fails when its reader has to ask the author a question. You will not be there to answer. So the job is not "write the doc", it is "write a doc a stranger can act on". This skill gets you there in four stages and proves it with a reader test.

## Stage 1: Gather context

Before writing a sentence, collect what the doc depends on. Write these down in your notes:

1. **Reader.** Who opens this doc and what do they already know? (A new engineer, the owner, another agent, an on-call person at 3am.)
2. **Job.** What must the reader be able to do or decide after reading? Write it as one sentence: "After reading, the reader can deploy the app locally" or "...can choose between option A and B".
3. **Questions.** List 3–5 questions the doc must answer for that reader. These become your reader test. Make them concrete: "Which port does the API run on?", "What happens if the migration fails?", not "Is the doc clear?".
4. **Sources.** Read the code, the job, linked issues, existing docs and the Lead's rulings. Every fact in the doc must come from a source you can point to. Don't invent behavior.
5. **Gaps.** Facts you need but can't find. If a gap is small and reversible, make the most conservative assumption and mark it in the doc as an assumption. If the doc's main decision rests on it, use `ask_lead` with the options and your recommendation.

## Stage 2: Structure first

Draft only headings plus one line under each saying what the section will contain. Check the skeleton before filling it:

- Does the order follow the reader's path? (Usually: what this is, why it matters, how to use it, details, edge cases.)
- Does each of your Stage 1 questions map to a section that answers it?
- Is anything there only because docs "usually have it"? Cut it.

Common skeletons:
- **README:** what it is (2 lines) → quick start (copy-paste commands) → configuration → common tasks → troubleshooting.
- **Spec / design doc:** problem → goals and non-goals → proposal → alternatives considered and why not → risks → open questions.
- **Decision record:** context → decision → consequences → options rejected.
- **Runbook:** symptom → check → fix → how to confirm it's fixed → who to escalate to.

## Stage 3: Draft section by section

Fill one section at a time, then reread it before moving on.

- **Lead with the point.** First sentence of each section says the main thing; detail follows.
- **Be concrete.** Real commands, real file paths, real values, real example output. A command the reader can copy beats a description of it.
- **One term per idea.** Pick a name and keep it. Don't call the same thing "worker", "member" and "agent".
- **Short sentences, plain words.** Define a term the first time it appears if the reader may not know it.
- **Say why.** A rule with its reason survives edge cases; a bare rule gets broken.
- **Mark uncertainty honestly.** "Not yet tested on Windows" is better than silence.
- **Cut.** After each section, delete the sentence that adds least. Repeat until each one earns its place.

Verify every command and path in the doc: run the commands that are safe to run (read-only, local) and check the paths exist. A doc with a broken quick start is worse than no doc.

## Stage 4: Reader test with a fresh agent

You can't see your own gaps because you already know the answers. A reader with no context can.

1. **Get a fresh reader.** If you have a subagent/Task tool, start one with no conversation context. If not, ask your Lead with `ask_lead` to run the test (another member or a fresh agent). The reader gets **only the doc** (paste it or give the path) and the questions. No job, no code access unless the doc tells the reader to look at code, no hints.
2. **Prompt it plainly:** "Using only this document, answer each question. Quote the sentence that answers it. If the document doesn't answer it, say 'NOT ANSWERED' and what you'd have to guess."
3. **Grade each answer** against what is true:
   - Correct, with a quote → pass.
   - Wrong → the doc is misleading there. Fix the wording.
   - NOT ANSWERED or guessed → the doc has a gap. Add the missing fact in the section the reader would look in.
4. **Fix and retest** with a new fresh reader (not the same one; it now has context). Repeat until all questions pass. Stop after three rounds and report what still fails.
5. Also ask the reader one open question: "What part of this doc was confusing or did you have to read twice?" Fix anything real.

## Hard rules

- Every factual claim traces to a source you read. No invented features, flags or defaults.
- Never paste secrets, tokens or personal data into a doc.
- Don't change code to make a doc true in a docs-only job. Report the mismatch instead.
- Keep the existing doc's structure and voice when revising, unless restructuring is the job.

## Reporting

Run the job's done command if it has one (for example a markdown lint or a docs build). Then `report_status`:

- `DONE`: the doc path, the reader-test questions, and "all passed in N rounds".
- `DONE_WITH_CONCERNS`: a question still fails, an assumption is marked in the doc, or a command couldn't be verified. Name each.
- `NEEDS_CONTEXT`: the facts the doc's main point depends on aren't in any source you can read.
- `BLOCKED`: you can't reach the sources at all.

---
Written by RUI. Proprietary-RUI. The idea of a fresh-reader test for docs is common practice; no third-party text is reproduced.
