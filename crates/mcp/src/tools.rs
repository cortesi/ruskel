use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct RuskelSkeletonTool {
    /// Target to generate - a directory, file path, or a module name
    pub target: String,
    
    /// Render auto-implemented traits
    #[serde(default)]
    pub auto_impls: bool,
    
    /// Render private items
    #[serde(default)]
    pub private: bool,
    
    /// Disable default features
    #[serde(default)]
    pub no_default_features: bool,
    
    /// Enable all features
    #[serde(default)]
    pub all_features: bool,
    
    /// Specify features to enable
    #[serde(default)]
    pub features: Vec<String>,
    
    /// Enable quiet mode
    #[serde(default)]
    pub quiet: bool,
    
    /// Enable offline mode
    #[serde(default)]
    pub offline: bool,
}