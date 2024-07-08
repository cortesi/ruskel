use clap::Parser;
use libruskel::Ruskel;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Target to generate - a directory, file path, or a module name
    #[arg(default_value = ".")]
    target: String,

    /// Output raw JSON instead of rendered Rust code
    #[arg(long, default_value_t = false)]
    raw: bool,

    /// Render auto-implemented traits
    #[arg(long, default_value_t = false)]
    auto_impls: bool,

    /// Render private items
    #[arg(long, default_value_t = false)]
    private: bool,

    /// Disable default features
    #[arg(long, default_value_t = false)]
    no_default_features: bool,

    /// Enable all features
    #[arg(long, default_value_t = false)]
    all_features: bool,

    /// Specify features to enable
    #[arg(long, value_delimiter = ',')]
    features: Vec<String>,

    /// Enable syntax highlighting for the output
    #[arg(long, default_value_t = false)]
    highlight: bool,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let rs = Ruskel::new(&cli.target)?
        .with_no_default_features(cli.no_default_features)
        .with_all_features(cli.all_features)
        .with_features(cli.features)
        .with_highlighting(cli.highlight);

    if cli.raw {
        let json = rs.pretty_raw_json()?;
        println!("{}", json);
    } else {
        let rendered = rs.render(cli.auto_impls, cli.private)?;
        println!("{}", rendered);
    }

    Ok(())
}
