//! MCP Resources protocol handling.
//!
//! Implements the Resources capability where MCP servers expose data (files,
//! database schemas, application state) that can be read by the client. This
//! module provides:
//! - **McpRequestSender** trait: Abstraction for sending JSON-RPC requests to
//!   an MCP server (decouples from concrete `McpConnection`).
//! - **ResourcesManager**: Orchestrates resource listing and reading across
//!   multiple MCP servers, with caching.
//! - **Tool definitions**: Virtual tool schemas (`list_mcp_resources`,
//!   `list_mcp_resource_templates`, `read_mcp_resource`) exposed to the model.
//!
//! Design inspired by Codex's `mcp_resource_tool.rs` and `mcp_resource.rs`.

use std::collections::HashMap;

use crate::types::ToolDefinition;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::types::{McpResource, McpResourceContent, McpResourceTemplate};

// ---------------------------------------------------------------------------
// McpRequestSender trait
// ---------------------------------------------------------------------------

/// Trait for sending JSON-RPC requests to an MCP server.
///
/// This abstraction decouples the resources module from the concrete
/// `McpConnection` type, allowing independent compilation and easier testing.
#[async_trait::async_trait]
pub trait McpRequestSender: Send + Sync {
    /// Send a JSON-RPC request with the given method and optional params.
    /// Returns the `result` field from the response, or an error string.
    async fn send_request(&self, method: &str, params: Option<Value>) -> Result<Value, String>;
}

// ---------------------------------------------------------------------------
// ResourcesManager
// ---------------------------------------------------------------------------

/// Manages MCP resource operations across connected servers.
///
/// Provides caching for resource listings and methods for listing/reading
/// resources from individual servers or aggregated across all servers.
pub struct ResourcesManager {
    /// Cached resources per server: server_name → Vec<McpResource>
    resource_cache: RwLock<HashMap<String, Vec<McpResource>>>,
    /// Cached resource templates per server: server_name → Vec<McpResourceTemplate>
    template_cache: RwLock<HashMap<String, Vec<McpResourceTemplate>>>,
}

impl ResourcesManager {
    /// Create a new ResourcesManager with empty caches.
    pub fn new() -> Self {
        Self {
            resource_cache: RwLock::new(HashMap::new()),
            template_cache: RwLock::new(HashMap::new()),
        }
    }

    /// List resources from a specific server (with pagination support).
    ///
    /// Sends `resources/list` to the server and returns the resource list
    /// along with an optional next-page cursor.
    pub async fn list_resources(
        &self,
        sender: &dyn McpRequestSender,
        server_name: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<McpResource>, Option<String>), String> {
        let params = cursor.map(|c| serde_json::json!({"cursor": c}));

        debug!(server = %server_name, cursor = ?cursor, "Listing resources");

        let result = sender.send_request("resources/list", params).await?;

        let resources: Vec<McpResource> = result
            .get("resources")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let next_cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Update cache (only for the first page without cursor)
        if cursor.is_none() {
            let mut cache = self.resource_cache.write().await;
            cache.insert(server_name.to_string(), resources.clone());
        }

        debug!(
            server = %server_name,
            count = resources.len(),
            has_next = next_cursor.is_some(),
            "Listed resources"
        );

        Ok((resources, next_cursor))
    }

    /// List resource templates from a specific server.
    ///
    /// Sends `resources/templates/list` and returns templates with cursor.
    pub async fn list_resource_templates(
        &self,
        sender: &dyn McpRequestSender,
        server_name: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<McpResourceTemplate>, Option<String>), String> {
        let params = cursor.map(|c| serde_json::json!({"cursor": c}));

        debug!(server = %server_name, cursor = ?cursor, "Listing resource templates");

        let result = sender
            .send_request("resources/templates/list", params)
            .await?;

        let templates: Vec<McpResourceTemplate> = result
            .get("resourceTemplates")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let next_cursor = result
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Update cache (only for the first page)
        if cursor.is_none() {
            let mut cache = self.template_cache.write().await;
            cache.insert(server_name.to_string(), templates.clone());
        }

        debug!(
            server = %server_name,
            count = templates.len(),
            has_next = next_cursor.is_some(),
            "Listed resource templates"
        );

        Ok((templates, next_cursor))
    }

    /// Read a resource by URI from a specific server.
    ///
    /// Sends `resources/read` and returns the content items.
    pub async fn read_resource(
        &self,
        sender: &dyn McpRequestSender,
        server_name: &str,
        uri: &str,
    ) -> Result<Vec<McpResourceContent>, String> {
        let params = serde_json::json!({"uri": uri});

        debug!(server = %server_name, uri = %uri, "Reading resource");

        let result = sender.send_request("resources/read", Some(params)).await?;

        let contents: Vec<McpResourceContent> = result
            .get("contents")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if contents.is_empty() {
            warn!(
                server = %server_name,
                uri = %uri,
                "Resource read returned empty contents"
            );
        }

        debug!(
            server = %server_name,
            uri = %uri,
            content_count = contents.len(),
            "Read resource"
        );

        Ok(contents)
    }

    /// List resources from ALL connected servers (aggregated).
    ///
    /// Returns tuples of (server_name, resource) for each resource found.
    /// Uses cached data if available; otherwise queries each server.
    pub async fn list_all_resources(
        &self,
        senders: &HashMap<String, Box<dyn McpRequestSender>>,
    ) -> Vec<(String, McpResource)> {
        let mut all_resources = Vec::new();

        for (server_name, sender) in senders {
            match self
                .list_resources(sender.as_ref(), server_name, None)
                .await
            {
                Ok((resources, _cursor)) => {
                    for resource in resources {
                        all_resources.push((server_name.clone(), resource));
                    }
                }
                Err(e) => {
                    warn!(
                        server = %server_name,
                        error = %e,
                        "Failed to list resources from server"
                    );
                }
            }
        }

        all_resources
    }

    /// List resource templates from ALL connected servers (aggregated).
    pub async fn list_all_resource_templates(
        &self,
        senders: &HashMap<String, Box<dyn McpRequestSender>>,
    ) -> Vec<(String, McpResourceTemplate)> {
        let mut all_templates = Vec::new();

        for (server_name, sender) in senders {
            match self
                .list_resource_templates(sender.as_ref(), server_name, None)
                .await
            {
                Ok((templates, _cursor)) => {
                    for template in templates {
                        all_templates.push((server_name.clone(), template));
                    }
                }
                Err(e) => {
                    warn!(
                        server = %server_name,
                        error = %e,
                        "Failed to list resource templates from server"
                    );
                }
            }
        }

        all_templates
    }

    /// Get cached resources for a server (if previously fetched).
    pub async fn get_cached_resources(&self, server_name: &str) -> Option<Vec<McpResource>> {
        let cache = self.resource_cache.read().await;
        cache.get(server_name).cloned()
    }

    /// Get cached templates for a server (if previously fetched).
    pub async fn get_cached_templates(
        &self,
        server_name: &str,
    ) -> Option<Vec<McpResourceTemplate>> {
        let cache = self.template_cache.read().await;
        cache.get(server_name).cloned()
    }

    /// Invalidate all cached data for a specific server.
    pub async fn invalidate_cache(&self, server_name: &str) {
        {
            let mut cache = self.resource_cache.write().await;
            cache.remove(server_name);
        }
        {
            let mut cache = self.template_cache.write().await;
            cache.remove(server_name);
        }
        debug!(server = %server_name, "Invalidated resource cache");
    }

    /// Clear all cached data.
    pub async fn clear_cache(&self) {
        {
            let mut cache = self.resource_cache.write().await;
            cache.clear();
        }
        {
            let mut cache = self.template_cache.write().await;
            cache.clear();
        }
        debug!("Cleared all resource caches");
    }
}

impl Default for ResourcesManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ResourcesManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourcesManager")
            .field("resource_cache", &"<RwLock>")
            .field("template_cache", &"<RwLock>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tool Definitions
// ---------------------------------------------------------------------------

pub const LIST_MCP_RESOURCES_TOOL_NAME: &str = "list_mcp_resources";
pub const LIST_MCP_RESOURCE_TEMPLATES_TOOL_NAME: &str = "list_mcp_resource_templates";
pub const READ_MCP_RESOURCE_TOOL_NAME: &str = "read_mcp_resource";

pub fn is_resource_tool_name(name: &str) -> bool {
    matches!(
        name,
        LIST_MCP_RESOURCES_TOOL_NAME
            | LIST_MCP_RESOURCE_TEMPLATES_TOOL_NAME
            | READ_MCP_RESOURCE_TOOL_NAME
    )
}

/// Create the `list_mcp_resources` virtual tool definition.
///
/// This tool allows the model to list available resources from MCP servers.
pub fn list_resources_tool_def() -> Value {
    serde_json::json!({
        "name": LIST_MCP_RESOURCES_TOOL_NAME,
        "description": "Lists resources provided by MCP servers. Resources allow servers to share data that provides context to language models, such as files, database schemas, or application-specific information. Prefer resources over web search when possible.",
        "parameters": {
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional MCP server name. When omitted, lists resources from every configured server."
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque cursor returned by a previous list_mcp_resources call for the same server."
                }
            },
            "required": [],
            "additionalProperties": false
        }
    })
}

/// Create the `list_mcp_resource_templates` virtual tool definition.
///
/// This tool allows the model to list parameterized resource templates.
pub fn list_resource_templates_tool_def() -> Value {
    serde_json::json!({
        "name": LIST_MCP_RESOURCE_TEMPLATES_TOOL_NAME,
        "description": "Lists resource templates provided by MCP servers. Parameterized resource templates allow servers to share data that takes parameters and provides context to language models, such as files, database schemas, or application-specific information. Prefer resource templates over web search when possible.",
        "parameters": {
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional MCP server name. When omitted, lists resource templates from all configured servers."
                },
                "cursor": {
                    "type": "string",
                    "description": "Opaque cursor returned by a previous list_mcp_resource_templates call for the same server."
                }
            },
            "required": [],
            "additionalProperties": false
        }
    })
}

/// Create the `read_mcp_resource` virtual tool definition.
///
/// This tool allows the model to read a specific resource by URI.
pub fn read_resource_tool_def() -> Value {
    serde_json::json!({
        "name": READ_MCP_RESOURCE_TOOL_NAME,
        "description": "Read a specific resource from an MCP server given the server name and resource URI.",
        "parameters": {
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name exactly as configured. Must match the 'server' field returned by list_mcp_resources."
                },
                "uri": {
                    "type": "string",
                    "description": "Resource URI to read. Must be one of the URIs returned by list_mcp_resources."
                }
            },
            "required": ["server", "uri"],
            "additionalProperties": false
        }
    })
}

/// Generate all resource-related tool definitions.
///
/// Returns a vector of tool definitions that should be exposed to the model
/// when resource capabilities are available.
pub fn all_resource_tool_defs() -> Vec<Value> {
    vec![
        list_resources_tool_def(),
        list_resource_templates_tool_def(),
        read_resource_tool_def(),
    ]
}

pub fn all_resource_tool_definitions() -> Vec<ToolDefinition> {
    all_resource_tool_defs()
        .into_iter()
        .filter_map(|def| {
            Some(ToolDefinition::function(
                def.get("name")?.as_str()?.to_string(),
                def.get("description")?.as_str()?.to_string(),
                def.get("parameters").cloned(),
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // -------------------------------------------------------------------------
    // Mock McpRequestSender
    // -------------------------------------------------------------------------

    /// Mock sender that returns predefined responses based on method.
    struct MockSender {
        responses: Arc<Mutex<HashMap<String, Value>>>,
    }

    impl MockSender {
        fn new() -> Self {
            Self {
                responses: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        async fn set_response(&self, method: &str, response: Value) {
            let mut responses = self.responses.lock().await;
            responses.insert(method.to_string(), response);
        }
    }

    #[async_trait::async_trait]
    impl McpRequestSender for MockSender {
        async fn send_request(
            &self,
            method: &str,
            _params: Option<Value>,
        ) -> Result<Value, String> {
            let responses = self.responses.lock().await;
            responses
                .get(method)
                .cloned()
                .ok_or_else(|| format!("no mock response for method: {method}"))
        }
    }

    // -------------------------------------------------------------------------
    // Tool definition tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_list_resources_tool_def_structure() {
        let def = list_resources_tool_def();
        assert_eq!(def["name"], "list_mcp_resources");
        assert!(def["description"].as_str().unwrap().contains("resources"));
        assert_eq!(def["parameters"]["type"], "object");
        assert_eq!(def["parameters"]["additionalProperties"], false);
        assert!(def["parameters"]["properties"]["server"].is_object());
        assert!(def["parameters"]["properties"]["cursor"].is_object());
    }

    #[test]
    fn test_list_resource_templates_tool_def_structure() {
        let def = list_resource_templates_tool_def();
        assert_eq!(def["name"], "list_mcp_resource_templates");
        assert!(def["description"].as_str().unwrap().contains("templates"));
        assert_eq!(def["parameters"]["type"], "object");
    }

    #[test]
    fn test_read_resource_tool_def_structure() {
        let def = read_resource_tool_def();
        assert_eq!(def["name"], "read_mcp_resource");
        assert!(def["description"].as_str().unwrap().contains("Read"));
        assert_eq!(def["parameters"]["type"], "object");

        // server and uri are required
        let required = def["parameters"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("server")));
        assert!(required.contains(&json!("uri")));
    }

    #[test]
    fn test_all_resource_tool_defs_count() {
        let defs = all_resource_tool_defs();
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn test_all_resource_tool_definitions_are_function_tools() {
        let defs = all_resource_tool_definitions();
        let names = defs.iter().map(ToolDefinition::name).collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "list_mcp_resources",
                "list_mcp_resource_templates",
                "read_mcp_resource"
            ]
        );
    }

    #[test]
    fn test_tool_def_names_are_unique() {
        let defs = all_resource_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        let mut unique_names = names.clone();
        unique_names.sort();
        unique_names.dedup();
        assert_eq!(names.len(), unique_names.len());
    }

    // -------------------------------------------------------------------------
    // ResourcesManager tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_resources_basic() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({
                    "resources": [
                        {
                            "uri": "file:///test.txt",
                            "name": "test.txt",
                            "mimeType": "text/plain"
                        },
                        {
                            "uri": "file:///data.json",
                            "name": "data.json",
                            "description": "Config file",
                            "mimeType": "application/json"
                        }
                    ]
                }),
            )
            .await;

        let manager = ResourcesManager::new();
        let (resources, cursor) = manager
            .list_resources(&sender, "test-server", None)
            .await
            .unwrap();

        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].uri, "file:///test.txt");
        assert_eq!(resources[0].name, "test.txt");
        assert_eq!(resources[1].uri, "file:///data.json");
        assert_eq!(resources[1].description.as_deref(), Some("Config file"));
        assert!(cursor.is_none());
    }

    #[tokio::test]
    async fn test_list_resources_with_pagination() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({
                    "resources": [{"uri": "file:///a.txt", "name": "a.txt"}],
                    "nextCursor": "page2-token"
                }),
            )
            .await;

        let manager = ResourcesManager::new();
        let (resources, cursor) = manager
            .list_resources(&sender, "server", None)
            .await
            .unwrap();

        assert_eq!(resources.len(), 1);
        assert_eq!(cursor, Some("page2-token".to_string()));
    }

    #[tokio::test]
    async fn test_list_resources_empty() {
        let sender = MockSender::new();
        sender
            .set_response("resources/list", json!({"resources": []}))
            .await;

        let manager = ResourcesManager::new();
        let (resources, cursor) = manager
            .list_resources(&sender, "empty-server", None)
            .await
            .unwrap();

        assert!(resources.is_empty());
        assert!(cursor.is_none());
    }

    #[tokio::test]
    async fn test_list_resources_error() {
        let sender = MockSender::new();
        // No response set → will return error

        let manager = ResourcesManager::new();
        let result = manager.list_resources(&sender, "bad-server", None).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no mock response"));
    }

    #[tokio::test]
    async fn test_list_resource_templates_basic() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/templates/list",
                json!({
                    "resourceTemplates": [
                        {
                            "uriTemplate": "file:///projects/{project}/readme.md",
                            "name": "Project README",
                            "description": "README for a project",
                            "mimeType": "text/markdown"
                        }
                    ]
                }),
            )
            .await;

        let manager = ResourcesManager::new();
        let (templates, cursor) = manager
            .list_resource_templates(&sender, "server", None)
            .await
            .unwrap();

        assert_eq!(templates.len(), 1);
        assert_eq!(
            templates[0].uri_template,
            "file:///projects/{project}/readme.md"
        );
        assert_eq!(templates[0].name, "Project README");
        assert!(cursor.is_none());
    }

    #[tokio::test]
    async fn test_read_resource_text() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/read",
                json!({
                    "contents": [
                        {
                            "uri": "file:///test.txt",
                            "mimeType": "text/plain",
                            "text": "Hello, world!"
                        }
                    ]
                }),
            )
            .await;

        let manager = ResourcesManager::new();
        let contents = manager
            .read_resource(&sender, "server", "file:///test.txt")
            .await
            .unwrap();

        assert_eq!(contents.len(), 1);
        match &contents[0] {
            McpResourceContent::Text { uri, text, .. } => {
                assert_eq!(uri, "file:///test.txt");
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_read_resource_blob() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/read",
                json!({
                    "contents": [
                        {
                            "uri": "file:///image.png",
                            "mimeType": "image/png",
                            "blob": "iVBORw0KGgoAAAANSUhEUg=="
                        }
                    ]
                }),
            )
            .await;

        let manager = ResourcesManager::new();
        let contents = manager
            .read_resource(&sender, "server", "file:///image.png")
            .await
            .unwrap();

        assert_eq!(contents.len(), 1);
        match &contents[0] {
            McpResourceContent::Blob { uri, blob, .. } => {
                assert_eq!(uri, "file:///image.png");
                assert_eq!(blob, "iVBORw0KGgoAAAANSUhEUg==");
            }
            _ => panic!("Expected blob content"),
        }
    }

    #[tokio::test]
    async fn test_read_resource_empty_contents() {
        let sender = MockSender::new();
        sender
            .set_response("resources/read", json!({"contents": []}))
            .await;

        let manager = ResourcesManager::new();
        let contents = manager
            .read_resource(&sender, "server", "file:///empty")
            .await
            .unwrap();

        assert!(contents.is_empty());
    }

    // -------------------------------------------------------------------------
    // Cache tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_cache_populated_on_list() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({
                    "resources": [{"uri": "file:///cached.txt", "name": "cached.txt"}]
                }),
            )
            .await;

        let manager = ResourcesManager::new();

        // Initially no cache
        assert!(manager.get_cached_resources("server").await.is_none());

        // List populates cache
        manager
            .list_resources(&sender, "server", None)
            .await
            .unwrap();

        let cached = manager.get_cached_resources("server").await.unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].uri, "file:///cached.txt");
    }

    #[tokio::test]
    async fn test_cache_not_updated_with_cursor() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({
                    "resources": [{"uri": "file:///page2.txt", "name": "page2.txt"}]
                }),
            )
            .await;

        let manager = ResourcesManager::new();

        // List with cursor should NOT update cache
        manager
            .list_resources(&sender, "server", Some("page2-cursor"))
            .await
            .unwrap();

        assert!(manager.get_cached_resources("server").await.is_none());
    }

    #[tokio::test]
    async fn test_invalidate_cache() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({"resources": [{"uri": "file:///x.txt", "name": "x.txt"}]}),
            )
            .await;
        sender
            .set_response(
                "resources/templates/list",
                json!({"resourceTemplates": [{"uriTemplate": "t://{id}", "name": "tmpl"}]}),
            )
            .await;

        let manager = ResourcesManager::new();
        manager.list_resources(&sender, "s1", None).await.unwrap();
        manager
            .list_resource_templates(&sender, "s1", None)
            .await
            .unwrap();

        assert!(manager.get_cached_resources("s1").await.is_some());
        assert!(manager.get_cached_templates("s1").await.is_some());

        manager.invalidate_cache("s1").await;

        assert!(manager.get_cached_resources("s1").await.is_none());
        assert!(manager.get_cached_templates("s1").await.is_none());
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let sender = MockSender::new();
        sender
            .set_response(
                "resources/list",
                json!({"resources": [{"uri": "file:///a.txt", "name": "a"}]}),
            )
            .await;

        let manager = ResourcesManager::new();
        manager.list_resources(&sender, "s1", None).await.unwrap();
        manager.list_resources(&sender, "s2", None).await.unwrap();

        manager.clear_cache().await;

        assert!(manager.get_cached_resources("s1").await.is_none());
        assert!(manager.get_cached_resources("s2").await.is_none());
    }

    // -------------------------------------------------------------------------
    // list_all_resources tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_list_all_resources_aggregates() {
        let sender1 = MockSender::new();
        sender1
            .set_response(
                "resources/list",
                json!({"resources": [{"uri": "file:///s1.txt", "name": "s1.txt"}]}),
            )
            .await;

        let sender2 = MockSender::new();
        sender2
            .set_response(
                "resources/list",
                json!({"resources": [
                    {"uri": "file:///s2a.txt", "name": "s2a.txt"},
                    {"uri": "file:///s2b.txt", "name": "s2b.txt"}
                ]}),
            )
            .await;

        let manager = ResourcesManager::new();

        let mut senders: HashMap<String, Box<dyn McpRequestSender>> = HashMap::new();
        senders.insert("server1".to_string(), Box::new(sender1));
        senders.insert("server2".to_string(), Box::new(sender2));

        let all = manager.list_all_resources(&senders).await;
        assert_eq!(all.len(), 3);

        // Check that server names are correct
        let server_names: Vec<&str> = all.iter().map(|(s, _)| s.as_str()).collect();
        assert!(server_names.contains(&"server1"));
        assert!(server_names.contains(&"server2"));
    }

    #[tokio::test]
    async fn test_list_all_resources_handles_errors_gracefully() {
        // sender1 will fail (no response set)
        let sender1 = MockSender::new();

        let sender2 = MockSender::new();
        sender2
            .set_response(
                "resources/list",
                json!({"resources": [{"uri": "file:///ok.txt", "name": "ok.txt"}]}),
            )
            .await;

        let manager = ResourcesManager::new();

        let mut senders: HashMap<String, Box<dyn McpRequestSender>> = HashMap::new();
        senders.insert("failing-server".to_string(), Box::new(sender1));
        senders.insert("ok-server".to_string(), Box::new(sender2));

        // Should still return results from ok-server
        let all = manager.list_all_resources(&senders).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "ok-server");
    }

    // -------------------------------------------------------------------------
    // ResourcesManager default/debug tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resources_manager_default() {
        let manager = ResourcesManager::default();
        let debug_str = format!("{:?}", manager);
        assert!(debug_str.contains("ResourcesManager"));
    }
}
