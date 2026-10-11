# contextleleo demo: narration script

Voiceover for the 2:49 demo video ([`docs/assets/contextleleo-demo-1080p.mp4`](assets/contextleleo-demo-1080p.mp4), 1080p), section by section. The voice is the macOS "Samantha" text-to-speech voice.

| Time | Section |
| :--- | :--- |
| 0:00 | Title |
| 0:08 | The problem |
| 0:28 | How it works |
| 1:06 | Normal case: the terminal |
| 1:26 | Normal case: in Antigravity |
| 1:45 | Retrieval: the terminal |
| 2:15 | Retrieval: in Antigravity |
| 2:31 | Try it |

### 0:00 · Title

contextleleo. Carry your coding agent's context to the next agent, without carrying the baggage.

### 0:08 · The problem

Every coding agent keeps its history locked inside its own store. When you switch agents, you either start from zero, or paste a huge transcript. That burns tokens, and the new agent has to dig through noise, or compact it blindly. And the fix you found last month, in a different project? It's buried in a session you can't search.

### 0:28 · How it works

Here is how contextleleo works. It reads the real session stores of your agents, Claude Code, Cursor, Antigravity and more, through one adapter per agent. For retrieval, a fast local search proposes candidate passages. Then the Jev API answers yes or no for each one, with a relevance score. Weak ones are ignored. The kept passages, and your own session, are scored again for the next task. Plain rules turn those scores into keep, shrink, or drop. Every shrunk message leaves a stand-in that links back to the untouched original. And the result is written as a native session in the target agent's own store.

### 1:06 · Normal case: the terminal

First, the normal case. This is my current Claude Code session, and it is huge. One command moves it to Antigravity, and I tell Jev what happens next: fix the retry logic. Jev scores every message. Big, irrelevant tool output is dropped or shrunk. About forty percent fewer tokens.

### 1:26 · Normal case: in Antigravity

And here is the result in Antigravity. The normal session is there, under the project: my first message in full, and the long tool output replaced by short stand-ins. Eight hundred sixty four messages, a hundred ninety nine of them shrunk, and every stand-in points back to its exact message in the original.

### 1:45 · Retrieval: the terminal

Now retrieval. I hit a hedge vault bug in this session, and months ago, in another project with another agent, I fixed something similar. Local search finds sixty four candidates. Jev scores each one. Three are kept, all around point nine four. The rest are ignored: some over the limit of three, some below the point seven floor, and some Jev rejects outright. Then the chosen passages and the session are scored again, shrunk to what the next agent needs, and written as a new session.

### 2:15 · Retrieval: in Antigravity

Now the retrieval conversation. I reopen Antigravity, and there it is, under the project, titled with the question. It opens with the question, then the retrieved passages, each labelled with its source project and relevance, then the trimmed session.

### 2:31 · Try it

Retrieval and trimming use the Jev API, and need a key. The setup command asks for it. Everything else works with no account. Only short excerpts, with credentials masked, go to Jev, and your original sessions are never modified. contextleleo is open source, built on txcript.
