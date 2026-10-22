//! Opt-in token-free page for testing in an ordinary local browser
//! (`local_page_without_token`, ADR 033 note of 2026-10-22).
//!
//! When enabled, only GET and HEAD requests for the embedded page and its
//! static assets skip the bearer check, and only when the request names the
//! bound loopback address literally in its `Host` (or HTTP/2 authority).
//! Every `/api/` route and `/openapi.json` still require the instance token,
//! and the page holds no daemon data until the operator enters that token.
//! Requests forwarded under another host name, such as a tailnet name, keep
//! requiring the token on every route.

use std::net::SocketAddr;

use axum::http::{HeaderMap, Method, Uri, header};

#[derive(Debug, thiserror::Error)]
#[error("web_local_page_requires_loopback")]
pub struct LocalPageError;

/// Which requests may load the page without the token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalPageAccess {
    /// Lowercase `host[:port]` values that name the bound listener.
    allowed_hosts: Vec<String>,
}

impl LocalPageAccess {
    /// The access policy for the listener actually bound at `address`.
    pub fn for_listener(address: SocketAddr) -> Result<Self, LocalPageError> {
        if !address.ip().is_loopback() {
            return Err(LocalPageError);
        }
        let port = address.port();
        // `localhost` resolves to 127.0.0.1 or ::1 (the only IPv6 loopback),
        // never to another 127.0.0.0/8 address.
        let (literal, answers_localhost) = match address {
            SocketAddr::V4(v4) => (v4.ip().to_string(), v4.ip().octets() == [127, 0, 0, 1]),
            SocketAddr::V6(v6) => (format!("[{}]", v6.ip()), true),
        };
        let mut names = vec![literal];
        if answers_localhost {
            names.push("localhost".to_owned());
        }
        let mut allowed_hosts = Vec::new();
        for name in names {
            allowed_hosts.push(format!("{name}:{port}"));
            if port == 80 {
                allowed_hosts.push(name);
            }
        }
        Ok(Self { allowed_hosts })
    }

    /// Whether this request may skip the bearer check.
    #[must_use]
    pub fn permits(&self, method: &Method, uri: &Uri, headers: &HeaderMap) -> bool {
        (method == Method::GET || method == Method::HEAD)
            && is_page_path(uri.path())
            && self.names_listener(uri, headers)
    }

    fn names_listener(&self, uri: &Uri, headers: &HeaderMap) -> bool {
        let mut hosts = headers.get_all(header::HOST).iter();
        let host = match (hosts.next(), hosts.next()) {
            (Some(value), None) => value.to_str().ok().map(str::to_owned),
            (None, _) => uri
                .authority()
                .map(|authority| authority.as_str().to_owned()),
            (Some(_), Some(_)) => None,
        };
        host.is_some_and(|host| {
            let host = host.to_ascii_lowercase();
            self.allowed_hosts.contains(&host)
        })
    }
}

/// The page and its assets: everything except the JSON API and its
/// description.
fn is_page_path(path: &str) -> bool {
    path != "/api" && !path.starts_with("/api/") && path != "/openapi.json"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(host: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(host) = host {
            headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        }
        headers
    }

    fn access() -> LocalPageAccess {
        LocalPageAccess::for_listener("127.0.0.1:8420".parse().unwrap()).unwrap()
    }

    #[test]
    fn only_page_reads_addressed_to_the_listener_skip_the_token() {
        let access = access();
        let get = Method::GET;
        for (path, host) in [
            ("/", "127.0.0.1:8420"),
            ("/index.html", "localhost:8420"),
            ("/sw.js", "LOCALHOST:8420"),
            ("/assets/index.js?__WB_REVISION__=1", "127.0.0.1:8420"),
        ] {
            assert!(
                access.permits(&get, &path.parse().unwrap(), &headers(Some(host))),
                "{path} {host}"
            );
        }
        assert!(access.permits(
            &Method::HEAD,
            &"/".parse().unwrap(),
            &headers(Some("127.0.0.1:8420"))
        ));
        for (method, path, host) in [
            (Method::GET, "/api/v1/health", Some("127.0.0.1:8420")),
            (Method::GET, "/api/v1/console", Some("127.0.0.1:8420")),
            (Method::GET, "/api", Some("127.0.0.1:8420")),
            (Method::GET, "/openapi.json", Some("127.0.0.1:8420")),
            (Method::POST, "/", Some("127.0.0.1:8420")),
            (Method::GET, "/", Some("127.0.0.1:9999")),
            (Method::GET, "/", Some("127.0.0.1")),
            (Method::GET, "/", Some("operator.tailnet.example:8420")),
            (Method::GET, "/", Some("operator.tailnet.example")),
            (Method::GET, "/", Some("evil.example")),
            (Method::GET, "/", None),
        ] {
            assert!(
                !access.permits(&method, &path.parse().unwrap(), &headers(host)),
                "{method} {path} {host:?}"
            );
        }
    }

    #[test]
    fn duplicate_hosts_fail_and_http2_authority_is_honored() {
        let access = access();
        let mut duplicate = headers(Some("127.0.0.1:8420"));
        duplicate.append(header::HOST, HeaderValue::from_static("127.0.0.1:8420"));
        assert!(!access.permits(&Method::GET, &"/".parse().unwrap(), &duplicate));
        let authority: Uri = "http://127.0.0.1:8420/".parse().unwrap();
        assert!(access.permits(&Method::GET, &authority, &headers(None)));
        let foreign: Uri = "http://evil.example:8420/".parse().unwrap();
        assert!(!access.permits(&Method::GET, &foreign, &headers(None)));
    }

    #[test]
    fn listener_names_follow_the_bound_address() {
        assert!(LocalPageAccess::for_listener("0.0.0.0:8420".parse().unwrap()).is_err());
        assert!(LocalPageAccess::for_listener("192.168.1.2:8420".parse().unwrap()).is_err());
        let other = LocalPageAccess::for_listener("127.0.0.2:8420".parse().unwrap()).unwrap();
        assert_eq!(other.allowed_hosts, ["127.0.0.2:8420"]);
        let v6 = LocalPageAccess::for_listener("[::1]:8420".parse().unwrap()).unwrap();
        assert_eq!(v6.allowed_hosts, ["[::1]:8420", "localhost:8420"]);
        let port_80 = LocalPageAccess::for_listener("127.0.0.1:80".parse().unwrap()).unwrap();
        assert!(port_80.permits(
            &Method::GET,
            &"/".parse().unwrap(),
            &headers(Some("localhost"))
        ));
    }
}
