use clap::Parser;
use libruskel::Renderer;

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
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let rs = libruskel::Ruskel::new(&cli.target)?
        .with_no_default_features(cli.no_default_features)
        .with_all_features(cli.all_features)
        .with_features(cli.features);

    if cli.raw {
        let json = rs.pretty_raw_json()?;
        println!("{}", json);
    } else {
        let renderer = Renderer::default()
            .with_auto_impls(cli.auto_impls)
            .with_private_items(cli.private);

        let crate_data = rs.json()?;
        let rendered = renderer.render(&crate_data)?;
        println!("{}", rendered);
    }

    Ok(())
}
