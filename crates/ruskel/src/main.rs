use clap::{Parser, ValueEnum};
use libruskel::{Ruskel, highlight};
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorMode::Auto => write!(f, "auto"),
            ColorMode::Always => write!(f, "always"),
            ColorMode::Never => write!(f, "never"),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

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
    #[arg(long, default_value_t = ColorMode::Auto, env = "RUSKEL_COLOR")]
    color: ColorMode,

    /// Disable paging
    #[arg(long, default_value_t = false)]
    no_page: bool,

    /// Enable offline mode, ensuring Cargo will not use the network
    #[arg(long, default_value_t = false)]
    offline: bool,

    /// Enable verbose mode, showing cargo output while rendering docs
    #[arg(long, default_value_t = false)]
    verbose: bool,

    /// Run as an MCP server on stdout
    #[arg(long, default_value_t = false)]
    mcp: bool,

    /// Host:port to bind to when running as MCP server (requires --mcp)
    #[arg(long)]
    addr: Option<String>,

    /// Log level for tracing output (only used with --mcp --addr)
    #[arg(long)]
    log: Option<LogLevel>,
}

fn check_nightly_toolchain() -> Result<(), String> {
    // Check if nightly toolchain is installed
    let output = Command::new("rustup")
        .args(["run", "nightly", "rustc", "--version"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run rustup: {e}"))?;

    if !output.status.success() {
        return Err("ruskel requires the nightly toolchain to be installed.\nRun: rustup toolchain install nightly".to_string());
    }

    // Check if rust-docs-json component is available (for std library support)
    let components_output = Command::new("rustup")
        .args(["component", "list", "--toolchain", "nightly"])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("Failed to check nightly components: {e}"))?;

    if components_output.status.success() {
        let components_str = String::from_utf8_lossy(&components_output.stdout);
        let has_rust_docs_json = components_str
            .lines()
            .any(|line| line.starts_with("rust-docs-json") && line.contains("(installed)"));

        if !has_rust_docs_json {
            eprintln!(
                "Warning: rust-docs-json component not installed. Standard library documentation will not be available."
            );
            eprintln!("To install: rustup component add rust-docs-json --toolchain nightly");
        }
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
        || !matches!(cli.color, ColorMode::Auto)
        || cli.no_page
    {
        return Err(
            "--mcp can only be used with --auto-impls, --private, --offline, and --verbose".into(),
        );
    }

    // Create configured Ruskel instance from CLI args
    let ruskel = Ruskel::new()
        .with_offline(cli.offline)
        .with_auto_impls(cli.auto_impls)
        .with_silent(!cli.verbose);

    // Run the MCP server
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(ruskel_mcp::run_mcp_server(
        ruskel,
        cli.addr.clone(),
        cli.log.map(|l| {
            match l {
                LogLevel::Error => "error",
                LogLevel::Warn => "warn",
                LogLevel::Info => "info",
                LogLevel::Debug => "debug",
                LogLevel::Trace => "trace",
            }
            .to_string()
        }),
    ))?;

    Ok(())
}

fn run_cmdline(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let should_highlight = match cli.color {
        ColorMode::Never => false,
        ColorMode::Always => true,
        ColorMode::Auto => io::stdout().is_terminal(),
    };

    let rs = Ruskel::new()
        .with_offline(cli.offline)
        .with_auto_impls(cli.auto_impls)
        .with_silent(!cli.verbose);

    let mut output = if cli.raw {
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

    // Apply highlighting if enabled and not raw output
    if should_highlight && !cli.raw {
        output = highlight::highlight_code(&output)?;
    }

    if io::stdout().is_terminal() && !cli.no_page {
        page_output(output)?;
    } else {
        println!("{output}");
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    // Validate that --addr is only used with --mcp
    if cli.addr.is_some() && !cli.mcp {
        eprintln!("Error: --addr can only be used with --mcp");
        std::process::exit(1);
    }

    // Validate that --log is only used with --mcp --addr
    if cli.log.is_some() && (cli.addr.is_none() || !cli.mcp) {
        eprintln!("Error: --log can only be used with --mcp --addr");
        std::process::exit(1);
    }

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

fn is_command_available(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

fn page_output(content: String) -> Result<(), Box<dyn std::error::Error>> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    // Check if the pager command is available
    if !is_command_available(&pager) {
        // If pager is not available, just print to stdout
        println!("{content}");
        return Ok(());
    }

    let mut child = Command::new(&pager).stdin(Stdio::piped()).spawn()?;

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
