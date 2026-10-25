//! A preview can only choose a static audience. Its input has no part-derived fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Card {
    Private,
    Public,
}
impl Card {
    pub fn description(self) -> &'static str {
        match self {
            Self::Private => "Sign in to open",
            Self::Public => "Anyone with the link",
        }
    }
    pub fn png(self) -> &'static [u8] {
        match self {
            Self::Private => include_bytes!("../assets/card-private.png"),
            Self::Public => include_bytes!("../assets/card-public.png"),
        }
    }
    pub fn hash(self) -> String {
        {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(self.png()))
        }
    }
    pub fn tags(self, origin: &str) -> String {
        format!(
            "<meta property=\"og:title\" content=\"partifactory\"><meta property=\"og:description\" content=\"{}\"><meta property=\"og:image\" content=\"{}/og/{}.png\"><meta name=\"twitter:card\" content=\"summary_large_image\">",
            self.description(),
            origin,
            self.hash()
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cards_are_static_1200_by_630_pngs_and_audience_only() {
        for c in [Card::Private, Card::Public] {
            let b = c.png();
            assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(b[16..20].try_into().unwrap()), 1200);
            assert_eq!(u32::from_be_bytes(b[20..24].try_into().unwrap()), 630);
            let tags = c.tags("https://parts.pf.test");
            assert!(!tags.contains("Private title"));
            assert!(tags.contains(c.description()));
            assert!(tags.contains("summary_large_image"));
        }
    }
}
