use bifrost_storage::{MAX_BREAKPOINT_TIMEOUT_MS, MIN_BREAKPOINT_TIMEOUT_MS};
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct OpenApiSpec {
    pub openapi: String,
    pub info: OpenApiInfo,
    pub servers: Vec<OpenApiServer>,
    pub paths: serde_json::Value,
    pub components: serde_json::Value,
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenApiInfo {
    pub title: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct OpenApiServer {
    pub url: String,
    pub description: String,
}

pub fn generate_openapi_spec() -> OpenApiSpec {
    let paths = generate_paths();
    let components = generate_components();

    OpenApiSpec {
        openapi: "3.0.3".to_string(),
        info: OpenApiInfo {
            title: "Bifrost Admin API".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            description: "Bifrost Proxy - Admin API for Web UI, CLI, and proxy management tools."
                .to_string(),
        },
        servers: vec![OpenApiServer {
            url: "/_bifrost".to_string(),
            description: "Bifrost Proxy (relative path)".to_string(),
        }],
        paths,
        components,
    }
}

fn generate_components() -> serde_json::Value {
    serde_json::json!({
        "securitySchemes": {
            "bearerAuth": {
                "type": "http",
                "scheme": "bearer",
                "bearerFormat": "JWT",
                "description": "JWT token from /api/auth/login"
            }
        },
        "schemas": {
            "BreakpointSettings": {
                "type": "object",
                "description": "Global breakpoint gate used by the toolbar and admin API. A matching breakpoint://request or breakpoint://response rule is also required before any traffic can pause.",
                "properties": {
                    "enabled": {"type": "boolean", "description": "Enable breakpoint globally. A matched breakpoint://request or breakpoint://response rule is still required before traffic can pause."},
                    "max_body_bytes": {"type": "integer", "description": "Body size limit for breakpoint UI capture to avoid copying large payloads"}
                }
            },
            "PerformanceBreakpointConfig": {
                "type": "object",
                "description": "Breakpoint timeout shown in Settings > Performance and applied to pending request/response pauses.",
                "properties": {
                    "timeout_ms": {"type": "integer", "description": "Auto-resume timeout for paused breakpoint traffic"},
                    "timeout_min_ms": {"type": "integer", "description": "Fixed minimum accepted breakpoint timeout"},
                    "timeout_max_ms": {"type": "integer", "description": "Fixed maximum accepted breakpoint timeout"}
                }
            },
            "PerformanceTrafficConfig": {
                "type": "object",
                "properties": {
                    "max_records": {"type": "integer"},
                    "max_db_size_bytes": {"type": "integer"},
                    "max_body_memory_size": {"type": "integer"},
                    "max_body_buffer_size": {"type": "integer"},
                    "max_body_probe_size": {"type": "integer"},
                    "super_performance_mode": {"type": "boolean", "description": "When true, proxy rules still run but traffic records, bodies, frames, WebSocket payloads, and traffic database updates are not persisted."},
                    "binary_traffic_performance_mode": {"type": "boolean"},
                    "inject_bifrost_badge": {"type": "boolean"},
                    "file_retention_days": {"type": "integer"},
                    "sse_stream_flush_bytes": {"type": "integer"},
                    "sse_stream_flush_interval_ms": {"type": "integer"},
                    "ws_payload_flush_bytes": {"type": "integer"},
                    "ws_payload_flush_interval_ms": {"type": "integer"},
                    "ws_payload_max_open_files": {"type": "integer"}
                }
            },
            "PerformanceConfig": {
                "type": "object",
                "properties": {
                    "traffic": {"$ref": "#/components/schemas/PerformanceTrafficConfig"},
                    "breakpoint": {"$ref": "#/components/schemas/PerformanceBreakpointConfig"},
                    "body_store_stats": {"type": "object", "nullable": true},
                    "frame_store_stats": {"type": "object", "nullable": true},
                    "ws_payload_store_stats": {"type": "object", "nullable": true},
                    "resource_alerts": {"type": "object"}
                }
            },
            "UpdatePerformanceConfigRequest": {
                "type": "object",
                "description": "Partial update for Settings > Performance. Omit fields that should stay unchanged.",
                "properties": {
                    "max_records": {"type": "integer"},
                    "max_db_size_bytes": {"type": "integer"},
                    "max_body_memory_size": {"type": "integer"},
                    "max_body_buffer_size": {"type": "integer"},
                    "max_body_probe_size": {"type": "integer"},
                    "super_performance_mode": {"type": "boolean", "description": "Enable or disable Super Performance Mode."},
                    "binary_traffic_performance_mode": {"type": "boolean"},
                    "inject_bifrost_badge": {"type": "boolean"},
                    "file_retention_days": {"type": "integer"},
                    "sse_stream_flush_bytes": {"type": "integer"},
                    "sse_stream_flush_interval_ms": {"type": "integer"},
                    "ws_payload_flush_bytes": {"type": "integer"},
                    "ws_payload_flush_interval_ms": {"type": "integer"},
                    "ws_payload_max_open_files": {"type": "integer"},
                    "breakpoint_timeout_ms": {
                        "type": "integer",
                        "minimum": MIN_BREAKPOINT_TIMEOUT_MS,
                        "maximum": MAX_BREAKPOINT_TIMEOUT_MS,
                        "description": "Auto-resume timeout for Breakpoint. Configure phases with breakpoint://request or breakpoint://response rules; this field only controls how long a matched pause may wait."
                    }
                }
            },
            "TrayConfig": {
                "type": "object",
                "description": "System tray helper settings shown in Settings > Tray.",
                "properties": {
                    "enabled": {"type": "boolean", "description": "Whether Bifrost should launch and keep the tray helper running"},
                    "supported": {"type": "boolean", "description": "Whether the current platform supports the native tray helper"},
                    "system_stats_supported": {"type": "boolean", "description": "Whether the current platform supports tray system status metrics. macOS supports this; Windows and Linux return false."},
                    "show_system_stats": {"type": "boolean", "description": "Whether the tray should show whole-system CPU, memory, disk, and network throughput when system_stats_supported is true"},
                    "system_stats_items": {"$ref": "#/components/schemas/TraySystemStatsItems"}
                }
            },
            "TraySystemStatsItems": {
                "type": "object",
                "description": "Per-metric tray system status visibility flags. Missing fields default to true.",
                "properties": {
                    "cpu": {"type": "boolean"},
                    "memory": {"type": "boolean"},
                    "disk": {"type": "boolean"},
                    "upload": {"type": "boolean"},
                    "download": {"type": "boolean"}
                }
            },
            "UpdateTrayConfigRequest": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean"},
                    "show_system_stats": {"type": "boolean", "description": "Only accepted on platforms where TrayConfig.system_stats_supported is true"},
                    "system_stats_items": {"allOf": [{"$ref": "#/components/schemas/TraySystemStatsItems"}], "description": "Only accepted on platforms where TrayConfig.system_stats_supported is true"}
                }
            },
            "BreakpointEdit": {
                "type": "object",
                "required": ["request_id", "phase"],
                "properties": {
                    "request_id": {"type": "string", "description": "ID of the request currently paused by breakpoint"},
                    "phase": {"type": "string", "enum": ["request", "response"], "description": "Breakpoint phase"},
                    "method": {"type": "string", "nullable": true, "description": "Edited request method; accepted only for request phase"},
                    "url": {"type": "string", "format": "uri", "nullable": true, "description": "Edited absolute HTTP(S) request URL; accepted only for request phase"},
                    "status": {"type": "integer", "minimum": 100, "maximum": 599, "nullable": true, "description": "Edited response status; accepted only for response phase"},
                    "headers": {
                        "type": "array",
                        "nullable": true,
                        "description": "Edited ordered header pairs. Duplicate names are preserved. Omit to keep the original headers.",
                        "items": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 2,
                            "items": {"type": "string"}
                        }
                    },
                    "body": {
                        "type": "string",
                        "nullable": true,
                        "description": "Edited body. null keeps the original body"
                    }
                }
            },
            "PendingBreakpoint": {
                "type": "object",
                "required": ["request_id", "phase", "headers", "body_omitted", "max_body_bytes", "paused_at_ms", "deadline_at_ms", "server_now_ms"],
                "properties": {
                    "request_id": {"type": "string"},
                    "phase": {"type": "string", "enum": ["request", "response"]},
                    "method": {"type": "string", "nullable": true},
                    "url": {"type": "string", "nullable": true},
                    "status": {"type": "integer", "nullable": true},
                    "headers": {
                        "type": "array",
                        "items": {"type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "string"}}
                    },
                    "body": {"type": "string", "nullable": true},
                    "body_omitted": {"type": "boolean"},
                    "body_size": {"type": "integer", "nullable": true},
                    "max_body_bytes": {"type": "integer"},
                    "content_encoding": {"type": "string", "nullable": true},
                    "paused_at_ms": {"type": "integer", "format": "int64"},
                    "deadline_at_ms": {"type": "integer", "format": "int64"},
                    "server_now_ms": {"type": "integer", "format": "int64"}
                }
            },
            "ErrorResponse": {
                "type": "object",
                "properties": {
                    "error": {"type": "string"},
                    "status": {"type": "integer"}
                }
            },
            "LoginRequest": {
                "type": "object",
                "required": ["username", "password"],
                "properties": {
                    "username": {"type": "string"},
                    "password": {"type": "string"}
                }
            },
            "LoginResponse": {
                "type": "object",
                "properties": {
                    "token": {"type": "string", "description": "JWT token"},
                    "username": {"type": "string"}
                }
            },
            "RuleInfo": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "content": {"type": "string", "description": "Rule DSL content. Breakpoint phases are selected with breakpoint://request and breakpoint://response values inside rules."},
                    "enabled": {"type": "boolean"}
                }
            },
            "ScriptInfo": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "script_type": {"type": "string", "enum": ["request", "response", "decode"]},
                    "content": {"type": "string"},
                    "enabled": {"type": "boolean"}
                }
            },
            "TrafficRecord": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Unique ID, e.g. REQ-6a1f6c82-000318"},
                    "seq": {"type": "integer"},
                    "ts": {"type": "integer", "description": "Timestamp (ms)"},
                    "m": {"type": "string", "description": "HTTP method"},
                    "h": {"type": "string", "description": "Host"},
                    "p": {"type": "string", "description": "Path"},
                    "s": {"type": "integer", "description": "Status code"},
                    "proto": {"type": "string", "enum": ["http", "https", "ws", "wss"]},
                    "dur": {"type": "integer", "description": "Duration (ms)"},
                    "lp": {"type": "integer", "description": "Listener port"},
                    "cip": {"type": "string", "description": "Client IP"},
                    "capp": {"type": "string", "description": "Client app"}
                }
            },
            "TrafficListResponse": {
                "type": "object",
                "properties": {
                    "records": {"type": "array", "items": {"$ref": "#/components/schemas/TrafficRecord"}},
                    "total": {"type": "integer"},
                    "next_cursor": {"type": "integer", "nullable": true},
                    "has_more": {"type": "boolean"}
                }
            },
            "WhitelistEntry": {
                "type": "object",
                "properties": {
                    "ip": {"type": "string", "description": "IP or CIDR"},
                    "type": {"type": "string", "enum": ["permanent", "temporary"]}
                }
            },
            "ConfigTlsRequest": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "description": "TLS interception switch"},
                    "unsafe_ssl": {"type": "boolean", "description": "Skip upstream certificate verification"},
                    "include": {"type": "array", "items": {"type": "string"}, "description": "Domain whitelist"},
                    "exclude": {"type": "array", "items": {"type": "string"}, "description": "Domain passthrough"},
                    "app_include": {"type": "array", "items": {"type": "string"}, "description": "App whitelist"},
                    "app_exclude": {"type": "array", "items": {"type": "string"}, "description": "App passthrough"}
                }
            },
            "SearchQuery": {
                "type": "object",
                "properties": {
                    "keyword": {"type": "string", "description": "Search keyword"},
                    "limit": {"type": "integer", "default": 50},
                    "method": {"type": "string"},
                    "host": {"type": "string"},
                    "status": {"type": "string", "enum": ["2xx", "3xx", "4xx", "5xx", "error"]},
                    "protocol": {"type": "string", "enum": ["HTTP", "HTTPS", "WS", "WSS"]},
                    "req_body": {"type": "boolean", "description": "Search in request body"},
                    "res_body": {"type": "boolean", "description": "Search in response body"}
                }
            },
            "ReplayRequest": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Target URL"},
                    "method": {"type": "string", "description": "HTTP method"},
                    "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                    "body": {"type": "string", "nullable": true}
                }
            },
            "SystemOverview": {
                "type": "object",
                "properties": {
                    "rules_count": {"type": "integer"},
                    "traffic_count": {"type": "integer"},
                    "version": {"type": "string"},
                    "uptime_seconds": {"type": "integer"}
                }
            },
            "CertInfo": {
                "type": "object",
                "properties": {
                    "installed": {"type": "boolean"},
                    "trusted": {"type": "boolean"},
                    "status_message": {"type": "string"}
                }
            }
        }
    })
}

fn generate_paths() -> serde_json::Value {
    serde_json::json!({
        // ═══════════════════════════════════════════════════════
        // Breakpoint
        // ═══════════════════════════════════════════════════════
        "/api/breakpoint/settings": {
            "get": {
                "tags": ["Breakpoint"],
                "summary": "Get breakpoint settings",
                "description": "Returns the global Breakpoint toolbar gate and capture size limit. This endpoint does not configure request/response phases; phases are configured in rules.",
                "operationId": "getBreakpointSettings",
                "responses": {
                    "200": {
                        "description": "Current breakpoint settings",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointSettings"}}}
                    }
                }
            },
            "post": {
                "tags": ["Breakpoint"],
                "summary": "Update breakpoint settings",
                "description": "Updates the global Breakpoint gate. Traffic only pauses when this gate is enabled and a matched rule authorizes breakpoint://request or breakpoint://response.",
                "operationId": "updateBreakpointSettings",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointSettings"}}}
                },
                "responses": {
                    "200": {"description": "Updated"},
                    "400": {"description": "Bad request"}
                }
            }
        },
        "/api/breakpoint/pending": {
            "get": {
                "tags": ["Breakpoint"],
                "summary": "List currently paused breakpoint requests/responses",
                "description": "Returns authoritative pause snapshots so the UI can recover after a reload or missed push event.",
                "operationId": "getPendingBreakpoints",
                "responses": {
                    "200": {
                        "description": "Pending breakpoints ordered by pause time",
                        "content": {"application/json": {"schema": {"type": "array", "items": {"$ref": "#/components/schemas/PendingBreakpoint"}}}}
                    }
                }
            }
        },
        "/api/breakpoint/resume": {
            "post": {
                "tags": ["Breakpoint"],
                "summary": "Resume paused breakpoint request/response",
                "description": "Submit edited headers and body from the Traffic detail UI or an external admin client to continue the paused request/response.",
                "operationId": "resumeBreakpoint",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointEdit"}}}
                },
                "responses": {
                    "200": {"description": "Resumed"},
                    "404": {"description": "Pending request not found"},
                    "409": {"description": "The request is paused at a different phase"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Auth
        // ═══════════════════════════════════════════════════════
        "/api/auth/status": {
            "get": {
                "tags": ["Auth"],
                "summary": "Authentication status",
                "operationId": "getAuthStatus",
                "responses": {
                    "200": {"description": "Auth status"}
                }
            }
        },
        "/api/auth/login": {
            "post": {
                "tags": ["Auth"],
                "summary": "Log in",
                "operationId": "login",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginRequest"}}}
                },
                "responses": {
                    "200": {
                        "description": "Login succeeded",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginResponse"}}}
                    },
                    "401": {"description": "Incorrect username or password"}
                }
            }
        },
        "/api/auth/logout": {
            "post": {
                "tags": ["Auth"],
                "summary": "Log out",
                "operationId": "logout",
                "responses": {"200": {"description": "Logged out"}}
            }
        },
        "/api/auth/passwd": {
            "post": {
                "tags": ["Auth"],
                "summary": "Change password",
                "operationId": "changePassword",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["current_password", "new_password"],
                        "properties": {
                            "current_password": {"type": "string"},
                            "new_password": {"type": "string"}
                        }
                    }}}
                },
                "responses": {
                    "200": {"description": "Password changed"},
                    "401": {"description": "Incorrect current password"}
                }
            }
        },
        "/api/auth/remote": {
            "post": {
                "tags": ["Auth"],
                "summary": "Enable or disable remote access",
                "operationId": "toggleRemoteAccess",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"enabled": {"type": "boolean"}}
                    }}}
                },
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/auth/revoke-all": {
            "post": {
                "tags": ["Auth"],
                "summary": "Revoke all JWT sessions",
                "operationId": "revokeAllSessions",
                "responses": {"200": {"description": "Revoked"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Traffic
        // ═══════════════════════════════════════════════════════
        "/api/traffic": {
            "get": {
                "tags": ["Traffic"],
                "summary": "List traffic records",
                "operationId": "listTraffic",
                "parameters": [
                    {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 50}},
                    {"name": "cursor", "in": "query", "schema": {"type": "integer"}, "description": "Pagination cursor"},
                    {"name": "method", "in": "query", "schema": {"type": "string"}},
                    {"name": "host", "in": "query", "schema": {"type": "string"}},
                    {"name": "status", "in": "query", "schema": {"type": "integer"}},
                    {"name": "protocol", "in": "query", "schema": {"type": "string", "enum": ["http", "https", "ws", "wss"]}},
                    {"name": "listener_port", "in": "query", "schema": {"type": "integer"}, "description": "Proxy port filter"},
                    {"name": "has_rule_hit", "in": "query", "schema": {"type": "boolean"}, "description": "Filter rule hit"},
                    {"name": "client_app", "in": "query", "schema": {"type": "string"}},
                    {"name": "format", "in": "query", "schema": {"type": "string", "enum": ["table", "compact", "json", "json-pretty"]}}
                ],
                "responses": {
                    "200": {
                        "description": "Traffic list",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TrafficListResponse"}}}
                    }
                }
            },
            "delete": {
                "tags": ["Traffic"],
                "summary": "Delete traffic records",
                "operationId": "deleteTraffic",
                "requestBody": {
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"ids": {"type": "array", "items": {"type": "string"}}}
                    }}}
                },
                "responses": {"200": {"description": "Deleted"}}
            }
        },
        "/api/traffic/{id}": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Get traffic record details",
                "operationId": "getTraffic",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}, "description": "Traffic record ID, e.g. REQ-6a1f6c82-000318"}
                ],
                "responses": {
                    "200": {"description": "Traffic record details"},
                    "404": {"description": "Not found"}
                }
            }
        },
        "/api/traffic/{id}/request-body": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Get request body",
                "operationId": "getTrafficRequestBody",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Request body"},
                    "404": {"description": "Not found"}
                }
            }
        },
        "/api/traffic/{id}/response-body": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Get response body",
                "operationId": "getTrafficResponseBody",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Response body"},
                    "404": {"description": "Not found"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Search
        // ═══════════════════════════════════════════════════════
        "/api/search": {
            "post": {
                "tags": ["Search"],
                "summary": "Search traffic (full-text)",
                "operationId": "searchTraffic",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SearchQuery"}}}
                },
                "responses": {
                    "200": {"description": "Search results"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Rules
        // ═══════════════════════════════════════════════════════
        "/api/rules": {
            "get": {
                "tags": ["Rules"],
                "summary": "List all rules",
                "description": "Lists rule DSL entries. Breakpoint uses this generic rules API: add breakpoint://request or breakpoint://response in a matched rule to authorize pausing that phase.",
                "operationId": "listRules",
                "responses": {
                    "200": {
                        "description": "Rules list",
                        "content": {"application/json": {"schema": {
                            "type": "array", "items": {"$ref": "#/components/schemas/RuleInfo"}
                        }}}
                    }
                }
            },
            "post": {
                "tags": ["Rules"],
                "summary": "Create rule",
                "description": "Creates a rule DSL entry. For Breakpoint, use breakpoint://request, breakpoint://response, or both values in matched rules.",
                "operationId": "createRule",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["name", "content"],
                        "properties": {
                            "name": {"type": "string"},
                            "content": {"type": "string", "description": "Rule content"}
                        }
                    }}}
                },
                "responses": {
                    "200": {"description": "Created"},
                    "400": {"description": "Rule already exists"}
                }
            }
        },
        "/api/rules/{name}": {
            "get": {
                "tags": ["Rules"],
                "summary": "Get rule details",
                "description": "Gets a rule DSL entry, including any breakpoint://request or breakpoint://response phase configuration.",
                "operationId": "getRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Rule details"},
                    "404": {"description": "Not found"}
                }
            },
            "put": {
                "tags": ["Rules"],
                "summary": "Update rule",
                "description": "Updates a rule DSL entry. Breakpoint phase configuration lives here, not in /api/breakpoint/settings.",
                "operationId": "updateRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"content": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Updated"}}
            },
            "delete": {
                "tags": ["Rules"],
                "summary": "Delete rule",
                "operationId": "deleteRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Deleted"}}
            }
        },
        "/api/rules/{name}/enable": {
            "put": {
                "tags": ["Rules"],
                "summary": "Enable rule",
                "operationId": "enableRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Enabled"}}
            }
        },
        "/api/rules/{name}/disable": {
            "put": {
                "tags": ["Rules"],
                "summary": "Disable rule",
                "operationId": "disableRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Disabled"}}
            }
        },
        "/api/rules/active-summary": {
            "get": {
                "tags": ["Rules"],
                "summary": "Active rules summary",
                "operationId": "getActiveRules",
                "responses": {"200": {"description": "Active rules list"}}
            }
        },
        "/api/rules/reorder": {
            "put": {
                "tags": ["Rules"],
                "summary": "Reorder rules",
                "operationId": "reorderRules",
                "responses": {"200": {"description": "Reordered"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Scripts
        // ═══════════════════════════════════════════════════════
        "/api/scripts": {
            "get": {
                "tags": ["Scripts"],
                "summary": "List scripts",
                "operationId": "listScripts",
                "parameters": [
                    {"name": "type", "in": "query", "schema": {"type": "string", "enum": ["request", "response", "decode"]}}
                ],
                "responses": {
                    "200": {
                        "description": "Scripts list",
                        "content": {"application/json": {"schema": {
                            "type": "array", "items": {"$ref": "#/components/schemas/ScriptInfo"}
                        }}}
                    }
                }
            },
            "put": {
                "tags": ["Scripts"],
                "summary": "Upload script",
                "operationId": "uploadScript",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["name", "type", "content"],
                        "properties": {
                            "name": {"type": "string"},
                            "type": {"type": "string", "enum": ["request", "response", "decode"]},
                            "content": {"type": "string", "description": "JS source code"}
                        }
                    }}}
                },
                "responses": {"200": {"description": "Uploaded"}}
            }
        },
        "/api/scripts/test": {
            "post": {
                "tags": ["Scripts"],
                "summary": "Test script with mock data",
                "operationId": "testScript",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": {"type": "string"},
                            "type": {"type": "string", "enum": ["request", "response", "decode"]}
                        }
                    }}}
                },
                "responses": {"200": {"description": "Test result"}}
            }
        },
        "/api/scripts/{name}": {
            "get": {
                "tags": ["Scripts"],
                "summary": "Get script",
                "operationId": "getScript",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Script content"}}
            },
            "delete": {
                "tags": ["Scripts"],
                "summary": "Delete script",
                "operationId": "deleteScript",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Deleted"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Values
        // ═══════════════════════════════════════════════════════
        "/api/values": {
            "get": {
                "tags": ["Values"],
                "summary": "List all values",
                "operationId": "listValues",
                "responses": {
                    "200": {"description": "Values list (key-value)"}
                }
            },
            "post": {
                "tags": ["Values"],
                "summary": "Create a value",
                "operationId": "createValue",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["name", "value"],
                        "properties": {
                            "name": {"type": "string"},
                            "value": {"type": "string"}
                        }
                    }}}
                },
                "responses": {"200": {"description": "Created"}}
            },
            "put": {
                "tags": ["Values"],
                "summary": "Batch upsert values",
                "operationId": "upsertValues",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["values"],
                        "properties": {
                            "values": {
                                "type": "object",
                                "additionalProperties": {"type": "string"}
                            }
                        }
                    }}}
                },
                "responses": {"200": {"description": "Values upserted"}}
            }
        },
        "/api/values/{name}": {
            "put": {
                "tags": ["Values"],
                "summary": "Set/update value",
                "operationId": "setValue",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Saved"}}
            },
            "delete": {
                "tags": ["Values"],
                "summary": "Delete value",
                "operationId": "deleteValue",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Deleted"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Whitelist
        // ═══════════════════════════════════════════════════════
        "/api/whitelist": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "List whitelist",
                "operationId": "listWhitelist",
                "responses": {
                    "200": {"description": "Whitelist list"}
                }
            },
            "post": {
                "tags": ["Whitelist"],
                "summary": "Add IP/CIDR to whitelist",
                "operationId": "addWhitelist",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string", "description": "IP or CIDR, e.g. 192.168.1.0/24"}}
                    }}}
                },
                "responses": {"200": {"description": "Added"}}
            }
        },
        "/api/whitelist/mode": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Get access mode",
                "operationId": "getAccessMode",
                "responses": {"200": {"description": "Current access mode"}}
            },
            "put": {
                "tags": ["Whitelist"],
                "summary": "Set access mode",
                "operationId": "setAccessMode",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"mode": {"type": "string", "enum": ["local_only", "whitelist", "interactive", "allow_all"]}}
                    }}}
                },
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/whitelist/allow-lan": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "allow-lan status",
                "operationId": "getAllowLan",
                "responses": {"200": {"description": "allow-lan status"}}
            },
            "put": {
                "tags": ["Whitelist"],
                "summary": "Set allow-lan",
                "operationId": "setAllowLan",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"allow_lan": {"type": "boolean"}}
                    }}}
                },
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/whitelist/pending": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Pending requests list",
                "operationId": "getPendingWhitelist",
                "responses": {"200": {"description": "Pending authorization requests"}}
            }
        },
        "/api/whitelist/pending/approve": {
            "post": {
                "tags": ["Whitelist"],
                "summary": "Approve pending request",
                "operationId": "approvePending",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Approved"}}
            }
        },
        "/api/whitelist/pending/reject": {
            "post": {
                "tags": ["Whitelist"],
                "summary": "Reject pending request",
                "operationId": "rejectPending",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Rejected"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Config
        // ═══════════════════════════════════════════════════════
        "/api/config": {
            "get": {
                "tags": ["Config"],
                "summary": "Get full proxy configuration",
                "operationId": "getConfig",
                "responses": {"200": {"description": "Current configuration"}}
            }
        },
        "/api/config/tls": {
            "get": {
                "tags": ["Config"],
                "summary": "Get TLS interception configuration",
                "operationId": "getTlsConfig",
                "responses": {"200": {"description": "TLS configuration"}}
            },
            "put": {
                "tags": ["Config"],
                "summary": "Update TLS configuration",
                "operationId": "updateTlsConfig",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ConfigTlsRequest"}}}
                },
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/config/tray": {
            "get": {
                "tags": ["Config"],
                "summary": "Get tray helper configuration",
                "operationId": "getTrayConfig",
                "responses": {
                    "200": {
                        "description": "Tray configuration",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TrayConfig"}}}
                    }
                }
            },
            "put": {
                "tags": ["Config"],
                "summary": "Update tray helper configuration",
                "operationId": "updateTrayConfig",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/UpdateTrayConfigRequest"}}}
                },
                "responses": {
                    "200": {
                        "description": "Updated tray configuration",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TrayConfig"}}}
                    }
                }
            }
        },
        "/api/config/server": {
            "get": {
                "tags": ["Config"],
                "summary": "Get server configuration",
                "operationId": "getServerConfig",
                "responses": {"200": {"description": "Server configuration"}}
            },
            "put": {
                "tags": ["Config"],
                "summary": "Update server configuration",
                "operationId": "updateServerConfig",
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/config/performance": {
            "get": {
                "tags": ["Config"],
                "summary": "Get performance configuration",
                "description": "Returns traffic performance settings plus Breakpoint auto-resume timeout and fixed safety bounds.",
                "operationId": "getPerformanceConfig",
                "responses": {
                    "200": {
                        "description": "Performance configuration",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/PerformanceConfig"}}}
                    }
                }
            },
            "put": {
                "tags": ["Config"],
                "summary": "Update performance configuration",
                "description": "Partially updates performance settings. Breakpoint timeout is configured here so the global Breakpoint settings stay focused on enablement and capture limits.",
                "operationId": "updatePerformanceConfig",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/UpdatePerformanceConfigRequest"}}}
                },
                "responses": {
                    "200": {
                        "description": "Updated",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/PerformanceConfig"}}}
                    },
                    "400": {"description": "Invalid performance value"}
                }
            }
        },
        "/api/config/connections": {
            "get": {
                "tags": ["Config"],
                "summary": "Active connections list",
                "operationId": "listConnections",
                "responses": {"200": {"description": "Active connections"}}
            }
        },
        "/api/config/connections/disconnect": {
            "post": {
                "tags": ["Config"],
                "summary": "Disconnect by domain",
                "operationId": "disconnectByDomain",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["domain"],
                        "properties": {"domain": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Disconnected"}}
            }
        },
        "/api/config/performance/clear-cache": {
            "delete": {
                "tags": ["Config"],
                "summary": "Clear body cache",
                "operationId": "clearBodyCache",
                "responses": {"200": {"description": "Deleted cache"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // System
        // ═══════════════════════════════════════════════════════
        "/api/system": {
            "get": {
                "tags": ["System"],
                "summary": "Basic system information",
                "operationId": "getSystemInfo",
                "responses": {
                    "200": {
                        "description": "System information",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SystemOverview"}}}
                    }
                }
            }
        },
        "/api/system/overview": {
            "get": {
                "tags": ["System"],
                "summary": "Overview (rules/traffic/pending count)",
                "operationId": "getSystemOverview",
                "responses": {"200": {"description": "System overview"}}
            }
        },
        "/api/system/memory": {
            "get": {
                "tags": ["System"],
                "summary": "Memory diagnostics",
                "operationId": "getMemoryDiagnostics",
                "responses": {"200": {"description": "Memory diagnostics"}}
            }
        },
        "/api/system/version-check": {
            "get": {
                "tags": ["System"],
                "summary": "Check for a new version",
                "operationId": "checkVersion",
                "parameters": [
                    {"name": "refresh", "in": "query", "schema": {"type": "boolean"}}
                ],
                "responses": {"200": {"description": "Version check result"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Cert
        // ═══════════════════════════════════════════════════════
        "/api/cert": {
            "get": {
                "tags": ["Cert"],
                "summary": "CA certificate information",
                "operationId": "getCertInfo",
                "responses": {
                    "200": {
                        "description": "Certificate information",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CertInfo"}}}
                    }
                }
            }
        },
        "/api/cert/install": {
            "post": {
                "tags": ["Cert"],
                "summary": "Install and trust local CA certificate",
                "operationId": "installLocalCa",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "confirmation": {"type": "string"}
                        },
                        "required": ["confirmation"]
                    }}}
                },
                "responses": {
                    "200": {
                        "description": "Updated certificate information",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CertInfo"}}}
                    }
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Proxy
        // ═══════════════════════════════════════════════════════
        "/api/proxy/address": {
            "get": {
                "tags": ["Proxy"],
                "summary": "Proxy address (IP, QR, etc.)",
                "operationId": "getProxyAddress",
                "responses": {"200": {"description": "Proxy address info"}}
            }
        },
        "/api/proxy/system": {
            "get": {
                "tags": ["Proxy"],
                "summary": "System proxy status, including live OS state and persisted Bifrost preference",
                "operationId": "getSystemProxy",
                "responses": {"200": {"description": "System proxy status"}}
            },
            "put": {
                "tags": ["Proxy"],
                "summary": "Enable or disable system proxy",
                "operationId": "toggleSystemProxy",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "enabled": {"type": "boolean"},
                            "host": {"type": "string"},
                            "port": {"type": "integer"},
                            "bypass": {"type": "string"}
                        }
                    }}}
                },
                "responses": {"200": {"description": "Updated"}}
            }
        },
        "/api/proxy/system/launchd": {
            "get": {
                "tags": ["Proxy"],
                "summary": "macOS system proxy cleanup LaunchDaemon status",
                "operationId": "getSystemProxyLaunchd",
                "responses": {"200": {"description": "LaunchDaemon status"}}
            },
            "put": {
                "tags": ["Proxy"],
                "summary": "Install or uninstall macOS system proxy cleanup LaunchDaemon",
                "operationId": "toggleSystemProxyLaunchd",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "enabled": {"type": "boolean"}
                        },
                        "required": ["enabled"]
                    }}}
                },
                "responses": {
                    "200": {"description": "Updated"},
                    "403": {"description": "Administrator authorization cancelled or unavailable"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Metrics
        // ═══════════════════════════════════════════════════════
        "/api/metrics": {
            "get": {
                "tags": ["Metrics"],
                "summary": "Current metrics snapshot",
                "operationId": "getMetrics",
                "responses": {"200": {"description": "Metrics data"}}
            }
        },
        "/api/metrics/history": {
            "get": {
                "tags": ["Metrics"],
                "summary": "Metrics history",
                "operationId": "getMetricsHistory",
                "parameters": [
                    {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 50}}
                ],
                "responses": {"200": {"description": "Metrics history"}}
            }
        },
        "/api/metrics/apps": {
            "get": {
                "tags": ["Metrics"],
                "summary": "Metrics by app",
                "operationId": "getMetricsByApp",
                "parameters": [
                    {"name": "include_summary", "in": "query", "schema": {"type": "boolean", "default": false}, "description": "Return server-computed summary with items"}
                ],
                "responses": {"200": {"description": "Per-app metrics"}}
            }
        },
        "/api/metrics/hosts": {
            "get": {
                "tags": ["Metrics"],
                "summary": "Metrics by host",
                "operationId": "getMetricsByHost",
                "parameters": [
                    {"name": "include_summary", "in": "query", "schema": {"type": "boolean", "default": false}, "description": "Return server-computed summary with items"}
                ],
                "responses": {"200": {"description": "Per-host metrics"}}
            }
        },
        "/api/diagnostics/process-resolver": {
            "get": {
                "tags": ["Diagnostics"],
                "summary": "Process resolver diagnostics snapshot",
                "description": "Returns cumulative, on-demand process resolver counters. These diagnostics are not recorded in metrics history or pushed periodically.",
                "operationId": "getProcessResolverDiagnostics",
                "responses": {
                    "200": {"description": "Process resolver diagnostics"},
                    "503": {"description": "Process resolver diagnostics are not configured"}
                }
            }
        },
        "/api/diagnostics/workers": {
            "get": {
                "tags": ["Diagnostics"],
                "summary": "Auxiliary worker diagnostics snapshot",
                "description": "Returns lifecycle, heartbeat, concurrency and failure state for managed auxiliary workers without starting idle workers.",
                "operationId": "getAuxiliaryWorkerDiagnostics",
                "responses": {
                    "200": {"description": "Auxiliary worker diagnostics"}
                }
            }
        },
        "/api/workers": {
            "get": {
                "tags": ["Workers"],
                "summary": "List auxiliary workers",
                "operationId": "listAuxiliaryWorkers",
                "responses": {"200": {"description": "Worker snapshots"}}
            }
        },
        "/api/workers/{kind}": {
            "get": {
                "tags": ["Workers"],
                "summary": "List one auxiliary worker kind",
                "operationId": "getAuxiliaryWorkerKind",
                "parameters": [{"name": "kind", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "Worker snapshots"}, "404": {"description": "Unknown worker kind"}}
            }
        },
        "/api/workers/{kind}/{action}": {
            "post": {
                "tags": ["Workers"],
                "summary": "Control auxiliary worker lifecycle",
                "description": "Action is start, stop, restart, or reset-circuit. Start and restart only operate on worker instances already registered by their owning capability.",
                "operationId": "controlAuxiliaryWorkerKind",
                "parameters": [
                    {"name": "kind", "in": "path", "required": true, "schema": {"type": "string"}},
                    {"name": "action", "in": "path", "required": true, "schema": {"type": "string", "enum": ["start", "stop", "restart", "reset-circuit"]}}
                ],
                "responses": {"200": {"description": "Lifecycle action result"}, "404": {"description": "Unknown worker kind"}, "503": {"description": "All requested worker actions failed"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Ports
        // ═══════════════════════════════════════════════════════
        "/api/ports": {
            "get": {
                "tags": ["Ports"],
                "summary": "List temporary ports",
                "operationId": "listPorts",
                "responses": {"200": {"description": "Temporary port bindings"}}
            },
            "post": {
                "tags": ["Ports"],
                "summary": "Bind temporary port",
                "operationId": "bindPort",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {
                            "port": {"type": "integer"},
                            "rule": {"type": "string"},
                            "group_rule": {"type": "string"},
                            "rule_file": {"type": "string"}
                        }
                    }}}
                },
                "responses": {"200": {"description": "Port bound"}}
            }
        },
        "/api/ports/{port}": {
            "get": {
                "tags": ["Ports"],
                "summary": "Temporary port details",
                "operationId": "getPort",
                "parameters": [
                    {"name": "port", "in": "path", "required": true, "schema": {"type": "integer"}}
                ],
                "responses": {"200": {"description": "Port detail"}}
            },
            "delete": {
                "tags": ["Ports"],
                "summary": "Destroy temporary port",
                "operationId": "destroyPort",
                "parameters": [
                    {"name": "port", "in": "path", "required": true, "schema": {"type": "integer"}}
                ],
                "responses": {"200": {"description": "Port destroyed"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Replay
        // ═══════════════════════════════════════════════════════
        "/api/replay/execute": {
            "post": {
                "tags": ["Replay"],
                "summary": "Execute replay request",
                "operationId": "executeReplay",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ReplayRequest"}}}
                },
                "responses": {"200": {"description": "Replay result"}}
            }
        },
        "/api/replay/requests": {
            "get": {
                "tags": ["Replay"],
                "summary": "List replay requests",
                "operationId": "listReplayRequests",
                "responses": {"200": {"description": "Saved replay requests"}}
            },
            "post": {
                "tags": ["Replay"],
                "summary": "Save replay request",
                "operationId": "saveReplayRequest",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ReplayRequest"}}}
                },
                "responses": {"200": {"description": "Saved"}}
            }
        },
        "/api/replay/history": {
            "get": {
                "tags": ["Replay"],
                "summary": "Replay history",
                "operationId": "getReplayHistory",
                "responses": {"200": {"description": "Replay history"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Notifications
        // ═══════════════════════════════════════════════════════
        "/api/notifications": {
            "get": {
                "tags": ["Notifications"],
                "summary": "List notifications",
                "operationId": "listNotifications",
                "parameters": [
                    {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 50}},
                    {"name": "offset", "in": "query", "schema": {"type": "integer", "default": 0}}
                ],
                "responses": {"200": {"description": "Notifications list"}}
            }
        },
        "/api/notifications/unread-count": {
            "get": {
                "tags": ["Notifications"],
                "summary": "Unread notification count",
                "operationId": "getUnreadCount",
                "responses": {"200": {"description": "Unread count"}}
            }
        },
        "/api/notifications/mark-all-read": {
            "post": {
                "tags": ["Notifications"],
                "summary": "Mark all as read",
                "operationId": "markAllRead",
                "responses": {"200": {"description": "Marked"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Remote Invoke (basic)
        // ═══════════════════════════════════════════════════════
        "/api/remote-invoke/status": {
            "get": {
                "tags": ["Remote"],
                "summary": "Remote invoke status",
                "operationId": "getRemoteStatus",
                "responses": {"200": {"description": "Remote invoke status"}}
            }
        },
        "/api/remote-invoke/grants": {
            "get": {
                "tags": ["Remote"],
                "summary": "Grants list",
                "operationId": "listGrants",
                "responses": {"200": {"description": "Grants list"}}
            }
        },
        "/api/remote-invoke/grants/{id}": {
            "delete": {
                "tags": ["Remote"],
                "summary": "Revoke grant",
                "operationId": "revokeGrant",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Grant revoked"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Websocket
        // ═══════════════════════════════════════════════════════
        "/api/websocket/connections": {
            "get": {
                "tags": ["WebSocket"],
                "summary": "WebSocket connections list",
                "operationId": "listWsConnections",
                "responses": {"200": {"description": "WebSocket connections"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Syntax (reference)
        // ═══════════════════════════════════════════════════════
        "/api/syntax": {
            "get": {
                "tags": ["Reference"],
                "summary": "Syntax information (protocols, template variables, filters, etc.)",
                "operationId": "getSyntax",
                "responses": {"200": {"description": "Syntax reference"}}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema<'a>(spec: &'a OpenApiSpec, name: &str) -> &'a serde_json::Value {
        spec.components["schemas"]
            .get(name)
            .unwrap_or_else(|| panic!("missing schema {name}"))
    }

    #[test]
    fn breakpoint_openapi_documents_current_configuration_surface() {
        let spec = generate_openapi_spec();

        assert!(spec.paths.get("/api/breakpoint/settings").is_some());
        assert!(spec.paths.get("/api/breakpoint/pending").is_some());
        assert!(spec.paths.get("/api/breakpoint/resume").is_some());
        assert!(spec.paths.get("/api/rules").is_some());
        assert!(spec.paths.get("/api/config/tray").is_some());
        assert!(spec.paths.get("/api/config/performance").is_some());

        let settings = &schema(&spec, "BreakpointSettings")["properties"];
        assert!(settings.get("enabled").is_some());
        assert!(settings.get("max_body_bytes").is_some());
        assert!(settings.get("hook_request").is_none());
        assert!(settings.get("hook_response").is_none());
        assert!(settings.get("timeout_ms").is_none());

        let edit = &schema(&spec, "BreakpointEdit")["properties"];
        assert!(edit.get("method").is_some());
        assert!(edit.get("url").is_some());
        assert!(edit.get("status").is_some());
        assert_eq!(edit["headers"]["type"], "array");

        let pending = &schema(&spec, "PendingBreakpoint")["properties"];
        assert!(pending.get("deadline_at_ms").is_some());
        assert!(pending.get("server_now_ms").is_some());
        assert!(pending.get("body_omitted").is_some());

        let performance = &schema(&spec, "PerformanceBreakpointConfig")["properties"];
        assert!(performance.get("timeout_ms").is_some());
        assert!(performance.get("timeout_min_ms").is_some());
        assert!(performance.get("timeout_max_ms").is_some());

        let traffic = &schema(&spec, "PerformanceTrafficConfig")["properties"];
        assert!(traffic.get("super_performance_mode").is_some());

        let performance_update = &schema(&spec, "UpdatePerformanceConfigRequest")["properties"];
        assert!(performance_update.get("super_performance_mode").is_some());

        let tray = &schema(&spec, "TrayConfig")["properties"];
        assert!(tray.get("enabled").is_some());
        assert!(tray.get("supported").is_some());
        assert!(tray.get("system_stats_supported").is_some());
        assert!(tray.get("show_system_stats").is_some());
        assert!(tray.get("system_stats_items").is_some());

        let tray_items = &schema(&spec, "TraySystemStatsItems")["properties"];
        assert!(tray_items.get("cpu").is_some());
        assert!(tray_items.get("memory").is_some());
        assert!(tray_items.get("disk").is_some());
        assert!(tray_items.get("upload").is_some());
        assert!(tray_items.get("download").is_some());
    }

    #[test]
    fn performance_openapi_exposes_fixed_breakpoint_timeout_bounds() {
        let spec = generate_openapi_spec();
        let request =
            &schema(&spec, "UpdatePerformanceConfigRequest")["properties"]["breakpoint_timeout_ms"];

        assert_eq!(request["minimum"].as_u64(), Some(MIN_BREAKPOINT_TIMEOUT_MS));
        assert_eq!(request["maximum"].as_u64(), Some(MAX_BREAKPOINT_TIMEOUT_MS));

        let get_response_schema = &spec.paths["/api/config/performance"]["get"]["responses"]["200"]
            ["content"]["application/json"]["schema"];
        assert_eq!(
            get_response_schema["$ref"].as_str(),
            Some("#/components/schemas/PerformanceConfig")
        );

        let put_request_schema = &spec.paths["/api/config/performance"]["put"]["requestBody"]
            ["content"]["application/json"]["schema"];
        assert_eq!(
            put_request_schema["$ref"].as_str(),
            Some("#/components/schemas/UpdatePerformanceConfigRequest")
        );
    }
}
