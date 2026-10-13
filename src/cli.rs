use std::path::PathBuf;

use clap::Parser;

use crate::pm::Pm;

#[derive(Parser)]
#[command(
    name = "depee",
    version,
    about = "Inspect and update Node.js dependencies"
)]
pub struct Cli {
    #[arg(long, default_value = ".")]
    pub cwd: PathBuf,
    #[arg(long, value_enum)]
    pub pm: Option<Pm>,
    #[arg(long)]
    pub clear_cache: bool,
}
