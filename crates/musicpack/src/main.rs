//! The `musicpack` command-line tool.
//!
//! A thin consumer of `musicpack-core`: argument parsing, package opening,
//! presentation, and the one piece of application policy the core
//! intentionally does not own — safe extraction of a container to a
//! directory. No format, verification, or container semantics are
//! reimplemented here.
//!
//! Commands:
//!
//! ```text
//! musicpack info   <package>
//! musicpack verify <package> [-q|--quiet] [--json]
//! musicpack pack   <package-dir> <output.mpak>
//! musicpack unpack <input.mpak> <directory>
//! ```

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod package;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::run(&args) {
        Ok(code) => code,
        Err(cli::CliError::Usage(message)) => {
            eprintln!("musicpack: {message}");
            eprintln!(
                "usage: musicpack <info|verify|pack|unpack> ...\n\
                 \x20  info   <package-dir|file.mpak>\n\
                 \x20  verify <package-dir|file.mpak> [-q|--quiet] [--json]\n\
                 \x20  pack   <package-dir> <output.mpak>\n\
                 \x20  unpack <file.mpak> <directory>"
            );
            ExitCode::from(2)
        }
        Err(cli::CliError::Failure(message)) => {
            eprintln!("musicpack: {message}");
            ExitCode::from(1)
        }
    }
}
