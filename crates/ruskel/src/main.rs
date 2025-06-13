use clap::Parser;
use libruskel::Ruskel;
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Target to generate - a directory, file path, or a module name
    #[arg(default_value = "./")]
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

    /// Colorize output
    #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"], env = "RUSKEL_COLOR")]
    color: String,

    /// Disable paging
    #[arg(long, default_value_t = false)]
    no_page: bool,

    /// Enable offline mode, ensuring Cargo will not use the network
    #[arg(long, default_value_t = false)]
    offline: bool,

    /// Enable quiet mode, disabling output while rendering docs
    #[arg(long, default_value_t = false)]
    quiet: bool,

    /// Run as an MCP server on stdout
    #[arg(long, default_value_t = false)]
    mcp: bool,
}

fn check_nightly_toolchain() -> Result<(), String> {
    let output = Command::new("rustup")
        .args(["run", "nightly", "rustc", "--version"])
        .stderr(Stdio::null()) // Suppress stderr to avoid rustup's error message
        .output()
        .map_err(|e| format!("Failed to run rustup: {e}"))?;

    if !output.status.success() {
        return Err("ruskel requires the nightly toolchain to be installed - run 'rustup toolchain install nightly'".to_string());
    }

    Ok(())
}

fn run_mcp(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    // Validate that only configuration arguments are provided with --mcp
    if cli.target != "./"
        || cli.raw
        || cli.no_default_features
        || cli.all_features
        || !cli.features.is_empty()
        || cli.color != "auto"
        || cli.no_page
    {
        return Err(
            "--mcp can only be used with --auto-impls, --private, --offline, and --quiet".into(),
        );
    }

    // Create configured Ruskel instance from CLI args
    let ruskel = Ruskel::new()
        .with_offline(cli.offline)
        .with_highlighting(false) // No highlighting for MCP output
        .with_auto_impls(cli.auto_impls)
        .with_silent(cli.quiet);

    // Run the MCP server
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(ruskel_mcp::run_mcp_server(ruskel))?;

    Ok(())
}

fn run_cmdline(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let should_highlight = match cli.color.as_str() {
        "never" => false,
        "always" => true,
        "auto" => io::stdout().is_terminal(),
        _ => unreachable!(),
    };

    let rs = Ruskel::new()
        .with_offline(cli.offline)
        .with_highlighting(should_highlight)
        .with_auto_impls(cli.auto_impls)
        .with_silent(cli.quiet);

    let output = if cli.raw {
        rs.raw_json(
            &cli.target,
            cli.no_default_features,
            cli.all_features,
            cli.features.clone(),
            cli.private,
        )?
    } else {
        rs.render(
            &cli.target,
            cli.no_default_features,
            cli.all_features,
            cli.features.clone(),
            cli.private,
        )?
    };

    if io::stdout().is_terminal() && !cli.no_page {
        page_output(output)?;
    } else {
        println!("{output}");
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let result = if cli.mcp {
        run_mcp(&cli)
    } else {
        if let Err(e) = check_nightly_toolchain() {
            eprintln!("{e}");
            std::process::exit(1);
        }
        run_cmdline(&cli)
    };

    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn page_output(content: String) -> Result<(), Box<dyn std::error::Error>> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let mut child = Command::new(pager).stdin(Stdio::piped()).spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Failed to open stdin for pager"))?;

    std::thread::spawn(move || {
        stdin.write_all(content.as_bytes()).ok();
        // Explicitly drop stdin to signal EOF to the pager
        drop(stdin);
    });

    // Wait for the pager to exit
    match child.wait() {
        Ok(status) => {
            if !status.success() {
                eprintln!("Pager exited with non-zero status: {status}");
            }
            Ok(())
        }
        Err(e) => Err(Box::new(io::Error::other(format!(
            "Failed to wait for pager: {e}"
        )))),
    }
}
