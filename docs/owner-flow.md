# Owner flow

Agreed 2026-11-17. This describes how you, the owner, follow and steer work in RUI. It is being built in three phases: daemon state and Inbox, then the app's Inbox and board, then the task page.

## Why

Today one piece of work shows up in four places (Rui chat, Work → Threads, Work → Tasks, Inbox), each with its own words.

"Needs you" can light up on Work while the Inbox is empty, because stops, failures and loop limits never reach the Inbox.

Messages to a blocked agent are saved and then silently go nowhere. Stops record only "owner", so nobody can tell which button stopped what, or why.

## Three rules

1. **The Inbox is the only place that says "needs you".** If something needs you, it is an Inbox item with a reason and buttons. Nothing else uses the words "Needs you".
2. **Every task has one home: its task page.** Chat, Inbox and Work only link to it.
3. **Every action you take gets a visible answer, right away.** A message, a stop, an answer or an approval always produces a line on the task page saying what happened next.

## The journey

You ask Rui for something in chat. Rui sets up a task and replies with a link card to its task page.

The task page shows the goal, the team, and one status line in plain words, e.g. "Working: Researcher 01 is reading the fide handbook."

While the team works, you don't have to watch. When it needs you, an Inbox item appears and the rail badge counts it.

You act from the Inbox item (answer, approve, resume, review). The task page timeline records what you did and what happened next.

When the work lands, you get one Inbox item: "Done, review the result", with the files and a short summary. Closing it marks the task done for you.

## Status words

These are the same five words everywhere: chat cards, Work, task page and Inbox.

| Word | Meaning | Reason line, always shown |
|---|---|---|
| Needs you | There is an open Inbox item | "Asks: which level do you need?" |
| Working | An agent is running now | "Researcher 01 is searching the web" |
| Waiting | Can't run yet, through no fault of yours | "Waiting on Researcher 01" or "Waiting for a free seat" |
| Stopped | Someone stopped it | "Stopped by you from Work → Stop all, 23:31" |
| Done | Landed and reviewed | "Delivered 2 files" |

A task that is stuck by itself (an error, a loop limit, a merge conflict) is "Needs you", with an Inbox item that explains it and offers "Retry", "Give it new instructions" or "Drop it".

## Inbox items

Each item has a title, one reason sentence, the task it belongs to, and two or three buttons.

- **Question:** an agent or Lead asks you something. Buttons are its options plus a free answer.
- **Approval:** Rui or an agent wants to run something risky, e.g. a dangerous command. "Run it" / "Don't run".
- **Stuck:** an error, loop limit or conflict. "Retry" / "New instructions" / "Drop it".
- **Review:** work landed. "Looks good" / "Ask for changes".

Stops you make yourself don't create an Inbox item. You did it, so the page simply shows "Stopped", with a "Resume" button.

## Task page

The task page replaces the side drawer. From top to bottom:

- **Header:** title, client, status word with its reason line, and Stop / Resume.
- **Team strip:** the Lead and each member, each with its own status word.
- **Timeline:** what happened, newest last, in plain sentences. Who did what, what you said, what they answered, and every stop, with its source and reason. The tool calls behind a step stay folded until you open them.
- **Results:** the files produced, opened in the viewer.
- **Message box:** "Message the team" with a picker for who gets it (the Lead by default).

## Messages to the team

When you send a message, the page answers immediately with what will happen. For example:

- "Sent to Researcher 01. It's running and will read it next."
- "Sent to Technical Writer 01. Its Lead and Researcher 01 were stopped, so I'm resuming them first."

If the chain can't be resumed (e.g. the task was dropped), it says so, and offers "Resume team".

## Stops

Every stop records **who** (you, Rui, or the system), **where from** (task page, Work → Stop all, Rui chat) and an optional **reason**.

Stop asks "Why? (optional)" in one line. Stop all asks you to confirm, and lists what it will stop.

Rui can stop only with a reason, and its stops show up in the timeline as "Stopped by Rui: <reason>".

## Work page

There is one board, not two. Its columns use the same words: Needs you, Working, Waiting, Stopped, Done.

Each card is one task, with its title, client and reason line. Clicking a card opens the task page.

The timeline chart stays above the board, unchanged.

## What goes away

- The Work → Tasks lanes view (Taken in / Shaped / Being built / Your call / Delivered).
- The thread side drawer. Its content moves into the task page.
- The words "Standing by", "In the loop", "Your call", "Landed" and "Thinking".

## Source of truth

This doc is the source of truth for the owner flow (decided 2026-11-17). Where it differs from the Open Design mock (the Tasks tab, the side drawer), this doc wins.

## Rui chat

Open questions from Rui sit at the bottom of the chat until you answer them.

Once answered, a question stays where it was asked, as one compact line ("You chose: Backend only"), and scrolls up with the conversation. Today every answered card is redrawn at the bottom after each new message (`views/chat.rs`, the `ask_cards` after the message list).

## Decided

- Landed work is not "Done" until you review it. It shows as "Needs you" with a Review item until you press "Looks good". "Ask for changes" sends it back to the team.
- Every new Inbox item for a stuck task or a review also sends a macOS notification.
- No database schema change. New states (stop source and reason, reviewed, dropped, resumed) are recorded as events, and the Inbox and status words are computed from existing data.

## Still open

- Does Rui's chat reply for a new task need anything more than the link card?
