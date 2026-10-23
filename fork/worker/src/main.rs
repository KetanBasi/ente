use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use ente_fork_worker::config::Config;
use ente_fork_worker::identity::{self, WorkerIdentity};
use ente_fork_worker::pipeline::Worker;
use ente_fork_worker::{harden, secure_mem};

#[derive(Parser)]
#[command(
    name = "ente-worker",
    version,
    about = "Home AI worker for the Ente fork"
)]
struct Cli {
    /// Path to worker.toml.
    #[arg(
        long,
        short,
        global = true,
        default_value = "/etc/ente-worker/worker.toml"
    )]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Poll the inbox forever.
    Run,
    /// Process the inbox once and exit.
    Once,
    /// Generate a worker keypair: secret on stdout (pipe it into a secret
    /// store), public key and fingerprint on stderr.
    Keygen {
        /// Allow printing the secret to a terminal.
        #[arg(long)]
        allow_terminal: bool,
    },
    /// Validate worker.toml and print the resolved settings.
    CheckConfig,
    /// Report which key-custody protections this host offers.
    Doctor,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Keygen { allow_terminal } => identity::keygen(allow_terminal),
        Command::CheckConfig => {
            let config = Config::load(&cli.config)?;
            println!("{config:#?}");
            Ok(())
        }
        Command::Doctor => {
            let locked = Config::load(&cli.config).map_or(64 * 1024, |c| c.locked_bytes());
            print!("{}", harden::harden(locked).render());
            Ok(())
        }
        Command::Run => run(&cli.config, false),
        Command::Once => run(&cli.config, true),
    }
}

fn run(path: &std::path::Path, once: bool) -> anyhow::Result<()> {
    let config = Config::load(path)?;
    // Harden before any secret is read.
    let report = harden::harden(config.locked_bytes());
    harden::log_report(&report);
    let options = secure_mem::Options {
        require_memfd_secret: config.memory.require_memfd_secret,
        ..Default::default()
    };
    let identity = WorkerIdentity::load(&config.identity, config.public_key.as_deref(), options)?;
    let poll = Duration::from_secs(config.runtime.poll_interval_secs);
    let worker = Worker::new(config, identity)?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            loop {
                match worker.run_once().await {
                    Ok(s) => log::info!("poll: {s:?}"),
                    Err(e) if once => return Err(e.into()),
                    Err(e) => log::error!("poll failed: {e}"),
                }
                if once {
                    return Ok(());
                }
                tokio::select! {
                    () = tokio::time::sleep(poll) => {}
                    _ = tokio::signal::ctrl_c() => {
                        log::info!("stopping");
                        return Ok(());
                    }
                }
            }
        })
}
