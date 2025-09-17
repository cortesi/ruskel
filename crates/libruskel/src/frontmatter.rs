use std::fmt::Write;

use crate::search::{SearchDomain, describe_domains};

/// Configuration describing optional frontmatter comments rendered ahead of skeleton output.
#[derive(Debug, Clone, Default)]
pub struct FrontmatterConfig {
    /// Whether the frontmatter should be rendered.
    pub enabled: bool,
    /// Original target specification requested by the user.
    pub target: Option<String>,
    /// Optional metadata for search-driven renders.
    pub search: Option<FrontmatterSearch>,
    /// Canonical module path selected during target resolution.
    pub filter: Option<String>,
}

impl FrontmatterConfig {
    /// Create a configuration with frontmatter enabled for the provided target specification.
    pub fn for_target(target: impl Into<String>) -> Self {
        Self {
            enabled: true,
            target: Some(target.into()),
            search: None,
            filter: None,
        }
    }

    /// Disable frontmatter rendering entirely.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Attach the resolved filter path used for rendering.
    pub fn with_filter(mut self, filter: Option<String>) -> Self {
        self.filter = filter;
        self
    }

    /// Attach search metadata summarising the invocation.
    pub fn with_search(mut self, search: FrontmatterSearch) -> Self {
        self.search = Some(search);
        self
    }

    /// Render the configured frontmatter when enabled, returning the formatted comment block.
    pub fn render(
        &self,
        include_private: bool,
        render_auto_impls: bool,
        render_blanket_impls: bool,
    ) -> Option<String> {
        if !self.enabled {
            return None;
        }

        let mut output = String::new();
        output.push_str(
            "// Ruskel skeleton - syntactically valid Rust with implementation omitted.\n",
        );

        let mut settings = Vec::new();
        if let Some(target) = &self.target {
            settings.push(format!("target={target}"));
        }

        if let Some(filter) = &self.filter
            && !filter.is_empty()
        {
            settings.push(format!("path={filter}"));
        }

        let visibility = if include_private { "private" } else { "public" };
        settings.push(format!("visibility={visibility}"));
        settings.push(format!("auto_impls={render_auto_impls}"));
        settings.push(format!("blanket_impls={render_blanket_impls}"));

        writeln!(output, "// settings: {}", settings.join(", "))
            .expect("write frontmatter settings");

        if let Some(search) = &self.search {
            output.push('\n');
            write_search_section(&mut output, search);
        }
        output.push('\n');

        Some(output)
    }
}

/// Summary of a search invocation attached to the frontmatter.
#[derive(Debug, Clone)]
pub struct FrontmatterSearch {
    /// Query string executed against the index.
    pub query: String,
    /// Domains evaluated during matching.
    pub domains: SearchDomain,
    /// Whether matching respected case sensitivity.
    pub case_sensitive: bool,
    /// Whether matched containers were expanded to include their children.
    pub expand_containers: bool,
    /// Matched items included in the rendered skeleton.
    pub hits: Vec<FrontmatterHit>,
}

/// Individual search hit included in the frontmatter summary.
#[derive(Debug, Clone)]
pub struct FrontmatterHit {
    /// Canonical path representing the matched item.
    pub path: String,
    /// Domains that contributed to the match.
    pub domains: SearchDomain,
}

fn write_search_section(buffer: &mut String, search: &FrontmatterSearch) {
    let domains = describe_domains(search.domains);
    let mut details = String::new();

    if search.case_sensitive {
        details.push_str("case_sensitive=true");
    } else {
        details.push_str("case_sensitive=false");
    }

    if !domains.is_empty() {
        if !details.is_empty() {
            details.push_str("; ");
        }
        details.push_str("domains=");
        details.push_str(&domains.join(", "));
    }

    if !details.is_empty() {
        details.push_str("; ");
    }
    details.push_str("expand_containers=");
    details.push_str(if search.expand_containers {
        "true"
    } else {
        "false"
    });

    writeln!(
        buffer,
        "// search: query=\"{}\"{}",
        search.query,
        if details.is_empty() {
            String::new()
        } else {
            format!("; {}", details)
        }
    )
    .expect("write frontmatter search");

    if search.hits.is_empty() {
        return;
    }

    writeln!(buffer, "// hits ({}):", search.hits.len()).expect("write frontmatter hit count");
    for hit in &search.hits {
        let labels = describe_domains(hit.domains);
        if labels.is_empty() {
            writeln!(buffer, "//   - {}", hit.path).expect("write frontmatter hit path");
        } else {
            writeln!(buffer, "//   - {} [{}]", hit.path, labels.join(", "))
                .expect("write frontmatter hit labels");
        }
    }
}
