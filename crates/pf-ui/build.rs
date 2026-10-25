use sha2::{Digest, Sha256};
use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    for p in [
        "assets/tokens.css",
        "assets/wrapper.css",
        "assets/plex-mono.woff2",
    ] {
        println!("cargo:rerun-if-changed={p}");
    }
    let tokens = fs::read_to_string("assets/tokens.css").unwrap();
    let wrapper = fs::read_to_string("assets/wrapper.css").unwrap();
    let hash = format!(
        "{:x}",
        Sha256::digest(fs::read("assets/plex-mono.woff2").unwrap())
    );
    let css = format!(
        "{tokens}\n@font-face{{font-family:'IBM Plex Mono';src:url('./{hash}') format('woff2');font-display:swap}}\n:root{{--pf-font-mono:'IBM Plex Mono',ui-monospace,monospace}}\n{wrapper}"
    );
    let mut output = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut output, 4096, 11, 22);
        w.write_all(css.as_bytes()).unwrap();
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("wrapper.css.br"),
        output,
    )
    .unwrap();
}
