use clap::Parser;
use libruskel::Ruskel;
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

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

    /// Enable syntax highlighting
    #[arg(long, default_value_t = false)]
    highlight: bool,

    /// Disable syntax highlighting
    #[arg(long, default_value_t = false, conflicts_with = "highlight")]
    no_highlight: bool,

    /// Disable paging
    #[arg(long, default_value_t = false)]
    no_page: bool,

    /// Enable offline mode, ensuring Cargo will not use the network
    #[arg(long, default_value_t = false)]
    offline: bool,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let should_highlight = if cli.no_highlight {
        false
    } else {
        cli.highlight || io::stdout().is_terminal()
    };

    let rs = Ruskel::new(&cli.target)
        .with_offline(cli.offline)
        .with_no_default_features(cli.no_default_features)
        .with_all_features(cli.all_features)
        .with_features(cli.features)
        .with_highlighting(should_highlight);

    let output = if cli.raw {
        rs.raw_json()?
    } else {
        rs.render(cli.auto_impls, cli.private)?
    };

    if io::stdout().is_terminal() && !cli.no_page {
        page_output(output)?;
    } else {
        println!("{}", output);
    }

    Ok(())
}

fn page_output(content: String) -> Result<(), Box<dyn std::error::Error>> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let mut child = Command::new(pager).stdin(Stdio::piped()).spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Failed to open stdin for pager"))?;

    std::thread::spawn(move || {
        stdin.write_all(content.as_bytes()).ok();
        // Explicitly drop stdin to signal EOF to the pager
        drop(stdin);
    });

    // Wait for the pager to exit
    match child.wait() {
        Ok(status) => {
            if !status.success() {
                eprintln!("Pager exited with non-zero status: {}", status);
            }
            Ok(())
        }
        Err(e) => Err(Box::new(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to wait for pager: {}", e),
        ))),
    }
}
