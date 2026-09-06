//! Command-line interface for the `ruskel` API skeleton generator.

use std::{
    env,
    error::Error,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    thread,
    time::SystemTime,
};

use bytesize::ByteSize;
use clap::{ColorChoice, Parser};
use libruskel::{
    CacheIssue, CacheStatus, CleanReport, CrateRequest, Ruskel, SearchDomain, SearchOptions,
    highlight, toolchain::ensure_nightly_with_docs,
};
use ruskel_mcp::RuskelServerDefaults;
use shell_words::split;
use timeago::Formatter as TimeAgoFormatter;
use tokio::runtime::Runtime;
use tracing_subscriber::filter::LevelFilter;

/// Message printed when a search flag is present but contains only whitespace.
const EMPTY_SEARCH_MESSAGE: &str = "Search query is empty; nothing to do.";
/// Error returned when `--mcp` is combined with flags that belong on individual
/// requests.
const MCP_REQUEST_SCOPED_FLAGS_ERROR: &str = "--mcp can only be used with --cache-dir, --auto-impls, --private, --no-frontmatter, --offline, --verbose, --addr, and --log";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
/// Parsed command-line options for the ruskel CLI.
struct Cli {
    /// Target to generate - a directory, file path, or a module name
    target: Option<String>,

    /// Override the dedicated Ruskel cache root
    #[arg(long, value_name = "PATH", env = "RUSKEL_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Remove cache-owned build data and report the result
    #[arg(long, conflicts_with_all = ["cache_status", "mcp", "target"])]
    clean_cache: bool,

    /// Report cache usage, entries, and last-use times
    #[arg(long, conflicts_with_all = ["clean_cache", "mcp", "target"])]
    cache_status: bool,

    /// Select a specific binary target when rendering a package
    #[arg(long, value_name = "NAME")]
    bin: Option<String>,

    /// Output raw JSON instead of rendered Rust code
    #[arg(long, default_value_t = false)]
    raw: bool,

    /// Search query used to filter the generated skeleton instead of rendering
    /// everything.
    #[arg(long)]
    search: Option<String>,

    /// Output a structured item listing instead of rendered code.
    #[arg(long, default_value_t = false, conflicts_with = "raw")]
    list: bool,

    /// Comma-separated list of search domains (name, doc, signature, path).
    /// Defaults to name, doc, signature.
    #[arg(
        long = "search-spec",
        value_delimiter = ',',
        value_name = "DOMAIN[,DOMAIN...]",
        default_value = "name,doc,signature",
        value_parser = SearchDomain::parse
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

impl Cli {
    /// Resolve the active search domains specified by the CLI flags.
    fn search_domains(&self) -> SearchDomain {
        if self.search_spec.is_empty() {
            SearchDomain::default()
        } else {
            self.search_spec
                .iter()
                .fold(SearchDomain::empty(), |mut acc, spec| {
                    acc |= *spec;
                    acc
                })
        }
    }

    /// Build search options for a concrete query using the CLI's current flags.
    fn build_search_options(&self, query: &str) -> SearchOptions {
        SearchOptions::configured(
            query,
            self.search_domains(),
            self.search_case_sensitive,
            !self.direct_match_only,
        )
    }

    /// Build one crate request from Cargo feature and visibility flags.
    fn crate_request(&self) -> CrateRequest {
        CrateRequest {
            no_default_features: self.no_default_features,
            all_features: self.all_features,
            features: self.features.clone(),
            private_items: self.private,
        }
    }

    /// Check whether the current CLI invocation uses request-scoped flags.
    fn uses_request_scoped_flags(&self) -> bool {
        self.target.is_some()
            || self.bin.is_some()
            || self.raw
            || self.list
            || self.search.is_some()
            || self.search_domains() != SearchDomain::default()
            || self.search_case_sensitive
            || self.direct_match_only
            || self.no_default_features
            || self.all_features
            || !self.features.is_empty()
            || !matches!(self.color, ColorChoice::Auto)
            || self.no_page
    }

    /// Derive the MCP server defaults from the allowed server-scoped flags.
    fn mcp_defaults(&self) -> Result<RuskelServerDefaults, Box<dyn Error>> {
        if self.uses_request_scoped_flags() {
            return Err(MCP_REQUEST_SCOPED_FLAGS_ERROR.into());
        }

        Ok(RuskelServerDefaults {
            private: self.private,
            frontmatter: !self.no_frontmatter,
        })
    }
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

/// Search-query state derived from CLI input.
enum SearchQuery<'a> {
    /// No search query was provided.
    Missing,
    /// A query argument was provided but contained only whitespace.
    Empty,
    /// A non-empty trimmed search query.
    Present(&'a str),
}

/// Normalize the optional `--search` argument into a simple state machine.
fn search_query_state(query: Option<&str>) -> SearchQuery<'_> {
    match query {
        Some(query) => match query.trim() {
            "" => SearchQuery::Empty,
            trimmed => SearchQuery::Present(trimmed),
        },
        None => SearchQuery::Missing,
    }
}

/// Construct a configured `Ruskel` instance from CLI arguments.
fn ruskel_from_cli(cli: &Cli) -> Ruskel {
    Ruskel::new()
        .with_offline(cli.offline)
        .with_auto_impls(cli.auto_impls)
        .with_frontmatter(!cli.no_frontmatter)
        .with_silent(!cli.verbose)
        .with_bin_target(cli.bin.clone())
        .with_cache_dir(cli.cache_dir.clone())
}

/// Return the positional target or the current-project default.
fn target_or_default(cli: &Cli) -> &str {
    cli.target.as_deref().unwrap_or("./")
}

/// Write generated output either through a pager or directly to stdout.
fn emit_output(cli: &Cli, output: String) -> Result<(), Box<dyn Error>> {
    if io::stdout().is_terminal() && !cli.no_page {
        page_output(output)?;
    } else {
        print!("{output}");
    }

    Ok(())
}

/// Apply syntax highlighting when requested.
fn highlight_output(output: String, should_highlight: bool) -> Result<String, Box<dyn Error>> {
    if should_highlight {
        Ok(highlight::highlight_code(&output)?)
    } else {
        Ok(output)
    }
}

/// Launch the MCP server variant of ruskel using the provided CLI
/// configuration.
fn run_mcp(cli: &Cli) -> Result<(), Box<dyn Error>> {
    let defaults = cli.mcp_defaults()?;
    let ruskel = ruskel_from_cli(cli);

    let runtime = Runtime::new()?;
    runtime.block_on(ruskel_mcp::run_mcp_server(
        ruskel,
        cli.addr.clone(),
        cli.log,
        defaults,
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

    let rs = ruskel_from_cli(cli);
    let target = target_or_default(cli);
    let request = cli.crate_request();

    if cli.list {
        return run_list(cli, &rs);
    }

    match search_query_state(cli.search.as_deref()) {
        SearchQuery::Present(query) => return run_search(cli, &rs, query, should_highlight),
        SearchQuery::Empty => {
            println!("{EMPTY_SEARCH_MESSAGE}");
            return Ok(());
        }
        SearchQuery::Missing => {}
    }

    let output = if cli.raw {
        rs.raw_json(target, &request)?
    } else {
        rs.render(target, &request)?
    };

    let output = highlight_output(output, should_highlight && !cli.raw)?;
    emit_output(cli, output)
}

/// Execute the list flow and print a structured item summary.
fn run_list(cli: &Cli, rs: &Ruskel) -> Result<(), Box<dyn Error>> {
    if cli.raw {
        return Err("--raw cannot be combined with --list".into());
    }

    let (search_options, query_label) = match search_query_state(cli.search.as_deref()) {
        SearchQuery::Missing => (None, None),
        SearchQuery::Empty => {
            println!("{EMPTY_SEARCH_MESSAGE}");
            return Ok(());
        }
        SearchQuery::Present(query) => (Some(cli.build_search_options(query)), Some(query)),
    };

    let listings = rs.list(
        target_or_default(cli),
        &cli.crate_request(),
        search_options.as_ref(),
    )?;

    if listings.is_empty() {
        if let Some(query) = query_label {
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

    emit_output(cli, buffer)
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

    let options = cli.build_search_options(query);

    let response = rs.search(target_or_default(cli), &cli.crate_request(), &options)?;

    if response.results.is_empty() {
        println!("No matches found for \"{}\".", query);
        return Ok(());
    }

    let output = highlight_output(response.rendered, should_highlight)?;
    emit_output(cli, output)
}

fn main() {
    match run(&Cli::parse()) {
        Ok(true) => {}
        Ok(false) => process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
}

/// Dispatch cache operations before nightly checks and ordinary commands.
fn run(cli: &Cli) -> Result<bool, Box<dyn Error>> {
    let ruskel = ruskel_from_cli(cli);
    if cli.cache_status {
        let status = ruskel.cache_status()?;
        print_cache_status(&status);
        return Ok(true);
    }
    if cli.clean_cache {
        let report = ruskel.clean_cache()?;
        print_clean_report(&report);
        return Ok(report.is_complete());
    }
    if cli.mcp {
        run_mcp(cli)?;
        return Ok(true);
    }

    check_nightly_toolchain().map_err(io::Error::other)?;
    run_cmdline(cli)?;
    Ok(true)
}

/// Print a deterministic cache-status report.
fn print_cache_status(status: &CacheStatus) {
    let now = SystemTime::now();
    println!("Cache root: {}", status.root().display());
    println!("Recognized usage: {}", format_size(status.total_bytes()));
    println!("Trash usage: {}", format_size(status.trash_bytes()));
    println!("Soft-budget excess: {}", format_size(status.excess_bytes()));
    println!("Toolchains: {}", status.toolchains().len());
    for toolchain in status.toolchains() {
        println!(
            "toolchain {} size={} last_use={} locked={}",
            toolchain.identity(),
            format_size(toolchain.size_bytes()),
            format_last_use(toolchain.last_use(), now),
            toolchain.is_locked()
        );
        for workspace in toolchain.workspaces() {
            let label = format_workspace_label(workspace.workspace_root(), workspace.identity());
            println!(
                "  workspace {} size={} last_use={} locked={}",
                label,
                format_size(workspace.size_bytes()),
                format_last_use(workspace.last_use(), now),
                workspace.is_locked()
            );
            if !workspace.packages().is_empty() {
                println!("    packages: {}", workspace.packages().join(", "));
            }
        }
    }
    print_issues("Skipped", status.skipped());
}

/// Print a deterministic cache-clean report.
fn print_clean_report(report: &CleanReport) {
    let outcome = if report.root_busy() {
        "busy"
    } else if report.failures().is_empty() {
        "complete"
    } else {
        "partial"
    };
    println!("Cache root: {}", report.root().display());
    println!("Clean result: {outcome}");
    println!("Removed entries: {}", report.removed_entries());
    println!("Removed bytes: {}", format_size(report.removed_bytes()));
    println!(
        "Recognized usage after clean: {}",
        format_size(report.usage_after())
    );
    print_issues("Skipped", report.skipped());
    print_issues("Failures", report.failures());
}

/// Print one ordered issue section when it is not empty.
fn print_issues(label: &str, issues: &[CacheIssue]) {
    println!("{label}: {}", issues.len());
    for issue in issues {
        println!("  {}: {}", issue.path().display(), issue.message());
    }
}

/// Format a byte count with an IEC size suffix.
fn format_size(bytes: u64) -> String {
    ByteSize::b(bytes).to_string()
}

/// Prefer recorded workspace context and retain the identity as a legacy
/// fallback.
fn format_workspace_label(workspace_root: Option<&Path>, identity: &str) -> String {
    workspace_root.map_or_else(
        || identity.to_string(),
        |root| format_path_with_home(root, dirs::home_dir().as_deref()),
    )
}

/// Replace the user's home-directory prefix with `~` for display.
fn format_path_with_home(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    let Ok(relative) = path.strip_prefix(home) else {
        return path.display().to_string();
    };
    if relative.as_os_str().is_empty() {
        "~".to_string()
    } else {
        Path::new("~").join(relative).display().to_string()
    }
}

/// Format a cache timestamp relative to one report time.
fn format_last_use(time: Option<SystemTime>, now: SystemTime) -> String {
    let Some(time) = time else {
        return "unknown".to_string();
    };

    match now.duration_since(time) {
        Ok(elapsed) => TimeAgoFormatter::new().convert(elapsed),
        Err(error) => {
            let mut formatter = TimeAgoFormatter::new();
            formatter.ago("");
            let delta = formatter.convert(error.duration());
            if delta == "now" {
                delta
            } else {
                format!("in {delta}")
            }
        }
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
        io::stdout().write_all(content.as_bytes())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_cli(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn mcp_defaults_allow_server_scoped_flags() {
        let cli = parse_cli(&[
            "ruskel",
            "--mcp",
            "--cache-dir",
            "/tmp/ruskel-cache",
            "--private",
            "--no-frontmatter",
            "--offline",
            "--verbose",
        ]);

        let defaults = cli
            .mcp_defaults()
            .expect("server defaults should be accepted");

        assert!(defaults.private);
        assert!(!defaults.frontmatter);
        assert_eq!(cli.cache_dir, Some(PathBuf::from("/tmp/ruskel-cache")));
    }

    #[test]
    fn mcp_defaults_reject_request_scoped_flags() {
        let cli = parse_cli(&["ruskel", "--mcp", "--search", "widget"]);

        let error = cli.mcp_defaults().expect_err("search should be rejected");

        assert_eq!(error.to_string(), MCP_REQUEST_SCOPED_FLAGS_ERROR);
    }

    #[test]
    fn search_domains_fold_selected_flags() {
        let cli = parse_cli(&["ruskel", "--search-spec", "name,path"]);
        assert_eq!(
            cli.search_domains(),
            SearchDomain::NAMES | SearchDomain::PATHS
        );
    }

    #[test]
    fn crate_request_collects_feature_and_visibility_flags() {
        let cli = parse_cli(&[
            "ruskel",
            "--no-default-features",
            "--all-features",
            "--features",
            "derive,std",
            "--private",
        ]);

        assert_eq!(
            cli.crate_request(),
            CrateRequest {
                no_default_features: true,
                all_features: true,
                features: vec![String::from("derive"), String::from("std")],
                private_items: true,
            }
        );
    }

    #[test]
    fn request_scoped_flag_detection_tracks_search_options() {
        let cli = parse_cli(&["ruskel", "--mcp", "--search-case-sensitive"]);
        assert!(cli.uses_request_scoped_flags());
    }

    #[test]
    fn cache_values_use_human_readable_formats() {
        use std::time::Duration;

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);

        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1_572_864), "1.5 MiB");
        assert_eq!(
            format_path_with_home(
                Path::new("/home/alice/git/ruskel"),
                Some(Path::new("/home/alice"))
            ),
            Path::new("~").join("git/ruskel").display().to_string()
        );
        assert_eq!(
            format_path_with_home(Path::new("/home/alice"), Some(Path::new("/home/alice"))),
            "~"
        );
        assert_eq!(
            format_path_with_home(
                Path::new("/home/alice2/ruskel"),
                Some(Path::new("/home/alice"))
            ),
            "/home/alice2/ruskel"
        );
        assert_eq!(
            format_path_with_home(Path::new("/work/ruskel"), None),
            "/work/ruskel"
        );
        assert_eq!(format_workspace_label(None, "legacy-hash"), "legacy-hash");
        assert_eq!(
            format_last_use(Some(now - Duration::from_secs(42 * 60)), now),
            "42 minutes ago"
        );
        assert_eq!(
            format_last_use(Some(now + Duration::from_secs(2 * 60 * 60)), now),
            "in 2 hours"
        );
        assert_eq!(format_last_use(None, now), "unknown");
    }
}
