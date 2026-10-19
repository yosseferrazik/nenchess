//! CSRF defense-in-depth for cookie-authenticated requests.
//!
//! Session cookies are `SameSite=Lax`, which already stops cross-site form
//! posts and fetches from carrying them. On top of that, requests that were
//! authenticated *by cookie* (not by `Authorization: Bearer`) must prove
//! they come from our own origin:
//!
//! * `Sec-Fetch-Site` (sent by every modern browser) — `same-origin` and
//!   `none` pass, `same-site` and `cross-site` are rejected;
//! * when absent, `Origin` must be listed in `ALLOWED_ORIGINS` or match the
//!   request `Host` authority;
//! * neither header means a non-browser client, which should be using a
//!   bearer token anyway (bearer requests never reach this check).
//!
//! The WebSocket handshake is a GET, so it is exempt here; `SameSite=Lax`
//! already withholds cookies from cross-site handshake attempts.

use axum::http::{header, HeaderMap, Method};

/// Is a cookie-authenticated `method` request allowed to proceed?
pub fn request_is_safe(method: &Method, headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        return matches!(site, "same-origin" | "none");
    }
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin_allowed(origin, headers, allowed_origins);
    }
    true
}

fn origin_allowed(origin: &str, headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    if allowed_origins
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(origin))
    {
        return true;
    }
    let Some(authority) = origin_authority(origin) else {
        return false;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    authority == host.to_ascii_lowercase()
}

/// `"https://host[:port]/path"` -> `"host[:port]"` (lowercased).
/// Anything without a scheme (or with userinfo) is malformed and rejected.
fn origin_authority(origin: &str) -> Option<String> {
    let (scheme, rest) = origin.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        None
    } else {
        Some(authority.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderName;

    use super::*;

    const ALLOWED: &[&str] = &["http://localhost:5173", "http://localhost:8081"];

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let name = HeaderName::from_bytes(name.as_bytes()).expect("header name");
            map.insert(name, value.parse().expect("header value"));
        }
        map
    }

    fn allowed() -> Vec<String> {
        ALLOWED.iter().map(|o| o.to_string()).collect()
    }

    #[test]
    fn safe_methods_always_pass() {
        let map = headers(&[("sec-fetch-site", "cross-site")]);
        assert!(request_is_safe(&Method::GET, &map, &allowed()));
        assert!(request_is_safe(&Method::HEAD, &map, &allowed()));
        assert!(request_is_safe(&Method::OPTIONS, &map, &allowed()));
    }

    #[test]
    fn fetch_metadata_decides_when_present() {
        for site in ["same-origin", "none"] {
            let map = headers(&[("sec-fetch-site", site)]);
            assert!(request_is_safe(&Method::POST, &map, &allowed()), "{site}");
        }
        for site in ["cross-site", "same-site"] {
            let map = headers(&[("sec-fetch-site", site)]);
            assert!(!request_is_safe(&Method::POST, &map, &allowed()), "{site}");
        }
    }

    #[test]
    fn origin_falls_back_to_allowlist_then_host() {
        // Listed origin passes even when the proxy rewrote the Host header.
        let map = headers(&[
            ("origin", "http://localhost:5173"),
            ("host", "127.0.0.1:8080"),
        ]);
        assert!(request_is_safe(&Method::POST, &map, &allowed()));

        // Same authority as the request host passes without being listed.
        let map = headers(&[
            ("origin", "http://other.example:9000"),
            ("host", "other.example:9000"),
        ]);
        assert!(request_is_safe(&Method::POST, &map, &allowed()));

        // A hostile origin is rejected.
        let map = headers(&[("origin", "https://evil.example"), ("host", "app.example")]);
        assert!(!request_is_safe(&Method::POST, &map, &allowed()));

        // `null` (sandboxed iframe, redirects) never matches.
        let map = headers(&[("origin", "null"), ("host", "app.example")]);
        assert!(!request_is_safe(&Method::POST, &map, &allowed()));
    }

    #[test]
    fn origins_with_credentials_or_junk_do_not_match() {
        let map = headers(&[
            ("origin", "http://user@localhost:5173"),
            ("host", "localhost:5173"),
        ]);
        assert!(!request_is_safe(&Method::POST, &map, &allowed()));
        let map = headers(&[("origin", "localhost:5173"), ("host", "localhost:5173")]);
        assert!(!request_is_safe(&Method::POST, &map, &allowed()));
    }

    #[test]
    fn no_headers_means_non_browser_and_passes() {
        let map = headers(&[]);
        assert!(request_is_safe(&Method::POST, &map, &allowed()));
    }
}
