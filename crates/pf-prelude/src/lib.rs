//! The prelude script injected into every part document, and the `/_rt` client (PLAN §2).
//!
//! - [`PRELUDE_JS`] is seeded into `shared_blobs` at start and served from `blobs-part` as the
//!   first `<script>` of every part document. Changing it changes its hash, so a fix ships with a
//!   deploy and needs no rebuild of any part.
//! - [`WRAPPER_JS`] is the minimal part-host wrapper script (the hand-off and the frame).
//! - The message schema between the prelude, the wrapper and `/_rt` is `PROTOCOL.md`.

/// The prelude, served as a shared blob.
pub const PRELUDE_JS: &str = include_str!("../js/prelude.js");

/// The part-host wrapper script, served as a shared blob.
pub const WRAPPER_JS: &str = include_str!("../js/wrapper.js");
pub const WRAPPER_BR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wrapper.js.br"));

/// The formatted-view editor (ProseMirror, decision 54), bundled from `editor/` by `editor/build.mjs`
/// and loaded by the wrapper on the first Edit of a Markdown part.
pub const EDITOR_JS: &str = include_str!("../js/editor.js");
pub const EDITOR_BR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/editor.js.br"));
/// The `shared_blobs.label` of the editor.
pub const EDITOR_LABEL: &str = "pf-editor.js";

/// The `shared_blobs.label` of the prelude.
pub const PRELUDE_LABEL: &str = "pf-prelude.js";
/// The `shared_blobs.label` of the wrapper script.
pub const WRAPPER_LABEL: &str = "pf-wrapper.js";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn brotli_len(s: &str) -> usize {
        let mut out = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
            w.write_all(s.as_bytes()).unwrap();
        }
        out.len()
    }

    #[test]
    fn prelude_is_at_most_12_kib_brotli() {
        let n = brotli_len(PRELUDE_JS);
        assert!(n <= 12 * 1024, "prelude is {n} bytes brotli");
    }

    #[test]
    fn scripts_parse_when_node_is_present() {
        let Ok(node) = which("node") else { return };
        for (name, src) in [("prelude.js", PRELUDE_JS), ("wrapper.js", WRAPPER_JS)] {
            let dir = std::env::temp_dir().join(format!("pf-prelude-check-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join(name);
            std::fs::write(&p, src).unwrap();
            let st = std::process::Command::new(&node)
                .arg("--check")
                .arg(&p)
                .status()
                .unwrap();
            assert!(st.success(), "{name} does not parse");
        }
    }

    fn which(bin: &str) -> Result<std::path::PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        std::env::split_paths(&path)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
            .ok_or(())
    }

    #[test]
    fn prelude_never_touches_real_cookies_or_storage() {
        // The shims redefine; the prelude never reads the native objects.
        assert!(!PRELUDE_JS.contains("window.localStorage"));
        assert!(PRELUDE_JS.contains("define(D, 'cookie'"));
        assert!(PRELUDE_JS.contains("define(W, 'indexedDB'"));
    }
}
