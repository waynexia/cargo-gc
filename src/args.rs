use clap::{Parser, Subcommand, command};

#[derive(Parser)]
#[command(author, version, about)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Gc(GcCommand),
}

#[derive(Parser)]
#[command(author, version, about)]
struct GcCommand {
    /// Display the detailed path of removed files.
    #[arg(short, long)]
    verbose: bool,

    /// Perform all checks without making any changes
    #[arg(short, long)]
    dry_run: bool,

    /// GC artifacts built in release profile
    #[arg(short, long)]
    release: bool,

    /// GC artifacts with the specified profile
    #[arg(long)]
    profile: Option<String>,

    /// Arguments pass to `cargo build`, use `--` to separate from `cargo-gc` arguments.
    #[arg(trailing_var_arg = true)]
    cargo_args: Vec<String>,
}

pub struct Args {
    pub profile: String,
    pub verbose: bool,
    pub dry_run: bool,
    pub cargo_args: Vec<String>,
}

impl Args {
    pub fn from_cli(cli: Cli) -> Self {
        let Command::Gc(cli) = cli.command;
        let profile = match (cli.profile, cli.release) {
            (None, true) => "release".into(),
            (None, false) => "dev".into(),
            (Some(_), true) => panic!("conflicting usage of --profile and --release"),
            (Some(profile), false) => profile,
        };

        let verbose = cli.verbose;
        let dry_run = cli.dry_run;

        Self {
            profile,
            verbose,
            dry_run,
            cargo_args: cli.cargo_args,
        }
    }

    pub fn cargo_profile_args(&self) -> Vec<String> {
        if self.profile != "debug" {
            vec!["--profile".into(), self.profile.clone()]
        } else {
            vec![]
        }
    }
}
