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

    /// Export a self-contained HTML snapshot of delay history and prediction accuracy.
    ///
    /// Queries the last N days from the database and writes a single HTML file that
    /// can be opened in any browser — no server required.
    ExportSite {
        /// Output file path.
        #[arg(long, default_value = "docs/index.html")]
        output: PathBuf,

        /// How many days of history to include.
        #[arg(long, default_value = "7")]
        days: u32,
    },

    /// Export the self-contained offline command-centre map (GB delay map).
    ExportMap {
        /// Output file path.
        #[arg(long, default_value = "docs/map.html")]
        output: PathBuf,

        /// How many days of history to include.
        #[arg(long, default_value = "7")]
        days: u32,
    },

    /// Export the self-contained offline "RailPredict OS" desktop (map flagship).
    ExportOs {
        /// Output file path.
        #[arg(long, default_value = "docs/os.html")]
        output: PathBuf,

        /// How many days of history to include.
        #[arg(long, default_value = "7")]
        days: u32,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum IngestSource {
    Gtfs,
    Cif,
    /// Rail Settlement Plan reference CSVs (stations + operators); `--file` is the directory.
    Rds,
}
