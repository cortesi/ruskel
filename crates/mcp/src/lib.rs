mod server;
mod tools;

pub use server::run_mcp_server;

#[cfg(test)]
mod tests {
    use crate::tools::RuskelSkeletonTool;
    use serde_json::json;

    #[test]
    fn test_tool_params_deserialization() {
        let params = json!({
            "target": "serde",
            "auto_impls": true,
            "private": false
        });
        
        let result: Result<RuskelSkeletonTool, _> = serde_json::from_value(params);
        assert!(result.is_ok());
        
        let tool = result.unwrap();
        assert_eq!(tool.target, "serde");
        assert_eq!(tool.auto_impls, true);
        assert_eq!(tool.private, false);
    }

    #[test]
    fn test_tool_params_defaults() {
        let params = json!({
            "target": "tokio"
        });
        
        let result: Result<RuskelSkeletonTool, _> = serde_json::from_value(params);
        assert!(result.is_ok());
        
        let tool = result.unwrap();
        assert_eq!(tool.target, "tokio");
        assert_eq!(tool.auto_impls, false);
        assert_eq!(tool.private, false);
        assert_eq!(tool.no_default_features, false);
        assert_eq!(tool.all_features, false);
        assert_eq!(tool.features.len(), 0);
        assert_eq!(tool.quiet, false);
        assert_eq!(tool.offline, false);
    }

    #[test] 
    fn test_tool_params_with_features() {
        let params = json!({
            "target": "tokio",
            "features": ["macros", "rt-multi-thread"],
            "no_default_features": true
        });
        
        let result: Result<RuskelSkeletonTool, _> = serde_json::from_value(params);
        assert!(result.is_ok());
        
        let tool = result.unwrap();
        assert_eq!(tool.target, "tokio");
        assert_eq!(tool.features, vec!["macros", "rt-multi-thread"]);
        assert_eq!(tool.no_default_features, true);
    }

    #[test]
    fn test_tool_params_missing_target() {
        let params = json!({
            "auto_impls": true
        });
        
        let result: Result<RuskelSkeletonTool, _> = serde_json::from_value(params);
        assert!(result.is_err());
    }
}