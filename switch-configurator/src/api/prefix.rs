//! Make the JSON API's self-referencing paths correct behind a reverse proxy
//! that mounts it under a sub-path.
//!
//! The proxy strips the prefix and advertises it in `X-Forwarded-Prefix` (the
//! contract CoNexus and the web UI use). The API only ever writes its own paths
//! in two places: the `poll_url` of the 202 responses and `api.endpoints` of
//! `/api/status`. Those are rewritten here; every other field is untouched.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;

pub const PREFIX_HEADER: &str = "x-forwarded-prefix";
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;

/// Accept only `/seg[/seg...]` of URL-safe characters; anything else is ignored.
/// (Same rule as the web UI's `prefix.rs`; the two crates share no code.)
pub fn sanitize_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() || !trimmed.starts_with('/') {
        return None;
    }
    let ok = trimmed[1..].split('/').all(|seg| {
        !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
    });
    ok.then(|| trimmed.to_string())
}

fn prefixed(path: &str, prefix: &str) -> String {
    if path.starts_with('/') && !path.starts_with("//") {
        format!("{prefix}{path}")
    } else {
        path.to_string()
    }
}

/// `"GET /switches/:id"` -> `"GET /p/switches/:id"`.
fn prefixed_endpoint(entry: &str, prefix: &str) -> String {
    match entry.split_once(' ') {
        Some((method, path)) => format!("{method} {}", prefixed(path, prefix)),
        None => entry.to_string(),
    }
}

pub fn rewrite_json(value: &mut Value, prefix: &str) {
    if let Some(Value::String(url)) = value.get_mut("poll_url") {
        *url = prefixed(url, prefix);
    }
    if let Some(Value::Array(endpoints)) = value.pointer_mut("/api/endpoints") {
        for entry in endpoints {
            if let Value::String(s) = entry {
                *s = prefixed_endpoint(s, prefix);
            }
        }
    }
}

pub async fn apply(req: Request, next: Next) -> Response {
    let prefix = req
        .headers()
        .get(PREFIX_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_prefix);
    let resp = next.run(req).await;
    let Some(prefix) = prefix else { return resp };

    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    match to_bytes(body, MAX_JSON_BYTES).await {
        Ok(bytes) => {
            let out = match serde_json::from_slice::<Value>(&bytes) {
                Ok(mut v) => {
                    rewrite_json(&mut v, &prefix);
                    serde_json::to_vec(&v).unwrap_or_else(|_| bytes.to_vec())
                }
                // Not our JSON shape: pass it through unchanged.
                Err(_) => bytes.to_vec(),
            };
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(out))
        }
        Err(e) => {
            tracing::error!("could not buffer JSON response for prefix rewriting: {e}");
            let mut resp = Response::new(Body::from("response too large to serve under a prefix"));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_accepts_paths_and_rejects_anything_unsafe() {
        assert_eq!(sanitize_prefix("/api-x/"), Some("/api-x".into()));
        for bad in ["", "/", "x", "//e.example", "/a b", "/../x", "/a\"b", "/a?x"] {
            assert_eq!(sanitize_prefix(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn poll_url_and_endpoint_list_gain_the_prefix_and_nothing_else_changes() {
        let mut v = json!({
            "poll_url": "/api/status",
            "message": "Poll /api/status",
            "api": {"port": 4000, "endpoints": ["GET /api/status", "POST /switches/:id/apply"]},
        });
        rewrite_json(&mut v, "/p");
        assert_eq!(v["poll_url"], "/p/api/status");
        assert_eq!(v["api"]["endpoints"][0], "GET /p/api/status");
        assert_eq!(v["api"]["endpoints"][1], "POST /p/switches/:id/apply");
        assert_eq!(v["message"], "Poll /api/status");
        assert_eq!(v["api"]["port"], 4000);
    }
}
