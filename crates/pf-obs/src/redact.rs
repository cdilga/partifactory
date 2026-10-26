//! Redaction, by type and by pattern (§5d).
//!
//! **By type:** tokens, caps and session ids live in the newtypes below. Their `Debug` and
//! `Display` print `[redacted]`, so `tracing::info!(?cap)` can never leak one. Read the value
//! with `expose()` (grep for it to audit every use). `pf_config::Secret<T>` is the same idea for
//! config secrets.
//!
//! **By pattern:** every log line passes through [`redact_str`] before it reaches stdout (the
//! writer in [`crate::logging`] does it), so a value that slipped past the types is still caught:
//! - `/c/<cap>/` path segments become `/c/[cap]/`;
//! - `pf_at_…`, `pf_rt_…` and `pf_pat_…` tokens become `pf_at_[redacted]` (and so on);
//! - `Bearer <token>` becomes `Bearer [redacted]`;
//! - a `#pf=…` fragment (the share-sheet key in a `Location` value) is stripped.
//!
//! `Authorization`, `Cookie` and `Set-Cookie` are never recorded: the access line has no header
//! fields at all, and [`is_sensitive_header`] is what any later header logging must consult.

use regex::Regex;
use std::borrow::Cow;
use std::fmt;
use std::sync::OnceLock;

/// What a redacted value prints as.
pub const REDACTED: &str = "[redacted]";

macro_rules! redacted_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            /// The only way to read the value.
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(REDACTED)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(REDACTED)
            }
        }
    };
}

redacted_type!(
    /// A part capability (the `<cap>` in `/c/<cap>/`).
    CapToken
);
redacted_type!(
    /// An access token or personal agent token (`pf_at_…`, `pf_pat_…`).
    BearerToken
);
redacted_type!(
    /// A workspace session id.
    SessionId
);
redacted_type!(
    /// An OAuth refresh token (`pf_rt_…`).
    RefreshToken
);

/// Headers whose values are never recorded anywhere.
pub fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "set-cookie" | "proxy-authorization"
    )
}

struct Patterns {
    cap: Regex,
    token: Regex,
    bearer: Regex,
    fragment: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // A cap segment runs to the next `/`, whitespace, quote, `?`, `#` or backslash (a JSON
        // escape), so the whole value goes whatever alphabet it uses. A run of `/c` segments is
        // taken whole, so `/c/c/<cap>` can't hide the cap behind a decoy `c` segment.
        cap: Regex::new(r#"/c(?:/c)*/[^/\s"'?#\\]+"#).expect("cap regex"),
        token: Regex::new(r"pf_(at|rt|pat)_[A-Za-z0-9_\-.~+/=]+").expect("token regex"),
        bearer: Regex::new(r"(?i)\bbearer\s+[^\s\x22\\]+").expect("bearer regex"),
        fragment: Regex::new(r#"#pf=[^\s"'\\]*"#).expect("fragment regex"),
    })
}

/// Apply the pattern layer to one string (a path, a message or a whole log line).
pub fn redact_str(s: &str) -> Cow<'_, str> {
    let p = patterns();
    let mut out = Cow::Borrowed(s);
    if p.cap.is_match(&out) {
        out = Cow::Owned(p.cap.replace_all(&out, "/c/[cap]").into_owned());
    }
    if p.token.is_match(&out) {
        out = Cow::Owned(p.token.replace_all(&out, "pf_${1}_[redacted]").into_owned());
    }
    if p.bearer.is_match(&out) {
        out = Cow::Owned(p.bearer.replace_all(&out, "Bearer [redacted]").into_owned());
    }
    if p.fragment.is_match(&out) {
        out = Cow::Owned(p.fragment.replace_all(&out, "").into_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn types_print_redacted() {
        let c = CapToken::new("secretcap");
        let b = BearerToken::new("pf_at_abc");
        let s = SessionId::new("sess");
        let r = RefreshToken::new("pf_rt_x");
        let all = format!("{c:?} {c} {b:?} {b} {s:?} {r}");
        assert!(!all.contains("secretcap") && !all.contains("abc") && !all.contains("sess"));
        assert_eq!(c.expose(), "secretcap");
    }

    #[test]
    fn cap_path_redacted() {
        assert_eq!(redact_str("/c/AbC-123_x/index.html"), "/c/[cap]/index.html");
        assert_eq!(redact_str("GET /c/zzz"), "GET /c/[cap]");
        assert_eq!(redact_str("/p/name"), "/p/name");
    }

    #[test]
    fn tokens_redacted() {
        assert_eq!(redact_str("tok=pf_at_abcDEF123"), "tok=pf_at_[redacted]");
        assert_eq!(
            redact_str("pf_rt_xyz pf_pat_q"),
            "pf_rt_[redacted] pf_pat_[redacted]"
        );
        assert_eq!(
            redact_str("Authorization: Bearer abc.def"),
            "Authorization: Bearer [redacted]"
        );
    }

    #[test]
    fn location_fragment_stripped() {
        assert_eq!(
            redact_str(r#"{"location":"https://x-part.pf.test/#pf=KEY123"}"#),
            r#"{"location":"https://x-part.pf.test/"}"#
        );
    }

    #[test]
    fn sensitive_headers() {
        assert!(is_sensitive_header("Cookie"));
        assert!(is_sensitive_header("set-cookie"));
        assert!(is_sensitive_header("AUTHORIZATION"));
        assert!(!is_sensitive_header("x-request-id"));
    }

    proptest! {
        #[test]
        fn random_caps_never_survive(
            pre in ".{0,40}",
            post in ".{0,40}",
            cap in "[A-Za-z0-9_-]{16,64}",
        ) {
            let line = format!("{pre}/c/{cap}/{post}");
            let out = redact_str(&line);
            prop_assert!(!out.contains(&cap), "cap survived: {out}");
        }

        #[test]
        fn random_tokens_never_survive(
            pre in ".{0,40}",
            post in "[ ,;\"]{1}.{0,40}",
            kind in prop::sample::select(vec!["at", "rt", "pat"]),
            tok in "[A-Za-z0-9_-]{16,64}",
        ) {
            let line = format!("{pre}pf_{kind}_{tok}{post}");
            let out = redact_str(&line);
            prop_assert!(!out.contains(&tok), "token survived: {out}");
        }

        #[test]
        fn json_escaped_lines_redacted(cap in "[A-Za-z0-9]{16,40}") {
            let line = serde_json::json!({"path": format!("/c/{cap}/a"), "msg": format!("pf_at_{cap}")}).to_string();
            let out = redact_str(&line);
            prop_assert!(!out.contains(&cap));
        }
    }
}
