//! Command-line interface for the `ruskel` API skeleton generator.

use std::{
    env,
    error::Error,
    io::{self, IsTerminal, Write},
    process::{self, Command, Stdio},
    thread,
};

use clap::{ColorChoice, Parser};
use libruskel::{
    Ruskel, SearchDomain, SearchOptions, highlight, parse_domain_token,
    toolchain::ensure_nightly_with_docs,
};
use shell_words::split;
use tokio::runtime::Runtime;
use tracing_subscriber::filter::LevelFilter;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// Parsed command-line options for the ruskel CLI.
struct Cli {
    /// Target to generate - a directory, file path, or a module name
    #[arg(default_value = "./")]
    target: String,

    /// Select a specific binary target when rendering a package
    #[arg(long, value_name = "NAME")]
    bin: Option<String>,

    /// Output raw JSON instead of rendered Rust code
    #[arg(long, default_value_t = false)]
    raw: bool,

    /// Search query used to filter the generated skeleton instead of rendering everything.
    #[arg(long)]
    search: Option<String>,

    /// Output a structured item listing instead of rendered code.
    #[arg(long, default_value_t = false, conflicts_with = "raw")]
    list: bool,

    /// Comma-separated list of search domains (name, doc, signature, path). Defaults to name, doc, signature.
    #[arg(
        long = "search-spec",
        value_delimiter = ',',
        value_name = "DOMAIN[,DOMAIN...]",
        default_value = "name,doc,signature",
        value_parser = parse_domain_token
    )]
    search_spec: Vec<SearchDomain>,

    /// Execute the search in a case sensitive manner.
    #[arg(long, default_value_t = false)]
    search_case_sensitive: bool,

    /// Suppress automatic expansion of matched containers when searching.
    #[arg(long, default_value_t = false)]
    direct_match_only: bool,

    /// Render auto-implemented traits
    #[arg(long, default_value_t = false)]
    auto_impls: bool,

    /// Render private items
    #[arg(long, default_value_t = false)]
    private: bool,

    /// Disable frontmatter comments in the rendered skeleton
    #[arg(long, default_value_t = false)]
    no_frontmatter: bool,

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
    #[arg(long, default_value_t = ColorChoice::Auto, env = "RUSKEL_COLOR")]
    color: ColorChoice,

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
    #[arg(long, requires = "mcp")]
    addr: Option<String>,

    /// Log level for tracing output (only used with --mcp --addr)
    #[arg(long, requires_all = ["mcp", "addr"])]
    log: Option<LevelFilter>,
}

/// Ensure the nightly toolchain and rust-docs JSON component are present.
fn check_nightly_toolchain() -> Result<(), String> {
    match ensure_nightly_with_docs() {
        Ok(has_docs) => {
            if !has_docs {
                eprintln!(
                    "Warning: rust-docs-json component not installed. Standard library documentation will not be available."
                );
                eprintln!("To install: rustup component add rust-docs-json --toolchain nightly");
            }
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Launch the MCP server variant of ruskel using the provided CLI configuration.
fn run_mcp(cli: &Cli) -> Result<(), Box<dyn Error>> {
    // Validate that only configuration arguments are provided with --mcp
    if cli.target != "./"
        || cli.bin.is_some()
        || cli.raw
        || cli.no_default_features
        || cli.all_features
        || !cli.features.is_empty()
        || !matches!(cli.color, ColorChoice::Auto)
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
        .with_frontmatter(!cli.no_frontmatter)
        .with_silent(!cli.verbose);

    // Run the MCP server
    let runtime = Runtime::new()?;
    runtime.block_on(ruskel_mcp::run_mcp_server(
        ruskel,
        cli.addr.clone(),
        cli.log,
    ))?;

    Ok(())
}

/// Render a skeleton locally and stream it to stdout or a pager.
fn run_cmdline(cli: &Cli) -> Result<(), Box<dyn Error>> {
    let should_highlight = match cli.color {
        ColorChoice::Never => false,
        ColorChoice::Always => true,
        ColorChoice::Auto => io::stdout().is_terminal(),
    };

    let rs = Ruskel::new()
        .with_offline(cli.offline)
        .with_auto_impls(cli.auto_impls)
        .with_frontmatter(!cli.no_frontmatter)
        .with_silent(!cli.verbose)
        .with_bin_target(cli.bin.clone());

    if cli.list {
        return run_list(cli, &rs);
    }

    if let Some(query) = cli.search.as_deref() {
        return run_search(cli, &rs, query, should_highlight);
    }

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

/// Resolve the active search domains specified by the CLI flags.
fn search_domains_from_cli(cli: &Cli) -> SearchDomain {
    if cli.search_spec.is_empty() {
        SearchDomain::default()
    } else {
        cli.search_spec
            .iter()
            .fold(SearchDomain::empty(), |mut acc, spec| {
                acc |= *spec;
                acc
            })
    }
}

/// Build a `SearchOptions` value using the provided CLI configuration and query.
fn build_search_options(cli: &Cli, query: &str) -> SearchOptions {
    let mut options = SearchOptions::new(query);
    options.include_private = cli.private;
    options.case_sensitive = cli.search_case_sensitive;
    options.expand_containers = !cli.direct_match_only;
    options.domains = search_domains_from_cli(cli);
    options
}

/// Execute the list flow and print a structured item summary.
fn run_list(cli: &Cli, rs: &Ruskel) -> Result<(), Box<dyn Error>> {
    if cli.raw {
        return Err("--raw cannot be combined with --list".into());
    }

    let mut search_options: Option<SearchOptions> = None;
    let mut trimmed_query: Option<String> = None;

    if let Some(query) = cli.search.as_deref() {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            println!("Search query is empty; nothing to do.");
            return Ok(());
        }
        trimmed_query = Some(trimmed.to_string());
        search_options = Some(build_search_options(cli, trimmed));
    }

    let listings = rs.list(
        &cli.target,
        cli.no_default_features,
        cli.all_features,
        cli.features.clone(),
        cli.private,
        search_options.as_ref(),
    )?;

    if listings.is_empty() {
        if let Some(query) = trimmed_query {
            println!("No matches found for \"{query}\".");
        } else {
            println!("No items found.");
        }
        return Ok(());
    }

    let label_width = listings
        .iter()
        .map(|entry| entry.kind.label().len())
        .max()
        .unwrap_or(0);

    let mut buffer = String::new();
    for entry in listings {
        let label = entry.kind.label();
        if label_width > 0 {
            buffer.push_str(&format!(
                "{label:<width$} {}\n",
                entry.path,
                width = label_width
            ));
        } else {
            buffer.push_str(&format!("{label} {}\n", entry.path));
        }
    }

    if io::stdout().is_terminal() && !cli.no_page {
        page_output(buffer)?;
    } else {
        print!("{}", buffer);
    }

    Ok(())
}

/// Execute the search flow and print the filtered skeleton to stdout.
fn run_search(
    cli: &Cli,
    rs: &Ruskel,
    query: &str,
    should_highlight: bool,
) -> Result<(), Box<dyn Error>> {
    if cli.raw {
        return Err("--raw cannot be combined with --search".into());
    }

    let trimmed = query.trim();
    if trimmed.is_empty() {
        println!("Search query is empty; nothing to do.");
        return Ok(());
    }

    let options = build_search_options(cli, trimmed);

    let response = rs.search(
        &cli.target,
        cli.no_default_features,
        cli.all_features,
        cli.features.clone(),
        &options,
    )?;

    if response.results.is_empty() {
        println!("No matches found for \"{}\".", trimmed);
        return Ok(());
    }

    let mut output = response.rendered;
    if should_highlight {
        output = highlight::highlight_code(&output)?;
    }

    if io::stdout().is_terminal() && !cli.no_page {
        page_output(output)?;
    } else {
        print!("{}", output);
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
            process::exit(1);
        }
        run_cmdline(&cli)
    };

    if let Err(e) = result {
        eprintln!("{e}");
        process::exit(1);
    }
}

/// Check whether the given command is discoverable on the current `PATH`.
fn is_command_available(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

/// Parse the pager command and arguments from the `PAGER` environment variable.
fn pager_command_from_env() -> (String, Vec<String>) {
    const DEFAULT_PAGER: &str = "less";

    let raw_value = env::var("PAGER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_PAGER.to_string());

    match split(&raw_value) {
        Ok(mut parts) => {
            if parts.is_empty() {
                return (DEFAULT_PAGER.to_string(), Vec::new());
            }

            let command = parts.remove(0);
            (command, parts)
        }
        Err(_) => {
            let mut fallback: Vec<String> =
                raw_value.split_whitespace().map(str::to_owned).collect();

            if fallback.is_empty() {
                return (DEFAULT_PAGER.to_string(), Vec::new());
            }

            let command = fallback.remove(0);
            (command, fallback)
        }
    }
}

/// Display the generated content through a pager when available.
fn page_output(content: String) -> Result<(), Box<dyn Error>> {
    let (pager_command, pager_args) = pager_command_from_env();

    if !is_command_available(&pager_command) {
        println!("{content}");
        return Ok(());
    }

    let mut command = Command::new(&pager_command);
    command.args(&pager_args);
    command.stdin(Stdio::piped());

    let mut child = command.spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Failed to open stdin for pager"))?;

    thread::spawn(move || {
        stdin.write_all(content.as_bytes()).ok();
        drop(stdin);
    });

    match child.wait() {
        Ok(status) => {
            if !status.success() {
                eprintln!("Pager exited with non-zero status: {status}");
            }
            Ok(())
        }
        Err(error) => Err(Box::new(io::Error::other(format!(
            "Failed to wait for pager: {error}"
        )))),
    }
}
