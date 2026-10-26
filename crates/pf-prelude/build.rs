use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    // The wrapper and the formatted-view editor are served pre-compressed from the blob host.
    for name in ["wrapper.js", "editor.js"] {
        println!("cargo:rerun-if-changed=js/{name}");
        let input = fs::read(format!("js/{name}")).expect("script source");
        let mut output = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut output, 4096, 11, 22);
            w.write_all(&input).expect("compress script");
        }
        fs::write(
            PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(format!("{name}.br")),
            output,
        )
        .expect("write compressed script");
    }
}
