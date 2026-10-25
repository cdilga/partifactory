# Install the part CLI

Slice 1 provides a static Linux amd64 CLI in `part-linux-amd64.tar.gz`. macOS,
Windows and ARM binaries come later. Choose a release from the deployment's forge
and download its `SHA256SUMS` and CLI archive over HTTPS. Verify the archive's
checksum before installing it.

With cargo-binstall 1.26.1 or newer, use the standalone distribution manifest from
the same release's source checkout:

```sh
cargo binstall --manifest-path deploy/part-cli.Cargo.toml \
  --pkg-url 'https://example.com/partifactory/releases/download/v0.1.0/part-linux-amd64.tar.gz' \
  --no-confirm partifactory-cli
part --help
```

Replace the example URL with that release's actual archive URL; the forge and tag
format are deployment choices. `--manifest-path` avoids a crates.io lookup: this
distribution package is not published there. The manifest installs the archive's
`part` executable under that exact name, which selects the agent CLI. The same
bytes invoked as `partifactory` select the server and administration commands.

The manifest permits Linux x86-64 GNU and musl targets and disables compilation
and third-party binary fallbacks. Other targets fail before downloading the
archive. Leave `--targets` and `--strategies` unset when installing for this machine;
these flags override platform selection and the manifest's strategy restrictions.
To install somewhere other than Cargo's default binary directory, add
`--install-path /path/to/bin`.

For manual installation, extract the verified archive and place the executable in
a directory on `PATH`, preserving its filename `part`.

The release installation check uses the real archive, an isolated installation
directory, and a loopback HTTP server. It checks the installed bytes and filename,
and verifies that unsupported platforms make no archive request:

```sh
python3 tests/deploy/binstall.py --binstall /path/to/cargo-binstall \
  --assets target/release-assets
```
