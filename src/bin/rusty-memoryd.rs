use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rusty::advisor::{Hooks, Mode};
use serde_json::json;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Local L1/L2 memory advisor for rusty")]
struct Cli {
    #[arg(long, env = "RUSTY_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Control>,
}
#[derive(Subcommand)]
enum Control {
    Status,
    Remember {
        #[arg(long, default_value = "fact")]
        kind: String,
        text: String,
    },
    Export {
        file: PathBuf,
    },
    Import {
        file: PathBuf,
    },
}
fn main() -> Result<()> {
    let args = Cli::parse();
    let home = args
        .home
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config/rusty")))
        .ok_or_else(|| anyhow::anyhow!("set RUSTY_HOME or --home"))?;
    let Some(command) = args.command else { return rusty::advisor::serve(&home) };
    let mut h = Hooks::connect(&home, &std::env::current_dir()?, Mode::On)?;
    let (op, data) = match &command {
        Control::Status => ("status", json!(null)),
        Control::Remember { kind, text } => ("remember", json!({"kind":kind,"text":text})),
        Control::Export { .. } => ("export", json!(null)),
        Control::Import { file } => {
            let mut text = String::new();
            GzDecoder::new(std::fs::File::open(file)?).take(4 * 1024 * 1024).read_to_string(&mut text)?;
            ("import", serde_json::from_str(&text)?)
        }
    };
    let r = h.control(op, data);
    if r.error {
        bail!("{}", r.text);
    }
    if let Control::Export { file } = command {
        // Never overwrite an existing destination; snapshots can contain private context.
        let f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(file)?;
        let mut w = GzEncoder::new(f, Compression::best());
        w.write_all(&serde_json::to_vec(&r.data)?)?;
        w.finish()?;
    } else {
        println!("{}", r.text);
    }
    Ok(())
}
