use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate and print raw JSON output
    Raw {
        /// Target to generate - a directory or a module name
        #[arg(value_name = "TARGET")]
        target: Option<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Raw { target } => {
            let json = libruskel::generate_json(target.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }

    Ok(())
}
