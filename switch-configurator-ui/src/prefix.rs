//! Serve the UI under a reverse-proxy sub-path (e.g. `/switch-configurator/`).
//!
//! The templates emit root-absolute links (`/switch/..`, `/static/..`), which
//! would escape the sub-path. The proxy strips the prefix before forwarding and
//! advertises it in `X-Forwarded-Prefix` (same contract CoNexus uses); this
//! middleware puts it back on every HTML link and redirect in one place instead
//! of threading a `base` value through every template struct.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

pub const PREFIX_HEADER: &str = "x-forwarded-prefix";

/// Attributes whose root-absolute value must gain the prefix.
const URL_ATTRS: [&str; 6] = ["href", "src", "action", "hx-get", "hx-post", "hx-delete"];

/// Largest HTML body we are willing to buffer for rewriting.
const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;

/// Accept only `/seg[/seg...]` made of URL-safe characters. The header is
/// written into HTML, so anything else is ignored rather than escaped.
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

/// Prefix a root-absolute path; leave anything else (relative, `//host`,
/// absolute URLs) alone.
pub fn prefix_location(location: &str, prefix: &str) -> String {
    if location.starts_with('/') && !location.starts_with("//") {
        format!("{prefix}{location}")
    } else {
        location.to_string()
    }
}

/// Rewrite root-absolute URL attributes in an HTML document and expose the
/// prefix to scripts through `<meta name="base-path">`.
pub fn rewrite_html(html: &str, prefix: &str) -> String {
    let mut out = html.to_string();
    for attr in URL_ATTRS {
        out = out.replace(&format!("{attr}=\"/"), &format!("{attr}=\"{prefix}/"));
        out = out.replace(&format!("{attr}='/"), &format!("{attr}='{prefix}/"));
    }
    // `//host/..` is protocol-relative, not ours: undo the rewrite for it.
    out = out.replace(&format!("=\"{prefix}//"), "=\"//");
    if let Some(pos) = out.find("<head>") {
        let at = pos + "<head>".len();
        out.insert_str(at, &format!("<meta name=\"base-path\" content=\"{prefix}\">"));
    }
    out
}

pub async fn apply(req: Request, next: Next) -> Response {
    let prefix = req
        .headers()
        .get(PREFIX_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_prefix);
    let resp = next.run(req).await;
    let Some(prefix) = prefix else { return resp };

    let (mut parts, body) = resp.into_parts();
    for name in [header::LOCATION.as_str(), "hx-redirect", "hx-location"] {
        if let Some(value) = parts.headers.get(name).and_then(|v| v.to_str().ok()) {
            if let Ok(new) = HeaderValue::from_str(&prefix_location(value, &prefix)) {
                parts.headers.insert(
                    axum::http::HeaderName::from_bytes(name.as_bytes()).expect("static name"),
                    new,
                );
            }
        }
    }

    let is_html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if !is_html {
        return Response::from_parts(parts, body);
    }
    match to_bytes(body, MAX_HTML_BYTES).await {
        Ok(bytes) => {
            let html = String::from_utf8_lossy(&bytes);
            let rewritten = rewrite_html(&html, &prefix);
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(rewritten))
        }
        // Over the limit or a broken body: failing loudly beats serving links
        // that silently point outside the prefix.
        Err(e) => {
            tracing::error!("could not buffer HTML response for prefix rewriting: {e}");
            let mut resp = Response::new(Body::from("response too large to serve under a prefix"));
            *resp.status_mut() = axum::http::StatusCode::BAD_GATEWAY;
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_plain_paths_and_trims_trailing_slash() {
        assert_eq!(sanitize_prefix("/switch-configurator"), Some("/switch-configurator".into()));
        assert_eq!(sanitize_prefix("/a/b/"), Some("/a/b".into()));
    }

    #[test]
    fn sanitize_rejects_empty_root_and_anything_that_could_inject_html() {
        for bad in ["", "/", "x", "//evil.example", "/a\"><script>", "/a b", "/../x", "/a//b", "/a?x=1"] {
            assert_eq!(sanitize_prefix(bad), None, "{bad:?} must be rejected");
        }
    }

    #[test]
    fn location_gains_the_prefix_only_when_root_absolute() {
        assert_eq!(prefix_location("/switch/sw1", "/p"), "/p/switch/sw1");
        assert_eq!(prefix_location("//evil.example/x", "/p"), "//evil.example/x");
        assert_eq!(prefix_location("https://x.example/", "/p"), "https://x.example/");
        assert_eq!(prefix_location("relative", "/p"), "relative");
    }

    #[test]
    fn html_links_forms_htmx_and_assets_gain_the_prefix() {
        let html = "<head><link href=\"/static/style.css\"></head><a href=\"/switch/s1\">x</a>\
            <form action=\"/draft/s1/ports\"></form><span hx-post=\"/preview/s1\" hx-get='/preview/s1/yaml'></span>\
            <script src=\"/static/htmx.min.js\"></script>";
        let out = rewrite_html(html, "/p");
        for want in [
            "href=\"/p/static/style.css\"",
            "href=\"/p/switch/s1\"",
            "action=\"/p/draft/s1/ports\"",
            "hx-post=\"/p/preview/s1\"",
            "hx-get='/p/preview/s1/yaml'",
            "src=\"/p/static/htmx.min.js\"",
        ] {
            assert!(out.contains(want), "missing {want} in {out}");
        }
    }

    #[test]
    fn html_leaves_external_relative_and_protocol_relative_links_alone() {
        let out = rewrite_html(
            "<head></head><a href=\"https://x.example/\">a</a><a href=\"#top\">b</a><a href=\"//cdn.example/x\">c</a><a href=\"rel\">d</a>",
            "/p",
        );
        for want in ["href=\"https://x.example/\"", "href=\"#top\"", "href=\"//cdn.example/x\"", "href=\"rel\""] {
            assert!(out.contains(want), "{want} was changed: {out}");
        }
    }

    #[test]
    fn html_exposes_the_prefix_to_scripts_through_a_meta_tag() {
        let out = rewrite_html("<html><head><title>t</title></head></html>", "/p");
        assert!(out.contains("<head><meta name=\"base-path\" content=\"/p\">"), "{out}");
    }
}
