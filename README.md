# CISSP Coach

A local CISSP study app: adaptive 50-question batches across all eight CISSP
domains, per-domain difficulty progression, and a 7-step "Think Like a Manager"
chat coach. Runs as a single Rust binary on `127.0.0.1`, reads API keys from
`.env`, and persists everything to a single SQLite file.

## Quick start

1. **Install Rust** (if you don't have it): https://rustup.rs/
2. **Clone / enter the project directory:**
   ```powershell
   cd C:\opt\CISSP_Exam
   ```
3. **Copy the env template and add your key(s):**
   ```powershell
   Copy-Item .env.example .env
   notepad .env       # set OPENAI_API_KEY and/or ANTHROPIC_API_KEY
   ```
4. **Run it:**
   ```powershell
   .\scripts\run.ps1            # debug build, fast iteration
   .\scripts\run.ps1 -Release   # optimized build (slower first compile)
   ```
   The launcher copies `.env.example` → `.env` if missing, runs the server, and
   opens `http://127.0.0.1:7878` in your default browser.

You can also run manually:
```powershell
cargo run
# or
cargo build --release
.\target\release\cissp-coach.exe
```

## Configuration (`.env`)

```
OPENAI_API_KEY=sk-proj-...        # at least one provider must be set
ANTHROPIC_API_KEY=sk-ant-...      # optional
BIND_ADDR=127.0.0.1:7878          # local-only by default
DATA_DIR=./data                   # SQLite lives here
DEFAULT_PROVIDER=openai           # 'openai' | 'anthropic'
DEFAULT_MODEL=gpt-4o
RUST_LOG=info
```

The frontend reads the booleans `has_openai` / `has_anthropic` from
`/api/settings` so it can show ✅/❌ for each, but the **keys themselves never
leave the server**.

## Data

Everything lives in `data/cissp.db`. To back up, just copy that file. To migrate
to another machine, copy `data/` over.

If you have a JSON export from the previous browser-localStorage version of the
app (the `📥 Export DB` button), you can drop it into the **⚙️ Settings → 📤
Import DB** flow on the new server. The schema is forwards-compatible.

## Architecture

- **Rust + axum + rusqlite + reqwest** — single static binary; SQLite is bundled.
- **`static/index.html`** is embedded into the binary via `rust-embed`, so the
  release build is a single `cissp-coach.exe`.
- **API keys are server-side only.** The browser never sees them. All LLM
  streaming runs through the Rust server, which forwards normalized SSE events
  back to the page.
- **Adaptive engine** (`src/engine.rs`) is a pure-function port of the previous
  JavaScript engine; same constants (`ROLLING_WINDOW=10`, `(1-acc)^1.5+0.10`
  weighting, `[2,15]` per-domain count clamp, ≥80% promote / ≤40% demote).
- **HTTP API** lives under `/api/*`:
  - `GET /api/settings`, `PATCH /api/settings`
  - `GET /api/dashboard`
  - `POST /api/batches/generate` (SSE: `plan` → `progress` → `done`/`error`)
  - `GET /api/batches/current`
  - `POST /api/batches/:id/answer | skip | finish | cancel`
  - `GET /api/batches/:id/summary`
  - `POST /api/batches/:id/study-guide` — returns a personalised PDF (optional, see below)
  - `POST /api/chat/stream` (SSE: `token` → `done`/`error`)
  - `GET /api/chat/history`, `DELETE /api/chat/history`
  - `GET /api/export`, `POST /api/import`, `POST /api/data/reset`

## Files

```
Cargo.toml
.env.example          # template; copy to .env
migrations/
  0001_init.sql       # schema, run on every boot (idempotent)
src/
  main.rs             # bootstrap
  config.rs           # .env parsing
  db.rs               # rusqlite + r2d2 pool
  engine.rs           # adaptive engine
  static_assets.rs    # embedded static/
  llm/
    mod.rs            # shared types
    openai.rs         # streaming chat completions
    anthropic.rs      # streaming messages
  routes/
    mod.rs            # router
    settings.rs
    dashboard.rs
    batches.rs        # generate/answer/skip/finish/summary
    chat.rs           # stream/history
    data.rs           # export/import
static/
  index.html          # the SPA
  system_prompt.txt   # 7-step coach prompt
scripts/
  run.ps1             # PowerShell launcher
data/
  cissp.db            # created on first run
```

## Optional: PDF Study Guide

After finishing a batch, the summary screen has a **📚 Generate Study Guide PDF**
button that produces a paginated PDF with:

1. A cover page with overall accuracy and a per-domain miss tally.
2. An LLM-synthesised study notes section (patterns, domain-by-domain review,
   suggested study order).
3. Every missed question verbatim, grouped by domain, with the correct answer
   highlighted, your wrong answer flagged, and the original explanation.

**This feature requires Python 3.10+ and ReportLab on the server** (the rest of
the app does not). PDF rendering is handled by `scripts/study_guide.py`, which
the Rust server spawns as a subprocess and pipes JSON to.

Install the Python dependency once:

```powershell
python -m pip install -r requirements.txt
```

If Python or ReportLab is not available, the rest of the app keeps working —
you'll just see an error if you click the study-guide button. Everything else
(quiz batches, chat coach, dashboard, export/import) is pure Rust.

## Security notes

- **`127.0.0.1` only by default.** Don't expose this to a LAN/internet without
  adding auth — there isn't any. It's a single-user local app.
- **`.env` is gitignored.** Don't commit it.
- **No CSRF protection.** The CORS layer is permissive because the only client
  is the embedded SPA on the same origin.

## Co-authored-by

Co-Authored-By: Oz <oz-agent@warp.dev>
