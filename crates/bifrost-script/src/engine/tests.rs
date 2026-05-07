use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_engine_init() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });

    assert!(engine.init().await.is_ok());
    assert!(temp_dir.path().join("request").exists());
    assert!(temp_dir.path().join("response").exists());
}

#[tokio::test]
async fn test_save_and_load_script() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let script_content = r#"log.info("Hello from test script");"#;
    engine
        .save_script(ScriptType::Request, "test-script", script_content)
        .await
        .unwrap();

    let loaded = engine
        .load_script(ScriptType::Request, "test-script")
        .await
        .unwrap();
    assert_eq!(loaded, script_content);
}

#[tokio::test]
async fn test_list_scripts() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Request, "script-a", "// A")
        .await
        .unwrap();
    engine
        .save_script(ScriptType::Request, "script-b", "// B")
        .await
        .unwrap();

    let scripts = engine.list_scripts(ScriptType::Request).await.unwrap();
    assert_eq!(scripts.len(), 2);
}

#[tokio::test]
async fn test_decode_test_returns_output() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "text/plain".to_string());
    let request = RequestData {
        url: "https://example.com/".to_string(),
        method: "GET".to_string(),
        host: "example.com".to_string(),
        path: "/".to_string(),
        protocol: "https".to_string(),
        client_ip: "127.0.0.1".to_string(),
        client_app: None,
        headers,
        body: Some("hello".to_string()),
    };
    let ctx = ScriptContext {
        request_id: "test".to_string(),
        script_name: "test".to_string(),
        script_type: ScriptType::Decode,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let script = r#"
log.info("decode phase:", ctx.phase);
ctx.output = { code: "0", data: request.body, msg: "" };
"#;

    let result = engine
        .test_script(ScriptType::Decode, script, Some(&request), None, &ctx)
        .await;

    assert!(result.success);
    assert!(result.decode_output.is_some());
    let out = result.decode_output.unwrap();
    assert_eq!(out.code, "0");
    assert_eq!(out.data, "hello");
    assert_eq!(out.msg, "");
    assert!(!result.logs.is_empty());
}

#[tokio::test]
async fn test_parser_script_reads_full_body_base64() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let script = r#"
ctx.output = {
  code: "0",
  data: JSON.stringify({ phase: ctx.phase, bodyBase64: request.bodyBase64 }),
  msg: ""
};
"#;
    engine
        .save_script(ScriptType::Parser, "bp/local", script)
        .await
        .unwrap();

    let request = RequestData {
        url: "https://example.com/api".to_string(),
        method: "POST".to_string(),
        host: "example.com".to_string(),
        path: "/api".to_string(),
        protocol: "https".to_string(),
        client_ip: "127.0.0.1".to_string(),
        client_app: None,
        headers: HashMap::new(),
        body: None,
    };
    let ctx = ScriptContext {
        request_id: "bp-local".to_string(),
        script_name: "bp/local".to_string(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let (out, _) = engine
        .execute_parser_script(
            "bp/local",
            "request",
            &request,
            b"\x00hello\xff",
            &ResponseData::default(),
            &[],
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(out.code, "0");
    assert!(out.data.contains("AGhlbGxv/w=="));
}

#[tokio::test]
async fn test_local_parser_ref_can_carry_script_options() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let script = r#"
ctx.output = {
  code: "0",
  data: ctx.scriptName,
  msg: ""
};
"#;
    engine
        .save_script(ScriptType::Parser, "build_in_bp", script)
        .await
        .unwrap();

    let script_ref = "build_in_bp?psm=foo.bar.order&idlSource=bam";
    let ctx = ScriptContext {
        request_id: "bp-options".to_string(),
        script_name: script_ref.to_string(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let (out, _) = engine
        .execute_parser_script(
            script_ref,
            "response",
            &RequestData::default(),
            &[],
            &ResponseData::default(),
            b"body",
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(out.code, "0");
    assert_eq!(out.data, script_ref);
}

#[tokio::test]
async fn test_local_parser_ref_rejects_path_traversal_name() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let ctx = ScriptContext {
        request_id: "bp-invalid-name".to_string(),
        script_name: "../evil?psm=foo".to_string(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let err = engine
        .execute_parser_script(
            "../evil?psm=foo",
            "response",
            &RequestData::default(),
            &[],
            &ResponseData::default(),
            b"body",
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("cannot contain '..'"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_init_releases_build_in_bp_parser_script() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        timeout_ms: 1000,
        max_memory: 1024 * 1024,
    });

    engine.init().await.unwrap();

    let released = std::fs::read_to_string(temp_dir.path().join("parser/build_in_bp.js")).unwrap();
    assert_eq!(released, crate::builtins::build_in_bp_script_content());

    let loaded = engine
        .load_script(ScriptType::Parser, "build_in_bp")
        .await
        .unwrap();
    assert_eq!(loaded, crate::builtins::build_in_bp_script_content());
}

#[tokio::test]
async fn test_init_overwrites_stale_build_in_bp_parser_script_and_cache() {
    let temp_dir = TempDir::new().unwrap();
    let parser_dir = temp_dir.path().join("parser");
    std::fs::create_dir_all(&parser_dir).unwrap();
    std::fs::write(
        parser_dir.join("build_in_bp.js"),
        "ctx.output = { code: 'stale' };",
    )
    .unwrap();

    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        timeout_ms: 1000,
        max_memory: 1024 * 1024,
    });

    let stale = engine
        .load_script(ScriptType::Parser, "build_in_bp")
        .await
        .unwrap();
    assert!(stale.contains("stale"));

    engine.init().await.unwrap();

    let loaded = engine
        .load_script(ScriptType::Parser, "build_in_bp")
        .await
        .unwrap();
    assert_eq!(loaded, crate::builtins::build_in_bp_script_content());
    assert_eq!(
        std::fs::read_to_string(parser_dir.join("build_in_bp.js")).unwrap(),
        crate::builtins::build_in_bp_script_content()
    );
}

#[tokio::test]
async fn test_parser_script_net_fetch_posts_body_base64() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};

            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&buf[..header_end + 4]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Content-Length:")
                                .or_else(|| line.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    let body_start = header_end + 4;
                    if buf.len() >= body_start + content_length {
                        break;
                    }
                }
            }

            let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let body = &buf[header_end + 4..];
            let body_hex = body.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let payload = serde_json::json!({ "hex": body_hex }).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                payload.len(),
                payload
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let script = r#"
function queryParam(name) {
  var query = ctx.scriptName.split("?")[1] || "";
  var parts = query.split("&");
  for (var i = 0; i < parts.length; i++) {
    var kv = parts[i].split("=");
    if (decodeURIComponent(kv[0] || "") === name) {
      return decodeURIComponent(kv.slice(1).join("="));
    }
  }
  return "";
}
var resp = JSON.parse(net.fetch(queryParam("url"), JSON.stringify({
  method: "POST",
  bodyBase64: response.bodyBase64
})));
ctx.output = { code: "0", data: JSON.parse(resp.body).hex, msg: "" };
"#;
    engine
        .save_script(ScriptType::Parser, "body_base64_post", script)
        .await
        .unwrap();

    let script_ref = format!(
        "body_base64_post?url={}",
        format!("http://{addr}/parse")
            .replace(':', "%3A")
            .replace('/', "%2F")
    );
    let request = RequestData {
        path: "/orders/GetOrder".to_string(),
        ..Default::default()
    };
    let response = ResponseData {
        request: request.clone(),
        ..Default::default()
    };
    let ctx = ScriptContext {
        request_id: "bp-bam".to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let (out, _) = engine
        .execute_parser_script(
            &script_ref,
            "response",
            &request,
            &[],
            &response,
            &[0x00, 0xff, 0x41],
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(out.code, "0");
    assert_eq!(out.data, "00ff41");
}

#[tokio::test]
async fn test_build_in_bp_decodes_real_next_agent_thrift_rpc_bytes() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};

        for _ in 0..6 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buf);
            let body = if request.contains("/api/endpoint/list") {
                serde_json::json!({
                    "status_code": 0,
                    "data": [{
                        "endpoint_id": 3964350,
                        "rpc_method": "Healthz",
                        "path": "/api/v1/healthz"
                    }]
                })
            } else if request.contains("/api/endpoint/info") {
                serde_json::json!({
                    "status_code": 0,
                    "data": {
                        "endpoint_id": 3964350,
                        "rpc_method": "Healthz",
                        "req_schema": {
                            "req_type": "idl/flow.devops.next_agent.HealthzRequest"
                        },
                        "resp_schema": {
                            "200": {
                                "resp_type": "idl/flow.devops.next_agent.HealthResponse"
                            }
                        }
                    }
                })
            } else {
                serde_json::json!({
                    "status_code": 0,
                    "data": {
                        "structs": {
                            "idl/flow.devops.next_agent.HealthzRequest": {
                                "type": "object",
                                "children": {
                                    "Base": {
                                        "type": "object",
                                        "field_id": 255,
                                        "raw_name": "Base",
                                        "ref_name": "idl/base.Base"
                                    }
                                }
                            },
                            "idl/flow.devops.next_agent.HealthResponse": {
                                "type": "object",
                                "children": {
                                    "status": {
                                        "type": "string",
                                        "field_id": 1,
                                        "raw_name": "status",
                                        "raw_type": "string"
                                    },
                                    "BaseResp": {
                                        "type": "object",
                                        "field_id": 255,
                                        "raw_name": "BaseResp",
                                        "ref_name": "idl/base.BaseResp"
                                    }
                                }
                            },
                            "idl/base.Base": {
                                "type": "object",
                                "children": {}
                            },
                            "idl/base.BaseResp": {
                                "type": "object",
                                "children": {}
                            }
                        }
                    }
                })
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scripts/parser/build_in_bp.js");
    let script = std::fs::read_to_string(script_path).unwrap();
    engine
        .save_script(ScriptType::Parser, "build_in_bp", &script)
        .await
        .unwrap();

    let base_ref = format!(
            "build_in_bp?protocol=thrift&psm=flow.devops.next_agent&version=1.0.77&method=Healthz&bamToken=ak%3Dmock&bamBaseUrl={}",
            format!("http://{addr}").replace(':', "%3A").replace('/', "%2F")
        );
    let request = RequestData {
        path: "/api/v1/healthz".to_string(),
        ..Default::default()
    };
    let response = ResponseData {
        request: request.clone(),
        ..Default::default()
    };

    let ctx = ScriptContext {
        request_id: "bp-thrift-real-req".to_string(),
        script_name: base_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };
    let (req_out, _) = engine
        .execute_parser_script(
            &base_ref,
            "request",
            &request,
            &[
                0x80, 0x01, 0x00, 0x01, 0, 0, 0, 7, b'H', b'e', b'a', b'l', b't', b'h', b'z', 0, 0,
                0, 7, 0x0c, 0, 1, 0,
            ],
            &response,
            &[],
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(req_out.code, "0", "{}", req_out.msg);
    assert!(req_out.data.contains("\"method\":\"Healthz\""));
    assert!(req_out.data.contains("\"schema_type\":\"request\""));

    let response_ref = format!("{base_ref}&schemaType=response");
    let ctx = ScriptContext {
        request_id: "bp-thrift-real-res".to_string(),
        script_name: response_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };
    let (res_out, _) = engine
        .execute_parser_script(
            &response_ref,
            "response",
            &request,
            &[],
            &response,
            &[
                0x80, 0x01, 0x00, 0x02, 0, 0, 0, 7, b'H', b'e', b'a', b'l', b't', b'h', b'z', 0, 0,
                0, 7, 0x0c, 0, 0, 0x0b, 0, 1, 0, 0, 0, 2, b'o', b'k', 0x0c, 0, 0xff, 0, 0, 0,
            ],
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(res_out.code, "0", "{}", res_out.msg);
    assert!(res_out.data.contains("\"method\":\"Healthz\""));
    assert!(res_out.data.contains("\"schema_type\":\"response\""));
    assert!(res_out.data.contains("\"status\":\"ok\""));
}

#[tokio::test]
async fn test_build_in_bp_decodes_next_agent_http_rpc_response() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};

        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = stream.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&buf);
            let body = if request.contains("/api/endpoint/list") {
                serde_json::json!({
                    "status_code": 0,
                    "data": [{
                        "endpoint_id": 3964350,
                        "rpc_method": "Healthz",
                        "name": "Healthz",
                        "method": "GET",
                        "path": "/api/v1/healthz",
                        "serializer": "json",
                        "resp_serializer": "json"
                    }]
                })
            } else {
                serde_json::json!({
                    "status_code": 0,
                    "data": {
                        "endpoint_id": 3964350,
                        "rpc_method": "Healthz",
                        "name": "Healthz",
                        "method": "GET",
                        "path": "/api/v1/healthz",
                        "serializer": "json",
                        "resp_serializer": "json"
                    }
                })
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scripts/parser/build_in_bp.js");
    let script = std::fs::read_to_string(script_path).unwrap();
    engine
        .save_script(ScriptType::Parser, "build_in_bp", &script)
        .await
        .unwrap();

    let script_ref = format!(
            "build_in_bp?protocol=http-rpc&psm=flow.devops.next_agent&version=1.0.77&method=Healthz&bamToken=ak%3Dmock&bamBaseUrl={}",
            format!("http://{addr}").replace(':', "%3A").replace('/', "%2F")
        );
    let request = RequestData {
        url: "https://nextoncall.bytedance.net/api/nextagent/v1/healthz".to_string(),
        method: "GET".to_string(),
        host: "nextoncall.bytedance.net".to_string(),
        path: "/api/nextagent/v1/healthz".to_string(),
        protocol: "https".to_string(),
        ..Default::default()
    };
    let response = ResponseData {
        status: 200,
        request: request.clone(),
        ..Default::default()
    };
    let ctx = ScriptContext {
        request_id: "bp-http-rpc-next-agent".to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let (out, _) = engine
        .execute_parser_script(
            &script_ref,
            "response",
            &request,
            &[],
            &response,
            br#"{"status":"ok","BaseResp":{}}"#,
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(out.code, "0", "{}", out.msg);
    assert!(out.data.contains("\"protocol\":\"http-rpc\""));
    assert!(out.data.contains("\"method\":\"Healthz\""));
    assert!(out.data.contains("\"path\":\"/api/nextagent/v1/healthz\""));
    assert!(out.data.contains("\"status\":\"ok\""));
}

#[tokio::test]
async fn test_remote_parser_requires_sha256() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let ctx = ScriptContext {
        request_id: "bp-remote".to_string(),
        script_name: "remote".to_string(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };
    let err = engine
        .execute_parser_script(
            "https://127.0.0.1/parser.js",
            "request",
            &RequestData::default(),
            b"body",
            &ResponseData::default(),
            &[],
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("requires sha256=<hex>"));
}

#[tokio::test]
async fn test_remote_parser_downloads_verifies_and_caches() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let script = r#"ctx.output = { code: "0", data: "remote:" + response.bodyBase64, msg: "" };"#;
    let sha = ScriptEngine::sha256_hex(script.as_bytes());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let script_for_thread = script.to_string();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\n\r\n{}",
                    script_for_thread.len(),
                    script_for_thread
                );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let script_ref = format!("http://{addr}/parser.js?sha256={sha}");
    let ctx = ScriptContext {
        request_id: "bp-remote".to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let (out, _) = engine
        .execute_parser_script(
            &script_ref,
            "response",
            &RequestData::default(),
            &[],
            &ResponseData::default(),
            b"hello",
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(out.code, "0");
    assert_eq!(out.data, "remote:aGVsbG8=");
    let cache_root = temp_dir.path().join("_remote-cache").join("parser");
    let cached_files = std::fs::read_dir(cache_root)
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("js"))
        .count();
    assert_eq!(cached_files, 1);
}

#[tokio::test]
async fn test_remote_parser_download_timeout_uses_network_timeout() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let script = r#"ctx.output = { code: "0", data: "remote:" + response.bodyBase64, msg: "" };"#;
    let sha = ScriptEngine::sha256_hex(script.as_bytes());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            use std::time::Duration;

            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            std::thread::sleep(Duration::from_millis(300));
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = stream.write_all(response);
        }
    });

    let script_ref = format!("http://{addr}/parser.js?sha256={sha}");
    let ctx = ScriptContext {
        request_id: "bp-remote-timeout".to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let start = Instant::now();
    let err = engine
        .execute_parser_script_with_sandbox(
            &script_ref,
            "response",
            &RequestData::default(),
            &[],
            &ResponseData::default(),
            b"hello",
            &ctx,
            crate::sandbox::SandboxConfig {
                allow_network: true,
                network_timeout_ms: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("remote parser download failed"),
        "unexpected error: {err}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "timeout should fail quickly, elapsed: {:?}",
        start.elapsed()
    );

    let cache_root = temp_dir.path().join("_remote-cache").join("parser");
    let cached_files = std::fs::read_dir(cache_root)
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("js"))
        .count();
    assert_eq!(
        cached_files, 0,
        "timeout should not populate remote parser cache"
    );
}

#[tokio::test]
async fn test_remote_parser_download_rejects_body_over_script_limit() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    });
    engine.init().await.unwrap();

    let body = vec![b'a'; MAX_SCRIPT_FILE_BYTES as usize + 1];
    let sha = ScriptEngine::sha256_hex(&body);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};

            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let headers = b"HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(headers);
            let _ = stream.write_all(&body);
        }
    });

    let script_ref = format!("http://{addr}/parser.js?sha256={sha}");
    let ctx = ScriptContext {
        request_id: "bp-remote-too-large".to_string(),
        script_name: script_ref.clone(),
        script_type: ScriptType::Parser,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let err = engine
        .execute_parser_script(
            &script_ref,
            "response",
            &RequestData::default(),
            &[],
            &ResponseData::default(),
            b"hello",
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("remote parser script too large"),
        "unexpected error: {err}"
    );

    let cache_root = temp_dir.path().join("_remote-cache").join("parser");
    let cached_files = std::fs::read_dir(cache_root)
        .unwrap()
        .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("js"))
        .count();
    assert_eq!(
        cached_files, 0,
        "oversized remote parser should not populate remote parser cache"
    );
}

#[test]
fn test_validate_script_name() {
    assert!(ScriptEngine::validate_script_name("valid-name").is_ok());
    assert!(ScriptEngine::validate_script_name("valid_name").is_ok());
    assert!(ScriptEngine::validate_script_name("validName123").is_ok());
    assert!(ScriptEngine::validate_script_name("api/auth/add-token").is_ok());
    assert!(ScriptEngine::validate_script_name("folder/script").is_ok());
    assert!(ScriptEngine::validate_script_name("").is_err());
    assert!(ScriptEngine::validate_script_name("invalid name").is_err());
    assert!(ScriptEngine::validate_script_name("invalid.name").is_err());
    assert!(ScriptEngine::validate_script_name("/leading-slash").is_err());
    assert!(ScriptEngine::validate_script_name("trailing-slash/").is_err());
    assert!(ScriptEngine::validate_script_name("double//slash").is_err());
    assert!(ScriptEngine::validate_script_name("../path-traversal").is_err());
}

#[tokio::test]
async fn test_script_timeout_in_engine() {
    let temp_dir = TempDir::new().unwrap();
    let engine = ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: temp_dir.path().to_path_buf(),
        timeout_ms: 100,
        max_memory: 16 * 1024 * 1024,
    });
    engine.init().await.unwrap();

    let infinite_loop_script = r#"while(true) {}"#;
    engine
        .save_script(ScriptType::Request, "infinite-loop", infinite_loop_script)
        .await
        .unwrap();

    let mut request = RequestData::default();
    let ctx = ScriptContext {
        request_id: "test-timeout".to_string(),
        script_name: "infinite-loop".to_string(),
        script_type: ScriptType::Request,
        values: HashMap::new(),
        matched_rules: vec![],
    };

    let start = std::time::Instant::now();
    let result = engine
        .execute_request_script("infinite-loop", &mut request, &ctx)
        .await;
    let elapsed = start.elapsed();

    assert!(!result.success);
    assert!(result.error.is_some());
    assert!(
        result.error.as_ref().unwrap().contains("timeout"),
        "Error should mention timeout: {:?}",
        result.error
    );
    assert!(
        elapsed.as_millis() < 500,
        "Should timeout within 500ms, took {:?}",
        elapsed
    );
}
