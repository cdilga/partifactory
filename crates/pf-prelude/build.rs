use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=js/wrapper.js");
    let input = fs::read("js/wrapper.js").expect("wrapper source");
    let mut output = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut output, 4096, 11, 22);
        w.write_all(&input).expect("compress wrapper");
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("wrapper.js.br"),
        output,
    )
    .expect("write wrapper");
}
