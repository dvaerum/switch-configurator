# API Identifier Migration: hostname → id

## Overview

This document analyzes the migration from using `hostname` to `id` as the switch identifier in API endpoints.

**Current Endpoints:**
- `POST /switches/{hostname}/apply`
- `GET /switches/{hostname}/config`

**Proposed Endpoints:**
- `POST /switches/{id}/apply`
- `GET /switches/{id}/config`

---

## Motivation

### Why Switch from `hostname` to `id`?

1. **Required vs Optional Field**
   - `id` is required (guaranteed present)
   - `hostname` is `Option<String>` (could theoretically be None after merge)

2. **Simpler Lookup Logic**
   - Current: `s.hostname.as_ref().map(|h| h.as_str()) == Some(hostname.as_str())`
   - Proposed: `s.id == id`

3. **Guaranteed Uniqueness**
   - `id` is the primary key for switch identity in multi-config merging
   - `hostname` could theoretically have conflicts (though validated)

4. **Stability**
   - `id` is designed as the stable identifier
   - `hostname` could change (e.g., renaming a switch)

5. **Consistency with Internal Architecture**
   - Config merging uses `id` as the key
   - Status tracking uses `id` or hostname interchangeably
   - Using `id` aligns API with internal data model

---

## Required Changes

### 1. Route Definitions (src/api/server.rs)

**Current:**
```rust
.route("/switches/:hostname/apply", post(handlers::apply_config))
.route("/switches/:hostname/config", get(handlers::get_config))
```

**Change to:**
```rust
.route("/switches/:id/apply", post(handlers::apply_config))
.route("/switches/:id/config", get(handlers::get_config))
```

**Impact:** Minimal - just parameter name change in route definition

---

### 2. Handler Function Signatures (src/api/handlers.rs)

#### apply_config Handler

**Current (line 45-47):**
```rust
pub async fn apply_config(
    State(store): State<ConfigStore>,
    Path(hostname): Path<String>,
) -> impl IntoResponse {
```

**Change to:**
```rust
pub async fn apply_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
```

**Impact:** Parameter rename throughout function

---

#### get_config Handler

**Current (line 171-173):**
```rust
pub async fn get_config(
    State(store): State<ConfigStore>,
    Path(hostname): Path<String>,
) -> impl IntoResponse {
```

**Change to:**
```rust
pub async fn get_config(
    State(store): State<ConfigStore>,
    Path(id): Path<String>,
) -> impl IntoResponse {
```

**Impact:** Parameter rename throughout function

---

### 3. Switch Lookup Logic

#### apply_config Switch Lookup

**Current (line 51-72):**
```rust
let switch_config = match config
    .switches
    .iter()
    .find(|s| s.hostname.as_ref().map(|h| h.as_str()) == Some(hostname.as_str()))
{
    Some(cfg) => cfg.clone(),
    None => {
        // Record error
        store.status.record_error(
            "NotFound".to_string(),
            format!("Switch '{}' not found", hostname),
            Some(hostname.clone()),
            "apply_config".to_string(),
        ).await;

        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Switch '{}' not found", hostname)})),
        )
            .into_response();
    }
};
```

**Change to:**
```rust
let switch_config = match config
    .switches
    .iter()
    .find(|s| s.id == id)
{
    Some(cfg) => cfg.clone(),
    None => {
        // Record error
        store.status.record_error(
            "NotFound".to_string(),
            format!("Switch with id '{}' not found", id),
            Some(id.clone()),
            "apply_config".to_string(),
        ).await;

        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Switch with id '{}' not found", id)})),
        )
            .into_response();
    }
};
```

**Benefits:**
- Simpler comparison (no Option unwrapping)
- Direct string equality
- More performant (avoids Option overhead)

#### get_config Switch Lookup

**Current (line 177-197):**
```rust
let switch_config = match config
    .switches
    .iter()
    .find(|s| s.hostname.as_ref().map(|h| h.as_str()) == Some(hostname.as_str()))
{
    // ... similar to apply_config
};
```

**Change to:**
```rust
let switch_config = match config
    .switches
    .iter()
    .find(|s| s.id == id)
{
    // ... updated error messages
};
```

---

### 4. Logging and Status Messages

**Changes needed in:**

1. **Info logging** (line 77, 162):
   ```rust
   // Current
   info!("Applying configuration to switch: {}", hostname);
   info!("Successfully applied configuration to {}", hostname);

   // Change to
   info!("Applying configuration to switch: {} ({})", id, switch_config.hostname.as_ref().unwrap_or(&id));
   info!("Successfully applied configuration to {} ({})", id, switch_config.hostname.as_ref().unwrap_or(&id));
   ```

2. **Status tracking** (lines 59, 86, 107, etc.):
   ```rust
   // Current
   store.status.record_error(..., Some(hostname.clone()), ...);

   // Change to
   store.status.record_apply_failure(&id, ...);
   store.status.record_error(..., Some(id.clone()), ...);
   ```

**Decision:** Should status tracking use `id` or `hostname`?
- **Recommendation:** Use `id` for consistency
- Status tracker should be updated to use `id` as primary identifier
- Can include hostname in error messages for human readability

---

### 5. Response Body Changes

#### get_config Response

**Current (line 241-247):**
```rust
Json(json!({
    "hostname": hostname,
    "config": running_config
}))
```

**Options:**

**Option A - Include both id and hostname:**
```rust
Json(json!({
    "id": switch_config.id,
    "hostname": switch_config.hostname,
    "config": running_config
}))
```

**Option B - Use id only:**
```rust
Json(json!({
    "id": id,
    "config": running_config
}))
```

**Recommendation:** Option A - provides both for client convenience

---

## Summary of Changes

### Files to Modify

1. **src/api/server.rs** (2 lines)
   - Update route parameter from `:hostname` to `:id`

2. **src/api/handlers.rs** (~40 lines)
   - Update `apply_config` function signature and body
   - Update `get_config` function signature and body
   - Change switch lookup from hostname to id
   - Update all error messages
   - Update logging statements
   - Update response bodies

3. **docs/reference/api.md** (~15 locations)
   - Update endpoint documentation
   - Update path parameter descriptions
   - Update examples
   - Update error messages

4. **tests/api/** (if exist)
   - Update test cases to use `id` instead of `hostname`

---

## Breaking Change Analysis

### Impact Level: **HIGH**

This is a **breaking change** for API consumers:

1. **Existing API clients will break** - URLs must be updated
2. **Error messages change** - monitoring/alerting may need updates
3. **Response format changes** (if we include both id and hostname)

### Migration Strategy

#### Option 1: Big Bang Migration (Breaking Change)

**Pros:**
- Clean, simple implementation
- No legacy code to maintain
- Forces clients to update to correct identifier

**Cons:**
- Breaks all existing clients immediately
- Requires coordinated deployment

**Timeline:**
- Development: 1-2 hours
- Testing: 30 minutes
- Deployment: Coordinated with client updates

---

#### Option 2: Graceful Migration (Backward Compatible)

Support **both** identifiers during transition period:

```rust
.route("/switches/:identifier/apply", post(handlers::apply_config))
.route("/switches/:identifier/config", get(handlers::get_config))
```

```rust
pub async fn apply_config(
    State(store): State<ConfigStore>,
    Path(identifier): Path<String>,
) -> impl IntoResponse {
    let config = store.config.read().await;

    // Try lookup by id first (preferred)
    let switch_config = config
        .switches
        .iter()
        .find(|s| s.id == identifier)
        .or_else(|| {
            // Fall back to hostname lookup (deprecated)
            config
                .switches
                .iter()
                .find(|s| s.hostname.as_ref().map(|h| h.as_str()) == Some(identifier.as_str()))
        });

    match switch_config {
        Some(cfg) => {
            // Warn if using hostname
            if cfg.hostname.as_ref() == Some(&identifier) && cfg.id != identifier {
                tracing::warn!("API called with hostname '{}' - please migrate to using id '{}'", identifier, cfg.id);
            }
            // ... continue with configuration
        }
        None => {
            // Not found by id or hostname
            // ... error handling
        }
    }
}
```

**Pros:**
- Zero-downtime migration
- Existing clients continue working
- Can phase migration over weeks/months
- Can add deprecation warnings in logs

**Cons:**
- More complex code
- Need to maintain both paths
- Requires eventual cleanup phase
- Potential confusion about which to use

**Timeline:**
- Development: 3-4 hours
- Testing: 1 hour
- Deployment: Immediate, no coordination needed
- Deprecation period: 3-6 months
- Cleanup: Remove hostname support after deprecation

---

#### Option 3: Versioned API (Most Future-Proof)

Create a new API version:

```rust
// v1 - existing (deprecated)
.route("/v1/switches/:hostname/apply", post(handlers::v1_apply_config))
.route("/v1/switches/:hostname/config", get(handlers::v1_get_config))

// v2 - new (recommended)
.route("/v2/switches/:id/apply", post(handlers::v2_apply_config))
.route("/v2/switches/:id/config", get(handlers::v2_get_config))

// Default to v2
.route("/switches/:id/apply", post(handlers::v2_apply_config))
.route("/switches/:id/config", get(handlers::v2_get_config))
```

**Pros:**
- Crystal clear migration path
- Can evolve API independently
- Standard versioning practice
- Can eventually sunset v1

**Cons:**
- Most code to maintain initially
- Need to decide on versioning strategy
- More complex router setup

**Timeline:**
- Development: 4-6 hours
- Testing: 1-2 hours
- Deployment: Immediate
- v1 support: 6-12 months
- Cleanup: Remove v1 after sunset

---

## Recommendation

### For Internal/Controlled Deployment: **Option 1 (Breaking Change)**

If you control all API clients:
- Simpler implementation
- Forces immediate migration to correct pattern
- Cleaner codebase

**Implementation Steps:**
1. Update code (1-2 hours)
2. Update documentation
3. Notify all API consumers
4. Deploy simultaneously with client updates
5. Update monitoring/alerting

---

### For External/Production API: **Option 2 (Graceful Migration)**

If the API has external consumers or unknown clients:
- Zero downtime
- Gradual migration
- Add deprecation warnings

**Implementation Steps:**
1. Implement dual-lookup logic (3-4 hours)
2. Add deprecation warnings in logs
3. Update documentation with migration guide
4. Deploy immediately
5. Monitor for hostname usage (via logs)
6. After 3-6 months, remove hostname support
7. Clean up lookup code

---

## Testing Checklist

After implementing changes:

- [ ] GET /switches returns correct data with `id` field
- [ ] POST /switches/{id}/apply works with valid id
- [ ] POST /switches/{id}/apply returns 404 with invalid id
- [ ] GET /switches/{id}/config works with valid id
- [ ] GET /switches/{id}/config returns 404 with invalid id
- [ ] Error messages reference "id" not "hostname"
- [ ] Logging shows correct identifiers
- [ ] Status tracking records correct identifiers
- [ ] Multi-config merge still works (id is the key)
- [ ] API documentation updated
- [ ] Examples in docs use correct endpoint format

**If using graceful migration:**
- [ ] Lookup by id works (preferred path)
- [ ] Lookup by hostname works (fallback path)
- [ ] Deprecation warning logged when using hostname
- [ ] Response includes both id and hostname
- [ ] Monitoring can track hostname vs id usage

---

## Code Changes Summary

### Minimal Changes (Breaking Change Approach)

**Files:** 2 modified
**Lines:** ~50 changes

1. src/api/server.rs: 2 lines
2. src/api/handlers.rs: ~45 lines

**Estimated Time:** 2 hours including testing

---

### Full Changes (Graceful Migration Approach)

**Files:** 2 modified
**Lines:** ~80 changes

1. src/api/server.rs: 2 lines
2. src/api/handlers.rs: ~75 lines (dual lookup logic)

**Estimated Time:** 4 hours including testing

---

## Conclusion

**Recommendation:** Implement **Option 2 (Graceful Migration)** to maintain backward compatibility while encouraging migration to the correct identifier.

The `id` field is the proper identifier for switches in the system architecture, and migrating the API to use it aligns with the internal data model and multi-config merge system design.
