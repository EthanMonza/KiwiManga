# 🥝 KiwiManga

Personal Telegram bot that downloads manga & ranobe chapters/volumes, packs them
into CBZ/ZIP/EPUB archives and sends them straight into your chat.
Made by [@EthanMonza](https://t.me/EthanMonza). Personal use only — no public
catalogues, no channels.

## What it does

- **Plain-text search is the main UX.** Send a title (any language); the bot
  searches by title *and* alt-titles across all sources/languages with
  case/punctuation/whitespace-insensitive matching. One hit → card; several →
  paginated buttons; none → it asks for the full original title.
- **Pick what to download:** last 10 chapters, a range (`10-25`), volume(s),
  or a single chapter (paginated lists, callback payloads ≤ 64 bytes).
- **Chapter language** = your UI locale when the source has translations in it,
  otherwise an honest fallback to `en` (manga) / `ru` (ranobe) with a warning.
- **Packing:** chapter → CBZ (zero-padded pages). Volume → one merged archive
  with continuous page numbers (default) *or* one ZIP with N chapter files
  (`/settings` → per-chapter). Ranobe → EPUB (merged) / ZIP of EPUBs.
- **Telegram 50 MB cap:** archives above it are split into chapter-range parts
  labelled «Part k/n», sent sequentially.
- **i18n:** en, ru, tr, de, da, no, sv, fr, ar, ja, ko, zh, fi, mi (te reo
  Māori), es, it — and **kiwi-en**, a fully translated locale in exaggerated
  NZ slang. Missing key → `en`. Auto-detection via whatlang; a manual
  `/language` choice always overrides.

Commands: `/start` `/language` `/help` `/queue` `/cancel` `/status` `/settings`
(all registered with BotFather-style descriptions at startup + a reply menu).

## Architecture (built for 1 → tens of thousands of users)

```
Telegram ⇄ teloxide long-polling (no ports, no webhook)
   │  handlers are thin: validate → enqueue job → "accepted, position N"
   ▼
SQLite (/data/bot.db, sqlx migrations, WAL)
   jobs(chat_id, kind, payload, status, progress, error, attempts, timestamps)
   users(locale, auto_locale, pack_mode, chapter_lang)
   chapters_cache(source, title_id, chapter_id, lang, path, bytes)
   ▲  N workers (WORKERS env, default 2) claim jobs atomically
   │  global token-bucket rate limiter per source (MangaDex ≈ 1 rps —
   │  the source is shared by all users, so the limiter is global, not per-user)
   ▼
download → /data/cache/<source>/<title>/<chapter> → pack → upload
```

Guarantees & protections:

- **Resume after `kill -9`/redeploy:** on start and on shutdown, `running →
  pending`. A worker never holds work that dies with it.
- **Per-user caps:** max **3** active jobs, max **200** chapters per job —
  polite refusals otherwise. Dedup: the same pending/running job for a chat is
  rejected (unique partial index).
- **Source politeness:** `Retry-After` respected, exponential backoff + jitter,
  ≤5 attempts/request, 30 s per *request* timeout (never per job). Job-level
  transient failures are retried up to 3 attempts.
- **Telegram flood control:** progress edits throttled to 1 per 2.5 s;
  `RetryAfter` from TG → sleep + retry.
- **Cancellation** `/cancel` or the 🛑 button under every progress message —
  checked between chapters; temp files are cleaned up on every error path.
- **Disk:** free-space check before each job (200 MB floor).
- **Caches:** moka TTL caches for search/title sessions (30 min); on-disk
  chapter cache + `chapters_cache` table → repeats are served from disk.

**Scaling path (not in v1, by design):** handlers are already stateless and
all state lives in SQLite. To grow beyond one node: swap the sqlx SQLite pool
for Postgres (queries are plain runtime-checked SQL), run worker containers
separately (same codebase, a `--role worker` flag), and drop long-polling for
webhooks behind a load balancer. Nothing else changes.

## Configuration (env)

See [`env.example`](env.example).

| Var | Default | Meaning |
|---|---|---|
| `BOT_TOKEN` | — (required) | Telegram token (`TELOXIDE_TOKEN` accepted too) |
| `DATABASE_URL` | `sqlite:/data/bot.db?mode=rwc` | also accepts a bare path |
| `DATA_DIR` | `/data` | tmp + cache live here |
| `WORKERS` | `2` | concurrent job workers |
| `MAX_FILE_MB` | `50` | Bot API cap → split threshold |
| `LOG_LEVEL` | `info` | tracing level |
| `SOURCES_ENABLED` | `mangadex,ranobelib` | comma list |
| `VOLUME_PACK_DEFAULT` | `merged` | default for new users (`merged` \| `per_chapter`) |
| `MANGADEX_API_BASE` / `RANOBELIB_API_BASE` | official | override for tests/mirrors |
| `HTTP_TIMEOUT_SECS` / `HTTP_MAX_ATTEMPTS` | `30` / `5` | per-request |

## Deploy on Railway

1. Push this repo to GitHub → Railway → **New Project from GitHub repo**
   (Railway detects the Dockerfile).
2. **⚠️ Attach a VOLUME mounted at `/data`. Without a persistent volume the
   queue DB and the chapter cache are wiped on every redeploy — the bot will
   "forget" your pending jobs and re-download everything.**
   Railway UI: service → right-click → **Attach Volume** → mount path `/data`.
3. Set variables (`Variables` tab): at minimum `BOT_TOKEN`. Defaults for the
   rest match `env.example`.
4. The service needs **no public domain/port** — it is long-polling only. Do
   not enable "wake on demand"/sleep if you want the queue to keep draining.
5. **`cargo-chef` is what keeps the build alive**: the Dockerfile caches
   dependency compilation in a separate layer (`planner` → `builder` →
   `runtime` on `debian:bookworm-slim` + `ca-certificates`), so only your code
   recompiles per push. Without it the Rust build hits Railway's build timeout.
6. First deploy only: Railway builds ~8-10 min (cold dependency cache).

Rolling restarts are safe: SIGTERM → workers finish the current chapter, mark
jobs back to `pending`, exit clean.

## Local run

```bash
cp env.example .env   # fill BOT_TOKEN (and point DATA_DIR/DATABASE_URL at ./data)
cargo run
# or:
BOT_TOKEN=xxx DATA_DIR=./data DATABASE_URL=sqlite:./data/bot.db?mode=rwc cargo run
```

## Adding a source

1. Create `src/sources/<name>.rs` implementing `trait Source`
   (`search`, `title_card`, `list_chapters`, `download_chapter`).
   No `async_trait` — dyn-compatible dispatch is done via the `AnySource` enum.
2. Add a variant to `AnySource` (`src/sources/mod.rs`) + its method delegations.
3. Register in `AppState::new` behind a name in `SOURCES_ENABLED`, add a token
   bucket in `ratelimit.rs`, and map source → `ChapterKind` in `worker::payload_kind`.
4. Respect the source's rules (User-Agent with contact, their rate limits).
   **Cloudflare/captcha bypasses and proxy chains are prohibited** — if the
   source is unreachable from the server, the bot says exactly that.
   `src/sources/stub.rs` is a ready-made skeleton.

## Adding a locale

1. Create `locales/<code>.json` — copy `locales/en.json` (107 keys) and
   translate **every** key. Placeholders `{name}` must stay.
2. Add the code to `SUPPORTED` and `LOCALE_FILES` in `src/i18n.rs` (and the
   BotFather `/language` list is built from `SUPPORTED` automatically).
3. Optional: map `whatlang`'s detection to it in `src/langdetect.rs`.
4. Missing keys silently fall back to `en`; `cargo test` enforces key +
   placeholder parity for all locales.
5. `kiwi-en` shows the bar: it is a full peer locale, not a two-line gag.

## Verification

Self-gate checklist and results live in [`REPORT.md`](REPORT.md). Quick manual
run (needs the Rust toolchain; the sandbox had none):

```bash
cargo build            # expected: clean build
cargo fmt --check      # expected: no diff
cargo clippy --all-targets -- -D warnings   # expected: zero warnings
cargo test             # expected: unit + integration suites green
```

Test-bots integration flow to run manually: text request in Russian with a
localized title → card → chapters in ru → archive into chat; same in `en` and
`kiwi-en` (the last one must *sound* Māori-Kiwi, not like `en`).

## Known limitations (deliberate, v1)

- SQLite + single process. Fine to thousands of users; the scaling path above
  is documented but not implemented.
- Long-polling only (as specified) — restarts briefly drop updates.
- The on-disk chapter cache has **no automatic eviction yet** (only the
  `chapters_cache` table + manual `rm -rf /data/cache`). Disk check guards
  every job, but add a cron/LRU if the volume is small.
- RanobeLib serves the ru locale only; requesting another language for its
  titles honestly falls back to ru.
- MangaDex cover images are not fetched into the card (text-only cards) —
  `TitleInfo.url` links the source page instead.
- Cloudflare-protected sources are *not* bypassed by design.
- `zip` files use `Stored` (no compression) — page JPEG/PNG don't shrink and
  this halves RAM/CPU per archive.
- EPUBs are EPUB 2.0 (epub-builder default) — opens in every mainstream reader.

## License / credit

Made with 🥝 by [@EthanMonza](https://t.me/EthanMonza) — the credit is baked
into `/start`, `/help` and every archive caption, in all locales.
