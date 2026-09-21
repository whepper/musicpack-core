//! `musicpack-author` binary entry point.

use std::process::ExitCode;

use musicpack_author::cli::{CliError, run};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(CliError::Usage(message)) => {
            eprintln!("musicpack-author: {message}");
            ExitCode::from(2)
        }
        Err(CliError::Failure(message)) => {
            eprintln!("musicpack-author: {message}");
            ExitCode::FAILURE
        }
    }
}
