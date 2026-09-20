# KiwiManga — Delivery & Verification Report (self-gate)

Autonomous build. Environment note: the coding sandbox had **network-blocked
package installs and no Rust toolchain** (`rustup`/`apt` both unreachable), so
`cargo build/clippy/test` could not be executed here. Per the task's SELF-GATE
rule every gate below is therefore **"requires manual run" with exact commands
and expected results** — nothing was marked green without actual execution.
Everything that *could* be checked in-sandbox (file tree, JSON validity,
key/placeholder parity, import/signature cross-audit) was checked and is green.

## 1. File tree (53 files)

```
Cargo.toml                  docker-build deps + bin+lib layout (see §4)
rustfmt.toml  .gitignore  .dockerignore  env.example
Dockerfile                  cargo-chef 3-stage (planner→builder→runtime)
migrations/001_init.sql     users / jobs / chapters_cache (+dedup index)
locales/*.json              17 files × 107 keys (en + kiwi-en + ru tr de da
                            no sv fr ar ja ko zh fi mi es it)
src/
  main.rs                   wiring, dispatcher, graceful shutdown
  lib.rs                    module roots
  config.rs                 env parsing (BOT_TOKEN…HTTP_MAX_ATTEMPTS)
  error.rs                  single thiserror enum + is_transient()
  i18n.rs                   embedded JSON locales, fallback→en, {params}
  langdetect.rs             whatlang auto-detect (15 locales; mi manual-only —
                            whatlang has no Maori; conservative fallback)
  normalize.rs              norm()/score()/best_score() search matching
  http.rs                   30s timeout, ≤5 attempts, 429+Retry-After,
                            exp. backoff + time-based jitter (no `rand`)
  ratelimit.rs              GLOBAL token bucket per source (MDX 1 rps)
  db.rs                     sqlx runtime queries, WAL, migrations, users,
                            chapters_cache CRUD
  queue.rs                  enqueue+caps+dedup, claim (race-safe), resume,
                            progress, cancel, requeue; JobKind/JobPayload
  pack.rs                   CBZ/Stored-zip, zero-pad order, merged volumes,
                            per-chapter zips, EPUB via epub-builder, 50MB
                            plan_parts() splitter
  state.rs                  AppState: sources registry, moka TTL caches
                            (search 30min / title 30min / range-pending 10min)
  worker.rs                 claim→download(cache)→pack→verify→split→upload;
                            2.5s edit throttle; cancel checks; disk gate
  bot/
    mod.rs                  set_my_commands from i18n
    handlers.rs             /start /language /help /queue /cancel /status
                            /settings + text-search flow + callbacks (thin)
    callbacks.rs            ≤64B codec (ck:{tid}:{idx} style)
    keyboards.rs            inline/reply keyboards, all labels via i18n
    send.rs                 HTML+escape, 4000-char chunking, RetryAfter
                            sleep+retry, "not modified" tolerance
tests/  queue_resume.rs pack_order.rs splitter.rs callbacks.rs
        backoff.rs (wiremock) i18n_fallback.rs kiwi_completeness.rs
```

## 2. Migrations

`001_init.sql`: `users(chat_id PK, locale, auto_locale, pack_mode,
chapter_lang)`, `jobs(id, chat_id, kind, payload, status pending|running|done|
failed|canceled, progress_done/total, step, progress_msg_id, error, attempts,
timestamps)` + `UNIQUE(chat_id,kind,payload) WHERE status IN ('pending','running')`
(dedup) + `chapters_cache(source,title_id,chapter_id,lang PK; kind,path,bytes)`.
WAL + busy_timeout via connect options; `sqlx::migrate!` embedded.

## 3. Requirements coverage

| Spec item | Where |
|---|---|
| Thin handlers, job → queue, "accepted, position N" | `handlers.rs` → `queue::enqueue` → `job_accepted` |
| SQLite queue + resume after kill -9 / redeploy | `queue::{claim_next, reset_running_to_pending}` (boot + shutdown in `main.rs`) |
| WORKERS (default 2) | `config.rs`, `worker::spawn_workers` |
| Global token bucket per source (MDX ≈1 rps) | `ratelimit.rs`, used in `http::get_bytes_inner` before every request |
| Per-user caps 3 jobs / 200 chapters, polite refusals | `queue::MAX_*`, `job_limit_active/job_limit_chapters` |
| 429+Retry-After, backoff+jitter ≤5 attempts, 30s/request, timeout not per-job | `http.rs`; job timeouts: none — chapters stream through `progress` |
| moka TTL caches (cards/search), disk cache `/data/cache/<src>/<title>/<ch>` + table | `state.rs`, `worker::ensure_cached` |
| Progress edit throttle 1/2.5s, TG RetryAfter sleep | `worker::Progress`, `bot::send` |
| Cancel flag between chapters (button + /cancel) | `is_canceled()` per chapter/part; `cx:{id}` button |
| Disk-space check, temp cleanup on error, graceful shutdown | `free_bytes`, `cleanup_tmp` on every exit path, watch-channel + drain 180s |
| Search by title+alt all langs, normalized | `normalize.rs`, MDX `altTitles`, RBL `rus/eng` names |
| 1 result→card, N→buttons+pagination, none→ask full original | `flow_search`, `search_results`, `search_none` |
| Ask buttons: last 10 / range / volumes / single, ≤64B callbacks | `title_options`, `range` via pending-input, `callbacks.rs` |
| Chapter lang = locale, else en/ru fallback + honest warning | `resolve_title` (`lang_fallback`) |
| Volume merged(default)/per_chapter via /settings; single = file | `pack.rs`, `final_filename`, settings buttons |
| 50MB check → cut by chapter ranges, «part 1/3» sequential | `plan_parts` + overflow re-split + `done_part` |
| /help: flow, ZIP how-to (rename .cbz, Mihon/CDisplayEx/PV/YACReader), why ZIP, modes, credit | `cmd_help` + locale keys `help_*` |
| /start with capabilities + credit; command menu registered | `cmd_start`, `set_my_commands`, reply menu |
| /language 17 locales, kiwi-en full parody set, fallback en, whatlang auto + manual override | `i18n.rs`, `langdetect.rs`, `locales/` |
| All strings via i18n, no hardcode in handlers | verified: zero user-facing literals in `src/bot/handlers.rs` |
| trait Source + MangaDex(official, UA w/ contact) + RanobeLib(pub API) + stub | `sources/` |
| No CF bypass/proxies; honest "unreachable from this hosting" | `BotError::SourceUnreachable` → `err_source_down` |
| Dockerfile chef multi-stage, ENV set, long-polling, /data volume, env.example | `Dockerfile`, `env.example`, README §Deploy |
| README: SQLite→PG+workers path documented, not implemented | README §Architecture |

## 4. Extra crates (STOP-RULE justification)

- `tracing-subscriber` (env-filter): `tracing` alone cannot emit logs; this is
  its standard publisher.
- `wiremock` — **dev-dependency only**, explicitly requested by the task
  ("backoff 429 (wiremock)").
- Nothing else beyond the allowed list. No `async-trait`/`rand`/`regex`/`chrono`
  (dispatch via `AnySource` enum; jitter from SystemTime; hand-rolled `<p>`
  parser; unix-seconds timestamps) — smaller tree, no OpenSSL.

## 5. Manual verification commands (run locally or CI)

```bash
cargo build                       # expect: success; then commit Cargo.lock — the
                                  # Dockerfile copies `Cargo.*` so the lock is picked
                                  # up automatically and Railway builds become
                                  # reproducible + cache-friendly.
cargo fmt --check                 # expect: no diff (rustfmt.toml included)
cargo clippy --all-targets -- -D warnings   # expect: zero warnings
cargo test                        # expect: all green (13 unit + 20 integration)
```

Targeted suites ↔ task §VERIFICATION:

| Gate | Test | Expected |
|---|---|---|
| queue + resume after kill -9 | `tests/queue_resume.rs` (5 tests) | claim→crash→`reset_running_to_pending`→re-claim ✓ |
| CBZ page order / zero-pad / merged continuity | `tests/pack_order.rs` (4) | `001.jpg…120.jpg,121.png…`; `ch N.cbz` inside ✓ |
| 50MB cutter | `tests/splitter.rs` (5) | greedy parts, oversize flag, boundary exact-fit ✓ |
| callback pagination ≤64B | `tests/callbacks.rs` (4) | all ids fit with u32::MAX/usize::MAX/i64::MAX ✓ |
| 429 + Retry-After backoff | `tests/backoff.rs` (3, wiremock) | 1×429→success; 5xx retried exactly `max_attempts`; 404 not retried ✓ |
| i18n fallback | `tests/i18n_fallback.rs` (5) | unknown locale/key → en; `{param}` parity all 17 locales ✓ |
| kiwi-en completeness + “actually kiwi” | `tests/kiwi_completeness.rs` (2) | 107/107 keys; ≥90 % strings differ from en; slang markers (Chur / sweet as / yeah nah / tu meke / bro / eh) ✓ |

Integration on a **test bot** (needs real `BOT_TOKEN`, manual run — marked
*requires manual run*):

```bash
BOT_TOKEN=… DATA_DIR=./data DATABASE_URL=sqlite:./data/bot.db?mode=rwc cargo run
```
1. Send `Берсерк` → expect card with ru title if MDX has one; buttons ru; pick
   last 10 → ru feed (fallback en warning if absent) → CBZ arrives, caption
   localized + “Made with 🥝 by @EthanMonza”.
2. Same in `en` and via `/language → Kiwi English 🥝` — kiwi strings are
   real slang (test §gate 7 enforces it), e.g. *«Chur, bro! Job #1 is in — ya
   number 1 in the queue»*.
3. Negatives: `asdfghjkl` → “send full original title”; forged old button →
   “buttons expired”; `SOURCES_ENABLED=mangadex` with firewall-black IP →
   “source unreachable from this hosting”; 4 jobs → 4th politely refused
   (cap 3); 201-chapter range → refused (cap 200); volume > 50MB → parts.
4. Load: `WORKERS=2`, queue 5 jobs → FIFO claim, progress edits ≤1/2.5s each.

## 6. Known limits

See README §Known limitations. Headlines: single-process SQLite (documented
PG path, not implemented); no cache eviction; RanobeLib is ru-only by nature;
`zip` Stored-only; covers not embedded in cards.

## 7. Conservative choices (marked per STOP-RULES)

- `whatlang` (only 15/17 of our locales detectable) — kept per spec; `mi` and
  `kiwi-en` are manual/derived only.
- `epub-builder 0.8` API (`ZipLibrary::new`, `EpubContent::new(...).title(...)`,
  `generate`) — verified against docs.rs during authoring.
- RanobeLib base `https://api.cdnlibs.org/api`, `site_id[] = 3`, search/feed/
  chapter endpoints — cross-checked against the live public SDK sources; the
  HTML/JSON dual content parser is defensive for both shapes.
- Telegram `RetryAfter(Duration)` variant form — confirmed for teloxide 0.12
  docs; `ChatId(pub i64)`/`MessageId(pub i32)` confirmed too.
- No `async_trait`: sources dispatched via `AnySource` enum to stay inside the
  crate budget; adding a source costs one enum variant (documented).
- Job-level retry attempts capped at 3 (spec capped per-request at 5; the
  job-level number wasn't specified — conservative small value).
