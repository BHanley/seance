//! "Suggest a Change…": opens a new feature request in the default browser,
//! with the running version already in the body.

const NEW_ISSUE: &str = "https://github.com/BHanley/seance-requests/issues/new";

/// Everything but letters, digits and `-._~` as %XX (UTF-8 bytes).
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The feature-request body with the version line filled in.
pub fn body(version: &str) -> String {
    format!("**Seance version** (Seance > About Seance)\n{version}\n")
}

/// The URL the menu item opens.
pub fn url(version: &str) -> String {
    format!(
        "{NEW_ISSUE}?template=feature-request.md&body={}",
        encode(&body(version))
    )
}

/// Open it.
pub fn open() {
    crate::sysopen::open_detached(&url(env!("CARGO_PKG_VERSION")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_names_the_template_and_carries_the_version_encoded() {
        let u = url("0.28.17");
        assert!(u.starts_with(
            "https://github.com/BHanley/seance-requests/issues/new?template=feature-request.md&body="
        ));
        assert!(u.ends_with("0.28.17%0A"), "{u}");
        assert!(u.contains("%2A%2ASeance%20version%2A%2A%20%28Seance%20%3E%20About%20Seance%29%0A"));
        assert!(!u.contains(' ') && !u.contains('\n') && !u.contains('*'));
    }

    #[test]
    fn encoding_keeps_safe_characters_and_escapes_the_rest() {
        assert_eq!(encode("a-b_c.d~E9"), "a-b_c.d~E9");
        assert_eq!(encode("a b&c=d#é"), "a%20b%26c%3Dd%23%C3%A9");
    }
}
