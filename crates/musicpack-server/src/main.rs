//! The `musicpack-server` binary — a thin front end over the
//! [`musicpack_server`] library crate.
//!
//! Exit codes (the reference tool's convention): 0 success, 1 failure,
//! 2 usage error.

use std::process::ExitCode;

use musicpack_server::{cli, error::CliError, logging};

fn main() -> ExitCode {
    logging::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(CliError::Usage(message)) => {
            if message != "missing command" {
                eprintln!("musicpack-server: {message}");
            }
            cli::usage();
            ExitCode::from(2)
        }
        Err(CliError::Failure(message)) => {
            eprintln!("{message}");
            ExitCode::from(1)
        }
        Err(CliError::HelpRequested) => {
            cli::usage();
            ExitCode::SUCCESS
        }
        Err(CliError::VersionRequested) => {
            cli::print_version();
            ExitCode::SUCCESS
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, CliError> {
    let (command, config) = cli::parse(args)?;
    match command {
        cli::Command::Help => {
            cli::usage();
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Version => {
            cli::print_version();
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Token(subcommand) => cli::run_token(&subcommand, &config),
        cli::Command::Scan => cli::run_scan(&config, config.verify_on_scan),
        cli::Command::Verify => cli::run_scan(&config, true),
        cli::Command::Serve => cli::run_serve(&config),
    }
}
