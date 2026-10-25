//! `cargo run -p pf-serve --example serve --features dev`: the listeners alone, configured
//! exactly as `partifactory serve` is (site.toml + env + `_FILE` secrets). For working on
//! pf-serve and the e2e harness without the rest of the binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    pf_obs::init_json_logs();
    let env = pf_config::process_env();
    let loaded = match pf_config::load_from_env(&env) {
        Ok(l) => l,
        Err(diags) => {
            for d in &diags {
                pf_config::report(d);
            }
            return ExitCode::from(2);
        }
    };
    for w in &loaded.warnings {
        pf_config::report(w);
    }
    match pf_serve::run_blocking(&loaded, &env, |_| {}) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
