//! The navigation guard: the webview may only ever show the app's own
//! origin.
//!
//! The app origin is `tauri://localhost` on Windows and Linux and
//! `http://tauri.localhost` on macOS. The dev origin is allowed only when
//! the caller passes it, so a release build never trusts a dev server.

use url::Url;

/// Whether the webview may navigate to `url`.
///
/// `dev_url` is the configured dev server; it is `None` in a release build
/// and in any build that does not run `tauri dev`.
#[must_use]
pub fn navigation_allowed(url: &Url, dev_url: Option<&Url>) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => {
            if url.host_str() == Some("tauri.localhost") {
                return true;
            }
            dev_url.is_some_and(|dev| same_origin(url, dev))
        }
        _ => false,
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("test URL")
    }

    #[test]
    fn only_the_app_origin_and_dev_server_are_allowed() {
        assert!(navigation_allowed(
            &url("http://tauri.localhost/index.html"),
            None
        ));
        assert!(navigation_allowed(
            &url("https://tauri.localhost/index.html"),
            None
        ));
        assert!(navigation_allowed(&url("tauri://localhost/"), None));
        assert!(!navigation_allowed(&url("https://example.com/"), None));
        assert!(!navigation_allowed(&url("file:///C:/Windows/"), None));
        assert!(!navigation_allowed(&url("http://localhost:5180/"), None));
        assert!(navigation_allowed(
            &url("http://localhost:5180/x"),
            Some(&url("http://localhost:5180"))
        ));
    }

    #[test]
    fn a_different_port_is_not_the_dev_origin() {
        assert!(!navigation_allowed(
            &url("http://localhost:5181/x"),
            Some(&url("http://localhost:5180"))
        ));
    }

    #[test]
    fn a_different_scheme_is_not_the_dev_origin() {
        assert!(!navigation_allowed(
            &url("https://localhost:5180/x"),
            Some(&url("http://localhost:5180"))
        ));
    }
}
