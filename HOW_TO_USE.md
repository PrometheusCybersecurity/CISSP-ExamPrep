# How to Use CISSP Coach
A friendly walkthrough for studying for the CISSP exam with this app. If you're
looking for setup instructions (Rust, `.env`, Python for the PDF feature), see
[`README.md`](README.md). This file is for using the app once it's running.
## Table of Contents
1. [The big picture](#the-big-picture)
2. [Launching the app](#launching-the-app)
3. [The dashboard at a glance](#the-dashboard-at-a-glance)
4. [Quiz Mode — your daily driver](#quiz-mode--your-daily-driver)
5. [Difficulty tiers explained](#difficulty-tiers-explained)
6. [The "Coach this" button](#the-coach-this-button)
7. [Chat Coach — your tutor](#chat-coach--your-tutor)
8. [Study Guide PDFs](#study-guide-pdfs)
9. [Managing your data](#managing-your-data)
10. [Settings](#settings)
11. [A suggested study rhythm](#a-suggested-study-rhythm)
12. [FAQ](#faq)
## The big picture
CISSP Coach generates personalised practice questions, adapts to where you're
weak, and walks you through the **Think Like a Manager** reasoning the real
exam rewards. The two main surfaces are:
- **📝 Quiz Mode** — 50-question adaptive batches across all eight CISSP
  domains. The mix and difficulty are tuned to your recent performance.
- **💬 Chat Coach** — paste any question (yours, the app's, or one from a
  textbook) and get a 7-step breakdown: identify the trap, eliminate
  distractors, justify the manager-level answer, etc.
Everything you do is saved locally. No accounts, no cloud, no sync — just one
SQLite file in `data/cissp.db`.
## Launching the app
From the project folder, run the launcher:
```powershell
.\scripts\run.ps1
```
This builds the server, opens `http://127.0.0.1:7878` in your default browser,
and you're ready. Close the browser tab when you're done; the server keeps
running until you quit it (Ctrl+C in the terminal where it launched).
## The dashboard at a glance
When you first open the app you'll see the **Quiz Mode** tab with the dashboard:
- **🛡️ CISSP Adaptive Quiz** — title and quick description.
- **Stats bar** (very top of the page): total questions in your DB, overall
  accuracy, your weakest domain, and which AI provider/model is active.
- **Feature chips** — quick visual summary of your study state.
- **Big buttons:**
  - 🎯 **Generate 50 Questions** — kicks off a new adaptive batch.
  - 📚 **Generate Study Guide (All Misses)** — builds a PDF from every
    question you've gotten wrong across every batch (more on this below).
- **Next batch — adaptive distribution** — preview of how the next 50 will be
  split across domains (more questions go to weaker domains).
- **Domain cards** — one per CISSP domain showing your accuracy so far, the
  current difficulty tier (E / M / H / X), and how many questions are queued
  for that domain in the next batch.
There's also a tab switcher at the top: **📝 Quiz Mode** and **💬 Chat Coach**.
## Quiz Mode — your daily driver
### Generating a batch
Click **🎯 Generate 50 Questions**. A circular progress ring appears, and
questions stream in from the AI in real time. The batch is built so:
- All 8 domains are represented (minimum 2 questions each).
- Domains where you're scoring badly get more weight (up to 15 questions).
- Each domain's questions are pulled from its current difficulty tier; if
  you've earned 6+ slots, ~20% are pushed one tier above for stretch.
You'll see the per-domain plan as it generates ("D1 Risk Mgmt: 12 @ Hard",
etc.). Generation typically takes 60–120 seconds.
### Answering questions
Once generation finishes, you go straight into the first question:
- **Stem** at the top with domain + tier chips (e.g. `D3 · Arch / Eng` / `Hard`).
- **Four options** as buttons. Click one to answer.
- After you answer, the correct option turns green and (if you got it wrong)
  your pick turns red. The **Explanation** box appears below.
- Buttons:
  - **🧠 Coach this** (purple) — sends this exact question to Chat Coach for
    the 7-step breakdown.
  - **✖ End Batch** (red) — abandons the batch. Your already-answered
    questions are kept; the rest are discarded. Use this if you need to stop.
  - **⏭️ Skip** — moves on without answering. Skipped questions don't count
    toward your stats.
  - **Next ➤** — advances to the next question.
### Finishing a batch
When you answer (or skip) the last question, you land on the **summary screen**:
- **Big accuracy %** for this batch (color-coded green/amber/red).
- **Per-domain accuracy** — how you did in each domain this round.
- **Tier change report** — which domains got promoted (⬆), demoted (⬇), or
  held steady. The engine uses your recent accuracy to decide.
- **Missed questions** — clickable list. Click any miss to instantly send it
  to Chat Coach.
- **Buttons:**
  - 🎯 **Generate Next 50 (Adaptive)** — start another batch tuned to the
    new stats.
  - 📚 **Generate Study Guide PDF** — make a focused PDF for this batch's
    misses (see [Study Guide PDFs](#study-guide-pdfs)).
  - 📊 **Dashboard** — back to the dashboard view.
## Difficulty tiers explained
Each domain is independently tracked at one of four tiers, shown as a chip
(`E` / `M` / `H` / `X`):
| Tier | Name | What questions feel like |
|---|---|---|
| **E** | Easy | One-sentence recall / definition. Single concept. |
| **M** | Moderate | 2–4 sentence scenarios with one decision point. |
| **H** | Hard | Multi-sentence scenarios with cost/time/regulatory pressure; uses MOST/FIRST/BEST qualifiers. |
| **X** | Expert | Real-exam-style 4–7 sentence scenarios with deliberate misdirection and seductive distractors. |
### How tiers move
After each finished batch, per-domain rolling accuracy (last 10 answers) is
checked:
- ≥80% → **promote** one tier (E → M → H → X).
- ≤40% → **demote** one tier (X → H → M → E).
- Otherwise → **hold**.
You need at least 5 answers in a domain's recent window before tiers move, so
brand-new domains stay at Easy until you've calibrated.
## The "Coach this" button
Every question (during a batch and on the summary screen) has a **🧠 Coach
this** button. Clicking it:
1. Switches you to the Chat Coach tab.
2. Pre-fills the question stem, all four options, your answer, and the
   correct answer.
3. Asks the coach to walk through the **7-step "Think Like a Manager"
   breakdown.**
This is the single most useful study button in the app — every time you miss
something, hit it.
## Chat Coach — your tutor
Click the **💬 Chat Coach** tab to access the free-form coach. You can:
- Paste a question from anywhere (textbook, practice exam, work scenario)
  and ask for the breakdown.
- Ask conceptual questions ("Explain the difference between Bell-LaPadula
  and Biba in plain English").
- Drill specific topics ("Quiz me on PKI for 5 minutes, hard tier").
- Ask about your weak spots ("Based on my misses, what should I review
  before exam day?").
The Chat Coach uses the same provider/model you have configured. Responses
stream in real time.
### Managing the chat
- **Welcome screen** vanishes once you start chatting.
- **↓ Latest** button appears bottom-right when long answers push you below
  the latest message — click to jump down.
- **🗑️ Clear Chat** button (top-right of the chat pane) wipes the entire
  conversation. Useful when threads get too long. Only the chat is cleared —
  your question DB and stats are untouched.
- **📋** copy buttons on each AI message copy the answer to your clipboard.
## Study Guide PDFs
Two flavours, both produce a paginated PDF that downloads to your browser:
### Per-batch study guide
Available on the **batch summary screen** (after you finish or end a batch):
- Click **📚 Generate Study Guide PDF**.
- A circular progress ring appears at the top while the AI synthesises
  patterns from your misses and ReportLab renders the document.
- The PDF includes:
  1. **Cover page** — accuracy, missed-by-domain table, batch ID.
  2. **Study Notes** — AI-written analysis: cross-miss patterns,
     domain-by-domain review with anchor topics, suggested study order.
  3. **Missed questions** — every wrong answer in full, grouped by domain,
     with the correct option highlighted in green, your answer flagged in
     red, and the original explanation.
- Filename: `cissp-study-guide-YYYYMMDD-<batch-id>.pdf`.
### All-misses study guide (dashboard)
Available on the **main dashboard**:
- Click **📚 Generate Study Guide (All Misses)**.
- Same renderer, but it pulls every missed question across every batch
  (most recent 200) and produces an aggregate study guide. The AI gets a
  bigger sample so it can spot patterns that don't appear in any single
  batch.
- Filename: `cissp-all-misses-YYYYMMDD.pdf`.
> **Note:** The PDF feature requires Python + ReportLab on the server. If
> you haven't installed it, the rest of the app still works — you'll just
> get an alert if you click these buttons. See `README.md` for setup.
## Managing your data
All your data lives in `data/cissp.db`. The Settings modal (⚙️ button in the
top-right) gives you three controls:
- **📥 Export DB** — downloads a single JSON dump of every question, batch,
  domain stat, difficulty level, and chat message. This is your backup.
- **📤 Import DB** — pick a previously-exported JSON file. Replaces your
  current question bank, stats, and difficulty levels.
- **🗑️ Reset All Data** (red, right side) — wipes everything except your
  provider/model preference. Two confirmations required. Useful if you want
  a fresh start, or if your accuracy stats are skewed by old practice
  sessions you no longer want counted.
**Quick backup workflow:** click Export DB once a week. Save the file
somewhere safe. If anything ever goes sideways, Import DB restores it.
## Settings
Click ⚙️ Settings in the top-right header.
- **API keys** — shown as ✅ / ❌ so you know which providers are configured.
  The actual keys live in `.env` on the server and never leave it.
- **Provider** — OpenAI or Anthropic. Both are supported equally.
- **Model** — dropdown of current models for the chosen provider:
  - **OpenAI**: GPT-4o (recommended), GPT-4o Mini, GPT-4 Turbo, GPT-4, GPT-3.5.
  - **Anthropic**: Claude Sonnet 4.6 (recommended), Opus 4.7, Opus 4.6,
    Sonnet 4.5, Haiku 4.5.
- Click **💾 Save** to persist the choice. Provider/model survive even
  through Reset All Data.
**Pro tip:** for question generation, prefer the higher-tier model (Sonnet
4.6 / GPT-4o). For chat coaching, the same model works well. Faster/cheaper
models will produce noticeably less exam-realistic questions.
## A suggested study rhythm
The app rewards consistency. Here's a workflow that works:
1. **Day 1:** generate one batch. Don't worry about the score — it's
   calibration. Use **Coach this** on every miss.
2. **Day 2–7:** generate one batch per day. After each batch:
   - Read the tier-change report. Domains promoted? Great. Demoted? Spend
     extra time there.
   - Click **Generate Study Guide PDF** for any batch where you scored
     under 70% — saves the misses for offline review.
3. **End of week:** generate the **all-misses** study guide on the
   dashboard. Read it cover-to-cover. The AI patterns section is where
   you'll see your real weak spots.
4. **As exam approaches:** flip the dashboard chip — if your weakest domain
   is consistently the same one, batch it manually by dropping all other
   domains' tiers (you can do this implicitly by getting easier domains
   right and the hard one wrong, which the engine handles automatically).
**Time budget per batch:**
- ~1–2 minutes generation
- ~30–45 minutes answering (real exam questions take 1 min average,
  expert-tier ones take 2)
- ~10–15 minutes coaching the misses
- Total: ~45–60 minutes per session
## FAQ
**Q: Why are some of my answers showing the same letter (always A)?**
A: They shouldn't. The app server-side rebalances correct letters so each of
A/B/C/D appears ~25% of the time. If you see this, regenerate the batch.
**Q: What happens if I close the browser mid-batch?**
A: Nothing's lost. Your answered questions are persisted as you go. Reopen
the app and the batch continues from where you left off.
**Q: Can I delete a single bad question?**
A: Not directly through the UI. You can Reset All Data and start over, or
edit `data/cissp.db` with a SQLite tool if you're comfortable with that.
**Q: The study guide PDF says "no missed questions in the database".**
A: You haven't gotten any wrong yet (nice), or you reset your data
recently. Answer some questions first.
**Q: Can I run this on another machine?**
A: Yes. Stop the server, copy the entire `C:\opt\CISSP_Exam` folder
(including `data/`), put it on the new machine, install Rust + Python +
your `.env` keys, and run `scripts\run.ps1`.
**Q: Is my data sent anywhere?**
A: Only to your chosen LLM provider (OpenAI or Anthropic) when generating
questions or chatting with the coach. Your stats, batches, and answers stay
on your machine. The server binds to `127.0.0.1` only, so even other
machines on your network can't reach it.
**Q: How long until I'm exam-ready?**
A: Depends on your starting point. A typical pattern: ~10 batches (500
questions) before all domains hit Hard tier consistently, then another ~5
batches at Hard/Expert before you're scoring 75%+ across the board.
Treat that as a floor, not a ceiling — the real exam includes question
formats this app doesn't simulate (drag-and-drop, hotspot, etc.), so use
official ISC2 practice exams in the final week too.
---
Good luck. Click ⚙️ Settings if anything's off, hit **🧠 Coach this** every
time you miss, and trust the tier system — it'll push you exactly as hard as
you can take.
