//! CLI argument parsing for the `railpredict` binary.
//!
//! Running with no subcommand starts the server normally.
//! Running with a subcommand performs that one-shot operation and exits.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "railpredict", version, about = "UK Rail shadow system")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Ingest static timetable / station data into the database.
    IngestStatic {
        /// Data source format.
        #[arg(long, value_enum, default_value = "gtfs")]
        source: IngestSource,

        /// URL to download the source archive from.
        /// Mutually exclusive with --file.
        #[arg(long, conflicts_with = "file")]
        url: Option<String>,

        /// Path to a pre-downloaded source archive.
        /// Mutually exclusive with --url.
        #[arg(long, conflicts_with = "url")]
        file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum IngestSource {
    Gtfs,
    Cif,
}
