# ADR-0001: Cache metadata via standard HTTP caching headers

## Status

Accepted

## Context

`GET /switches/:id/mac-table` added a per-switch TTL cache (default 10s)
to avoid re-opening a session on a switch's exclusive serial device for
every poll — a live fetch takes ~11-13s on FortiSwitch and competes with
every other operational request for the same busy-flag lock.

The first cut exposed cache facts as bespoke JSON body fields
(`cached`, `cache_age_seconds`). That's not enough for a caller to reason
about staleness confidently, and it isn't enough on its own:

- **Age alone doesn't say when to expect fresh data.** A caller polling on
  its own schedule needs to know the server's TTL to decide whether
  re-requesting sooner is pointless (same cache entry) or worthwhile (about
  to expire).
- **There's no way to detect "did this actually get refreshed" without
  diffing the full payload.** Comparing `entries`/`raw_output`
  byte-for-byte is the only way to notice a change with plain fields —
  expensive, and indistinguishable from "no refresh happened yet" when a
  refresh happens to return identical data (e.g. nothing changed on the
  switch).

This is a real, recurring need — any future endpoint that adds its own
cache hits the same two gaps — so it needs a project-wide answer, not a
one-off shape on `mac-table`.

Before inventing a custom JSON contract for this, the obvious question:
does a standard already solve it? **Yes** — this is exactly what HTTP's
caching model (RFC 9111) and conditional-request model (RFC 9110 §13)
already define:

| Need | HTTP standard mechanism |
|---|---|
| How old the data is | `Age` response header (seconds) |
| When the cache is expected to refresh | `Cache-Control: max-age=N` |
| An id that changes only when the cache actually updates | `ETag` — an opaque validator |
| When it was last actually fetched | `Last-Modified` header |
| Cheap "did anything change" check | `If-None-Match: <etag>` request header → `304 Not Modified` |

Using the standard means every existing HTTP-aware tool (`curl -I`,
browser devtools, reverse proxies, monitoring, any HTTP client library)
already understands the response without reading this project's docs —
and it gets conditional requests (`304`, no body) for free, which a custom
JSON field can never provide.

## Decision

Every API response that can be served from a cache sets these response
headers, whether the request was a cache hit or triggered a live fetch:

- **`ETag`** — a quoted opaque validator, `"<generation-uuid>"`. A fresh
  UUID v4 is generated every time the underlying cache entry is actually
  written (every live fetch, never on a cache hit or a `304`). Two
  responses sharing the same `ETag` are guaranteed to carry the same
  underlying snapshot; a different `ETag` means a refresh happened, even
  if the payload content is byte-identical to the previous fetch.
- **`Age`** — seconds elapsed since the cached data was fetched
  (`now - fetched_at`), per RFC 9111 §5.1. `0` on a fresh live fetch.
- **`Cache-Control: max-age=N`** — the freshness lifetime in seconds
  (the endpoint's TTL, or the caller's `?max_age_seconds=` override — see
  below). Combined with `Age` by any HTTP-aware client to compute
  remaining freshness (`max-age - Age`) without needing to know the
  server's TTL out of band. `Expires` is intentionally not also sent —
  `Cache-Control: max-age` takes precedence over it per spec, so sending
  both is redundant.
- **`Last-Modified`** — the absolute fetch timestamp, IMF-fixdate format
  (`fetched_at`), per RFC 9110 §8.8.2.

**Conditional requests**: a request carrying `If-None-Match: "<etag>"`
that matches the current (still-fresh) cache entry's `ETag` gets
`304 Not Modified` with the same header set and **no response body** —
the standard way to let a caller confirm "still the same data" without
re-transferring `entries`/`raw_output`. A `?fresh=true` request (the
endpoint's own escape hatch to force a live fetch) always does real work
and never short-circuits to a `304`, even if `If-None-Match` happens to
match — `fresh=true` means "I explicitly do not want the cache," which
conditional-request short-circuiting would defeat.

The JSON response body carries only the endpoint-specific data
(`switch_id`, `entries`, `raw_output`) — no duplicate cache bookkeeping
fields in the body. Cache facts live in headers, where the standard
already put them.

## Consequences

- Every future cached endpoint reuses the same four headers and the same
  `If-None-Match` → `304` handling — no bespoke per-endpoint cache-shape
  decisions, and no custom "how do I check if this refreshed" logic to
  write client-side beyond standard `ETag` comparison.
- The cache-storage type backing any such endpoint must carry a
  `generation_id: Uuid` alongside its `fetched_at`, generated at write
  time (see `CachedMacTable` in `src/config.rs` for the reference
  implementation).
- Any endpoint-specific cache-bypass/TTL-override query parameters (e.g.
  `mac-table`'s `?fresh=true` and `?max_age_seconds=N`) are additive to
  this header contract, not a replacement for it — the response still
  reports accurate `Age`/`Cache-Control`/`ETag`/`Last-Modified` for
  whatever was actually served.
- `mac-table` (the only endpoint with a cache today) implements this
  contract — see the corresponding `docs/reference/api.md` section and
  `CHANGELOG.md` entry.
- A cached entry is only ever *replaced* (on refresh) or *removed*
  (when its switch is deleted or a full config reload drops it) — nothing
  proactively expires an entry that's simply gone unused. This is
  intentional: switch counts are small and bounded (not a user-controlled
  cache key space), so an unused entry sitting in memory costs nothing
  worth actively sweeping for.
- Any endpoint that wants callers to reliably hit a warm cache (rather than
  the first request after expiry always paying the live-fetch cost) should
  run a background refresher on the same TTL, using the identical
  `try_acquire_configuring` guard every other operational endpoint uses —
  see `mac-table`'s `spawn_mac_table_refresher` for the reference
  implementation. A skipped refresh cycle (switch busy with something
  else) must never block or error; it just leaves that one cycle's
  refresh to the next caller, exactly as if the refresher didn't exist.
