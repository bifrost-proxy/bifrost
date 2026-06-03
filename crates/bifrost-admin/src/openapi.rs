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
            description: "Bifrost Proxy - Admin API cho Web UI, CLI và các công cụ quản lý proxy."
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
                "description": "JWT token từ /api/auth/login"
            }
        },
        "schemas": {
            "BreakpointSettings": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "description": "Bật/tắt breakpoint"},
                    "hook_request": {"type": "boolean", "description": "Chặn request trước khi gửi lên upstream"},
                    "hook_response": {"type": "boolean", "description": "Chặn response trước khi trả về client"}
                }
            },
            "BreakpointEdit": {
                "type": "object",
                "properties": {
                    "request_id": {"type": "string", "description": "ID của request đang bị breakpoint"},
                    "phase": {"type": "string", "enum": ["request", "response"], "description": "Pha breakpoint"},
                    "headers": {
                        "type": "object",
                        "description": "Headers đã chỉnh sửa (key-value)",
                        "additionalProperties": {"type": "string"}
                    },
                    "body": {
                        "type": "string",
                        "nullable": true,
                        "description": "Body đã chỉnh sửa. null = giữ nguyên"
                    }
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
                    "content": {"type": "string"},
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
                    "ip": {"type": "string", "description": "IP hoặc CIDR"},
                    "type": {"type": "string", "enum": ["permanent", "temporary"]}
                }
            },
            "ConfigTlsRequest": {
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean", "description": "TLS interception switch"},
                    "unsafe_ssl": {"type": "boolean", "description": "Bỏ qua xác thực upstream cert"},
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
                "summary": "Lấy cấu hình breakpoint",
                "operationId": "getBreakpointSettings",
                "responses": {
                    "200": {
                        "description": "Cấu hình breakpoint hiện tại",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointSettings"}}}
                    }
                }
            },
            "post": {
                "tags": ["Breakpoint"],
                "summary": "Cập nhật cấu hình breakpoint",
                "operationId": "updateBreakpointSettings",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointSettings"}}}
                },
                "responses": {
                    "200": {"description": "Đã cập nhật"},
                    "400": {"description": "Bad request"}
                }
            }
        },
        "/api/breakpoint/resume": {
            "post": {
                "tags": ["Breakpoint"],
                "summary": "Resume request/response đang bị breakpoint",
                "description": "Gửi headers + body đã chỉnh sửa để tiếp tục request/response đang bị tạm dừng",
                "operationId": "resumeBreakpoint",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/BreakpointEdit"}}}
                },
                "responses": {
                    "200": {"description": "Đã resume"},
                    "404": {"description": "Không tìm thấy request đang chờ"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Auth
        // ═══════════════════════════════════════════════════════
        "/api/auth/status": {
            "get": {
                "tags": ["Auth"],
                "summary": "Trạng thái xác thực",
                "operationId": "getAuthStatus",
                "responses": {
                    "200": {"description": "Trạng thái auth"}
                }
            }
        },
        "/api/auth/login": {
            "post": {
                "tags": ["Auth"],
                "summary": "Đăng nhập",
                "operationId": "login",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginRequest"}}}
                },
                "responses": {
                    "200": {
                        "description": "Đăng nhập thành công",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/LoginResponse"}}}
                    },
                    "401": {"description": "Sai username/password"}
                }
            }
        },
        "/api/auth/logout": {
            "post": {
                "tags": ["Auth"],
                "summary": "Đăng xuất",
                "operationId": "logout",
                "responses": {"200": {"description": "Đã đăng xuất"}}
            }
        },
        "/api/auth/passwd": {
            "post": {
                "tags": ["Auth"],
                "summary": "Đổi mật khẩu",
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
                    "200": {"description": "Đã đổi mật khẩu"},
                    "401": {"description": "Sai mật khẩu hiện tại"}
                }
            }
        },
        "/api/auth/remote": {
            "post": {
                "tags": ["Auth"],
                "summary": "Bật/tắt remote access",
                "operationId": "toggleRemoteAccess",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"enabled": {"type": "boolean"}}
                    }}}
                },
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/auth/revoke-all": {
            "post": {
                "tags": ["Auth"],
                "summary": "Thu hồi tất cả JWT session",
                "operationId": "revokeAllSessions",
                "responses": {"200": {"description": "Đã thu hồi"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Traffic
        // ═══════════════════════════════════════════════════════
        "/api/traffic": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Liệt kê traffic records",
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
                        "description": "Danh sách traffic",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/TrafficListResponse"}}}
                    }
                }
            },
            "delete": {
                "tags": ["Traffic"],
                "summary": "Xóa traffic records",
                "operationId": "deleteTraffic",
                "requestBody": {
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "properties": {"ids": {"type": "array", "items": {"type": "string"}}}
                    }}}
                },
                "responses": {"200": {"description": "Đã xóa"}}
            }
        },
        "/api/traffic/{id}": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Lấy chi tiết traffic record",
                "operationId": "getTraffic",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}, "description": "Traffic record ID, e.g. REQ-6a1f6c82-000318"}
                ],
                "responses": {
                    "200": {"description": "Chi tiết traffic record"},
                    "404": {"description": "Không tìm thấy"}
                }
            }
        },
        "/api/traffic/{id}/request-body": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Lấy request body",
                "operationId": "getTrafficRequestBody",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Request body"},
                    "404": {"description": "Không tìm thấy"}
                }
            }
        },
        "/api/traffic/{id}/response-body": {
            "get": {
                "tags": ["Traffic"],
                "summary": "Lấy response body",
                "operationId": "getTrafficResponseBody",
                "parameters": [
                    {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Response body"},
                    "404": {"description": "Không tìm thấy"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Search
        // ═══════════════════════════════════════════════════════
        "/api/search": {
            "post": {
                "tags": ["Search"],
                "summary": "Tìm kiếm traffic (full-text)",
                "operationId": "searchTraffic",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SearchQuery"}}}
                },
                "responses": {
                    "200": {"description": "Kết quả tìm kiếm"}
                }
            }
        },

        // ═══════════════════════════════════════════════════════
        // Rules
        // ═══════════════════════════════════════════════════════
        "/api/rules": {
            "get": {
                "tags": ["Rules"],
                "summary": "Liệt kê tất cả rules",
                "operationId": "listRules",
                "responses": {
                    "200": {
                        "description": "Danh sách rules",
                        "content": {"application/json": {"schema": {
                            "type": "array", "items": {"$ref": "#/components/schemas/RuleInfo"}
                        }}}
                    }
                }
            },
            "post": {
                "tags": ["Rules"],
                "summary": "Tạo rule mới",
                "operationId": "createRule",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["name", "content"],
                        "properties": {
                            "name": {"type": "string"},
                            "content": {"type": "string", "description": "Nội dung rule"}
                        }
                    }}}
                },
                "responses": {
                    "200": {"description": "Đã tạo"},
                    "400": {"description": "Rule đã tồn tại"}
                }
            }
        },
        "/api/rules/{name}": {
            "get": {
                "tags": ["Rules"],
                "summary": "Lấy chi tiết rule",
                "operationId": "getRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {
                    "200": {"description": "Chi tiết rule"},
                    "404": {"description": "Không tìm thấy"}
                }
            },
            "put": {
                "tags": ["Rules"],
                "summary": "Cập nhật rule",
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
                "responses": {"200": {"description": "Đã cập nhật"}}
            },
            "delete": {
                "tags": ["Rules"],
                "summary": "Xóa rule",
                "operationId": "deleteRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Đã xóa"}}
            }
        },
        "/api/rules/{name}/enable": {
            "put": {
                "tags": ["Rules"],
                "summary": "Bật rule",
                "operationId": "enableRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Đã bật"}}
            }
        },
        "/api/rules/{name}/disable": {
            "put": {
                "tags": ["Rules"],
                "summary": "Tắt rule",
                "operationId": "disableRule",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Đã tắt"}}
            }
        },
        "/api/rules/active-summary": {
            "get": {
                "tags": ["Rules"],
                "summary": "Tổng hợp rules đang active",
                "operationId": "getActiveRules",
                "responses": {"200": {"description": "Danh sách rule đang active"}}
            }
        },
        "/api/rules/reorder": {
            "put": {
                "tags": ["Rules"],
                "summary": "Sắp xếp lại thứ tự rules",
                "operationId": "reorderRules",
                "responses": {"200": {"description": "Đã sắp xếp"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Scripts
        // ═══════════════════════════════════════════════════════
        "/api/scripts": {
            "get": {
                "tags": ["Scripts"],
                "summary": "Liệt kê scripts",
                "operationId": "listScripts",
                "parameters": [
                    {"name": "type", "in": "query", "schema": {"type": "string", "enum": ["request", "response", "decode"]}}
                ],
                "responses": {
                    "200": {
                        "description": "Danh sách scripts",
                        "content": {"application/json": {"schema": {
                            "type": "array", "items": {"$ref": "#/components/schemas/ScriptInfo"}
                        }}}
                    }
                }
            },
            "put": {
                "tags": ["Scripts"],
                "summary": "Upload script mới",
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
                "responses": {"200": {"description": "Đã upload"}}
            }
        },
        "/api/scripts/test": {
            "post": {
                "tags": ["Scripts"],
                "summary": "Test script với mock data",
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
                "responses": {"200": {"description": "Kết quả test"}}
            }
        },
        "/api/scripts/{name}": {
            "get": {
                "tags": ["Scripts"],
                "summary": "Lấy script",
                "operationId": "getScript",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Nội dung script"}}
            },
            "delete": {
                "tags": ["Scripts"],
                "summary": "Xóa script",
                "operationId": "deleteScript",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Đã xóa"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Values
        // ═══════════════════════════════════════════════════════
        "/api/values": {
            "get": {
                "tags": ["Values"],
                "summary": "Liệt kê tất cả values",
                "operationId": "listValues",
                "responses": {
                    "200": {"description": "Danh sách values (key-value)"}
                }
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
                "responses": {"200": {"description": "Đã lưu"}}
            },
            "delete": {
                "tags": ["Values"],
                "summary": "Xóa value",
                "operationId": "deleteValue",
                "parameters": [
                    {"name": "name", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"200": {"description": "Đã xóa"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Whitelist
        // ═══════════════════════════════════════════════════════
        "/api/whitelist": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Liệt kê whitelist",
                "operationId": "listWhitelist",
                "responses": {
                    "200": {"description": "Danh sách whitelist"}
                }
            },
            "post": {
                "tags": ["Whitelist"],
                "summary": "Thêm IP/CIDR vào whitelist",
                "operationId": "addWhitelist",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string", "description": "IP hoặc CIDR, e.g. 192.168.1.0/24"}}
                    }}}
                },
                "responses": {"200": {"description": "Đã thêm"}}
            }
        },
        "/api/whitelist/mode": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Lấy access mode",
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
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/whitelist/allow-lan": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Trạng thái allow-lan",
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
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/whitelist/pending": {
            "get": {
                "tags": ["Whitelist"],
                "summary": "Danh sách pending requests",
                "operationId": "getPendingWhitelist",
                "responses": {"200": {"description": "Pending authorization requests"}}
            }
        },
        "/api/whitelist/pending/approve": {
            "post": {
                "tags": ["Whitelist"],
                "summary": "Chấp nhận pending request",
                "operationId": "approvePending",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Đã chấp nhận"}}
            }
        },
        "/api/whitelist/pending/reject": {
            "post": {
                "tags": ["Whitelist"],
                "summary": "Từ chối pending request",
                "operationId": "rejectPending",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["ip"],
                        "properties": {"ip": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Đã từ chối"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Config
        // ═══════════════════════════════════════════════════════
        "/api/config": {
            "get": {
                "tags": ["Config"],
                "summary": "Lấy toàn bộ cấu hình proxy",
                "operationId": "getConfig",
                "responses": {"200": {"description": "Cấu hình hiện tại"}}
            }
        },
        "/api/config/tls": {
            "get": {
                "tags": ["Config"],
                "summary": "Lấy cấu hình TLS interception",
                "operationId": "getTlsConfig",
                "responses": {"200": {"description": "Cấu hình TLS"}}
            },
            "put": {
                "tags": ["Config"],
                "summary": "Cập nhật cấu hình TLS",
                "operationId": "updateTlsConfig",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ConfigTlsRequest"}}}
                },
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/config/server": {
            "get": {
                "tags": ["Config"],
                "summary": "Lấy cấu hình server",
                "operationId": "getServerConfig",
                "responses": {"200": {"description": "Cấu hình server"}}
            },
            "put": {
                "tags": ["Config"],
                "summary": "Cập nhật cấu hình server",
                "operationId": "updateServerConfig",
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/config/performance": {
            "get": {
                "tags": ["Config"],
                "summary": "Lấy cấu hình hiệu năng",
                "operationId": "getPerformanceConfig",
                "responses": {"200": {"description": "Cấu hình hiệu năng"}}
            },
            "put": {
                "tags": ["Config"],
                "summary": "Cập nhật cấu hình hiệu năng",
                "operationId": "updatePerformanceConfig",
                "responses": {"200": {"description": "Đã cập nhật"}}
            }
        },
        "/api/config/connections": {
            "get": {
                "tags": ["Config"],
                "summary": "Danh sách connections đang active",
                "operationId": "listConnections",
                "responses": {"200": {"description": "Active connections"}}
            }
        },
        "/api/config/connections/disconnect": {
            "post": {
                "tags": ["Config"],
                "summary": "Ngắt kết nối theo domain",
                "operationId": "disconnectByDomain",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "object",
                        "required": ["domain"],
                        "properties": {"domain": {"type": "string"}}
                    }}}
                },
                "responses": {"200": {"description": "Đã ngắt kết nối"}}
            }
        },
        "/api/config/performance/clear-cache": {
            "delete": {
                "tags": ["Config"],
                "summary": "Xóa body cache",
                "operationId": "clearBodyCache",
                "responses": {"200": {"description": "Đã xóa cache"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // System
        // ═══════════════════════════════════════════════════════
        "/api/system": {
            "get": {
                "tags": ["System"],
                "summary": "Thông tin hệ thống cơ bản",
                "operationId": "getSystemInfo",
                "responses": {
                    "200": {
                        "description": "Thông tin hệ thống",
                        "content": {"application/json": {"schema": {"$ref": "#/components/schemas/SystemOverview"}}}
                    }
                }
            }
        },
        "/api/system/overview": {
            "get": {
                "tags": ["System"],
                "summary": "Tổng quan (rules/traffic/pending count)",
                "operationId": "getSystemOverview",
                "responses": {"200": {"description": "Tổng quan hệ thống"}}
            }
        },
        "/api/system/memory": {
            "get": {
                "tags": ["System"],
                "summary": "Chẩn đoán bộ nhớ",
                "operationId": "getMemoryDiagnostics",
                "responses": {"200": {"description": "Memory diagnostics"}}
            }
        },
        "/api/system/version-check": {
            "get": {
                "tags": ["System"],
                "summary": "Kiểm tra phiên bản mới",
                "operationId": "checkVersion",
                "parameters": [
                    {"name": "refresh", "in": "query", "schema": {"type": "boolean"}}
                ],
                "responses": {"200": {"description": "Kết quả kiểm tra phiên bản"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Cert
        // ═══════════════════════════════════════════════════════
        "/api/cert": {
            "get": {
                "tags": ["Cert"],
                "summary": "Thông tin CA certificate",
                "operationId": "getCertInfo",
                "responses": {
                    "200": {
                        "description": "Thông tin cert",
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
                "summary": "Địa chỉ proxy (IP, QR...)",
                "operationId": "getProxyAddress",
                "responses": {"200": {"description": "Proxy address info"}}
            }
        },
        "/api/proxy/system": {
            "get": {
                "tags": ["Proxy"],
                "summary": "Trạng thái system proxy",
                "operationId": "getSystemProxy",
                "responses": {"200": {"description": "System proxy status"}}
            },
            "put": {
                "tags": ["Proxy"],
                "summary": "Bật/tắt system proxy",
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
                "responses": {"200": {"description": "Đã cập nhật"}}
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
                "summary": "Metrics theo ứng dụng",
                "operationId": "getMetricsByApp",
                "responses": {"200": {"description": "Per-app metrics"}}
            }
        },
        "/api/metrics/hosts": {
            "get": {
                "tags": ["Metrics"],
                "summary": "Metrics theo host",
                "operationId": "getMetricsByHost",
                "responses": {"200": {"description": "Per-host metrics"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Ports
        // ═══════════════════════════════════════════════════════
        "/api/ports": {
            "get": {
                "tags": ["Ports"],
                "summary": "Liệt kê temporary ports",
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
                "summary": "Chi tiết temporary port",
                "operationId": "getPort",
                "parameters": [
                    {"name": "port", "in": "path", "required": true, "schema": {"type": "integer"}}
                ],
                "responses": {"200": {"description": "Port detail"}}
            },
            "delete": {
                "tags": ["Ports"],
                "summary": "Hủy temporary port",
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
                "summary": "Thực thi replay request",
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
                "summary": "Liệt kê replay requests",
                "operationId": "listReplayRequests",
                "responses": {"200": {"description": "Saved replay requests"}}
            },
            "post": {
                "tags": ["Replay"],
                "summary": "Lưu replay request",
                "operationId": "saveReplayRequest",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ReplayRequest"}}}
                },
                "responses": {"200": {"description": "Đã lưu"}}
            }
        },
        "/api/replay/history": {
            "get": {
                "tags": ["Replay"],
                "summary": "Lịch sử replay",
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
                "summary": "Liệt kê notifications",
                "operationId": "listNotifications",
                "parameters": [
                    {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 50}},
                    {"name": "offset", "in": "query", "schema": {"type": "integer", "default": 0}}
                ],
                "responses": {"200": {"description": "Danh sách notifications"}}
            }
        },
        "/api/notifications/unread-count": {
            "get": {
                "tags": ["Notifications"],
                "summary": "Số notifications chưa đọc",
                "operationId": "getUnreadCount",
                "responses": {"200": {"description": "Unread count"}}
            }
        },
        "/api/notifications/mark-all-read": {
            "post": {
                "tags": ["Notifications"],
                "summary": "Đánh dấu tất cả đã đọc",
                "operationId": "markAllRead",
                "responses": {"200": {"description": "Đã đánh dấu"}}
            }
        },

        // ═══════════════════════════════════════════════════════
        // Remote Invoke (cơ bản)
        // ═══════════════════════════════════════════════════════
        "/api/remote-invoke/status": {
            "get": {
                "tags": ["Remote"],
                "summary": "Trạng thái remote invoke",
                "operationId": "getRemoteStatus",
                "responses": {"200": {"description": "Remote invoke status"}}
            }
        },
        "/api/remote-invoke/grants": {
            "get": {
                "tags": ["Remote"],
                "summary": "Danh sách grants",
                "operationId": "listGrants",
                "responses": {"200": {"description": "Grants list"}}
            }
        },
        "/api/remote-invoke/grants/{id}": {
            "delete": {
                "tags": ["Remote"],
                "summary": "Thu hồi grant",
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
                "summary": "Danh sách WebSocket connections",
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
                "summary": "Thông tin syntax (protocols, template vars, filters...)",
                "operationId": "getSyntax",
                "responses": {"200": {"description": "Syntax reference"}}
            }
        }
    })
}
