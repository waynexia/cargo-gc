use clap::{Parser, Subcommand};

use crate::catalog::CollectIntent;

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
    #[arg(short, long)]
    profile: Option<String>,

    /// Only collect the given intent(s): build, check or test. Repeatable.
    /// Defaults to probing the target directory for the intents in use.
    #[arg(short, long)]
    collect: Vec<CollectIntent>,

    /// Arguments pass to `cargo build`, use `--` to separate from `cargo-gc` arguments.
    #[arg(trailing_var_arg = true)]
    cargo_args: Vec<String>,
}

pub struct Args {
    pub profile: String,
    pub verbose: bool,
    pub dry_run: bool,
    pub collect: Vec<CollectIntent>,
    pub cargo_args: Vec<String>,
}

impl From<Cli> for Args {
    fn from(value: Cli) -> Self {
        let Command::Gc(cli) = value.command;
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
            collect: cli.collect,
            cargo_args: cli.cargo_args,
        }
    }
}
