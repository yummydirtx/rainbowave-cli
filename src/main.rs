mod animation;
mod audio;
mod render;
mod terminal;

use std::process::ExitCode;

use clap::Parser;

/// Turn the terminal into a flowing field of spectral light.
#[derive(Debug, Parser)]
#[command(name = "rainbowave", version, about)]
struct Cli {
    /// React to music and other audio playing through the system output
    #[arg(long)]
    audio: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match animation::run(cli.audio) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rainbowave: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{error::ErrorKind, CommandFactory, Parser};

    use super::Cli;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn default_invocation_has_no_required_arguments() {
        let cli = Cli::try_parse_from(["rainbowave"]).unwrap();
        assert!(!cli.audio);
    }

    #[test]
    fn audio_mode_is_optional() {
        let cli = Cli::try_parse_from(["rainbowave", "--audio"]).unwrap();
        assert!(cli.audio);
    }

    #[test]
    fn help_and_version_are_available() {
        let help = Cli::try_parse_from(["rainbowave", "--help"]).unwrap_err();
        assert_eq!(help.kind(), ErrorKind::DisplayHelp);

        let version = Cli::try_parse_from(["rainbowave", "--version"]).unwrap_err();
        assert_eq!(version.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn unsupported_arguments_are_rejected() {
        let error = Cli::try_parse_from(["rainbowave", "--speed", "2"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
    }
}
