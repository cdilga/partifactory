//! The `part` CLI: the op registry over HTTPS, with device-code sign-in.
//!
//! Reached as `part …` (the binary invoked through a `part` symlink or copy).

use clap::{Parser, Subcommand};
use std::ffi::OsString;

/// Exit code for a subcommand whose bead hasn't landed (EX_USAGE).
pub const EXIT_NOT_IMPLEMENTED: i32 = 64;

#[derive(Parser)]
#[command(
    name = "part",
    version,
    about = "part: make and edit parts on a partifactory server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Connect this machine to a server (device code).
    Login,
    /// Create a part.
    Create,
    /// Edit a part.
    Edit,
    /// Read a part.
    Get,
    /// Render a part and run its checks.
    Check,
    /// Read or change a part's data.
    Data,
    /// Get a deep link for an action only a person may take.
    LinkFor,
}

/// Run the CLI with these args (args[0] is the program name). Returns the exit code.
pub fn run(args: Vec<OsString>) -> i32 {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return code;
        }
    };
    let name = match cli.command {
        Command::Login => "login",
        Command::Create => "create",
        Command::Edit => "edit",
        Command::Get => "get",
        Command::Check => "check",
        Command::Data => "data",
        Command::LinkFor => "link-for",
    };
    eprintln!("part {name}: not implemented yet");
    EXIT_NOT_IMPLEMENTED
}
