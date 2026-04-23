use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use speak_it::{Daemon, dependency_report, doctor_output, transcribe_file};

#[derive(Debug, Parser)]
#[command(
    name = "speak-it",
    version,
    about = "Press-to-talk STT client for Linux X11"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Daemon,
    Doctor,
    Once { audio_file: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Doctor => doctor(),
        Commands::Once { audio_file } => once(audio_file).await,
        Commands::Daemon => daemon().await,
    }
}

fn doctor() -> Result<()> {
    let report = dependency_report();
    let (ok, lines) = doctor_output(&report);
    for line in lines {
        println!("{line}");
    }
    if ok {
        println!("doctor: ok");
        Ok(())
    } else {
        bail!("doctor: failed")
    }
}

async fn once(audio_file: PathBuf) -> Result<()> {
    if !audio_file.is_file() {
        bail!("audio file does not exist: {}", audio_file.display());
    }

    let text = transcribe_file(&audio_file).await?;
    println!("{text}");
    Ok(())
}

async fn daemon() -> Result<()> {
    let report = dependency_report();
    report.validate(true)?;
    let daemon = Daemon::connect(report.recorder.expect("validated recorder"))?;
    daemon.run().await
}
