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
        #[arg(value_name = "TARGET", default_value = ".")]
        target: String,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Raw { target } => {
            let rs = libruskel::Ruskel::new(target)?;
            match rs.pretty_raw_json() {
                Ok(json) => println!("{}", json),
                Err(e) => eprintln!("Error while generating JSON: {}", e),
            }
        }
    }

    Ok(())
}
