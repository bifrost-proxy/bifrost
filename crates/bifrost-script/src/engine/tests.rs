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

// ---------------------------------------------------------------------------
// Additional coverage tests for the engine lifecycle, execute_* variants,
// test_script branches, cache invalidation, and the remote-ref URL helpers.
// ---------------------------------------------------------------------------

fn mk_ctx(name: &str, ty: ScriptType) -> ScriptContext {
    ScriptContext {
        request_id: "req".to_string(),
        script_name: name.to_string(),
        script_type: ty,
        values: HashMap::new(),
        matched_rules: vec![],
    }
}

fn mk_engine(dir: &std::path::Path) -> ScriptEngine {
    ScriptEngine::new(ScriptEngineConfig {
        scripts_dir: dir.to_path_buf(),
        timeout_ms: 2000,
        max_memory: 16 * 1024 * 1024,
    })
}

#[tokio::test]
async fn test_scripts_dir_accessor() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    assert_eq!(engine.scripts_dir(), &temp_dir.path().to_path_buf());
}

#[tokio::test]
async fn test_delete_script_then_missing() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Request, "to-delete", "// x")
        .await
        .unwrap();
    engine
        .delete_script(ScriptType::Request, "to-delete")
        .await
        .unwrap();

    // Deleting again must error NotFound.
    let err = engine
        .delete_script(ScriptType::Request, "to-delete")
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::NotFound(_)));
}

#[tokio::test]
async fn test_rename_script_moves_content_and_cleans_dirs() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Response, "nested/old", "// payload")
        .await
        .unwrap();
    // Prime the cache so the rename takes the cache-move branch.
    let _ = engine.load_script(ScriptType::Response, "nested/old").await;

    engine
        .rename_script(ScriptType::Response, "nested/old", "fresh")
        .await
        .unwrap();

    let moved = engine
        .load_script(ScriptType::Response, "fresh")
        .await
        .unwrap();
    assert_eq!(moved, "// payload");
    // The now-empty nested dir should have been pruned.
    assert!(!temp_dir.path().join("response/nested").exists());
}

#[tokio::test]
async fn test_rename_script_errors() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    // Source missing -> NotFound.
    let err = engine
        .rename_script(ScriptType::Request, "ghost", "x")
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::NotFound(_)));

    // Destination already exists -> InvalidName.
    engine
        .save_script(ScriptType::Request, "a", "// a")
        .await
        .unwrap();
    engine
        .save_script(ScriptType::Request, "b", "// b")
        .await
        .unwrap();
    let err = engine
        .rename_script(ScriptType::Request, "a", "b")
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::InvalidName(_)));
}

#[tokio::test]
async fn test_load_script_not_found_and_invalid_name() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let err = engine
        .load_script(ScriptType::Request, "missing")
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::NotFound(_)));

    for bad in ["", "/lead", "trail/", "a..b", "a//b", "weird*name"] {
        let err = engine
            .load_script(ScriptType::Request, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, ScriptError::InvalidName(_)), "name={bad}");
    }

    // Over-long name (>128 chars).
    let long = "a".repeat(129);
    let err = engine
        .load_script(ScriptType::Request, &long)
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::InvalidName(_)));
}

#[tokio::test]
async fn test_list_scripts_empty_dir_and_nested() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());

    // Before init the decode dir doesn't exist -> empty vec.
    let empty = engine.list_scripts(ScriptType::Decode).await.unwrap();
    assert!(empty.is_empty());

    engine.init().await.unwrap();
    engine
        .save_script(ScriptType::Decode, "top", "// 1")
        .await
        .unwrap();
    engine
        .save_script(ScriptType::Decode, "deep/inner", "// 2")
        .await
        .unwrap();
    let scripts = engine.list_scripts(ScriptType::Decode).await.unwrap();
    let names: Vec<_> = scripts.iter().map(|s| s.name.clone()).collect();
    assert!(names.contains(&"top".to_string()));
    assert!(names.contains(&"deep/inner".to_string()));
}

#[tokio::test]
async fn test_execute_request_script_success_modifies_request() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"request.method = "PUT"; request.body = "modified";"#;
    engine
        .save_script(ScriptType::Request, "mod", script)
        .await
        .unwrap();

    let mut request = RequestData {
        method: "GET".to_string(),
        ..Default::default()
    };
    let ctx = mk_ctx("mod", ScriptType::Request);
    let result = engine
        .execute_request_script("mod", &mut request, &ctx)
        .await;
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(request.method, "PUT");
    assert_eq!(request.body.as_deref(), Some("modified"));
}

#[tokio::test]
async fn test_execute_request_script_missing_returns_failure() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let mut request = RequestData::default();
    let ctx = mk_ctx("ghost", ScriptType::Request);
    let result = engine
        .execute_request_script("ghost", &mut request, &ctx)
        .await;
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_execute_request_script_runtime_error() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Request, "boom", "throw new Error('boom');")
        .await
        .unwrap();
    let mut request = RequestData::default();
    let ctx = mk_ctx("boom", ScriptType::Request);
    let result = engine
        .execute_request_script("boom", &mut request, &ctx)
        .await;
    assert!(!result.success);
    assert!(result.error.unwrap().to_lowercase().contains("boom"));
}

#[tokio::test]
async fn test_execute_response_script_success_modifies_response() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"response.status = 503; response.body = "down";"#;
    engine
        .save_script(ScriptType::Response, "rmod", script)
        .await
        .unwrap();

    let mut response = ResponseData {
        status: 200,
        ..Default::default()
    };
    let ctx = mk_ctx("rmod", ScriptType::Response);
    let result = engine
        .execute_response_script("rmod", &mut response, &ctx)
        .await;
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(response.status, 503);
    assert_eq!(response.body.as_deref(), Some("down"));
}

#[tokio::test]
async fn test_execute_response_script_missing_returns_failure() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let mut response = ResponseData::default();
    let ctx = mk_ctx("ghost", ScriptType::Response);
    let result = engine
        .execute_response_script("ghost", &mut response, &ctx)
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_with_config_paths() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(
            ScriptType::Request,
            "cfgreq",
            r#"request.method = "DELETE";"#,
        )
        .await
        .unwrap();
    engine
        .save_script(ScriptType::Response, "cfgres", r#"response.status = 418;"#)
        .await
        .unwrap();

    let cfg = UnifiedConfig::default();

    let mut request = RequestData::default();
    let ctx = mk_ctx("cfgreq", ScriptType::Request);
    let r = engine
        .execute_request_script_with_config("cfgreq", &mut request, &ctx, &cfg)
        .await;
    assert!(r.success, "error: {:?}", r.error);
    assert_eq!(request.method, "DELETE");

    let mut response = ResponseData::default();
    let ctx = mk_ctx("cfgres", ScriptType::Response);
    let r = engine
        .execute_response_script_with_config("cfgres", &mut response, &ctx, &cfg)
        .await;
    assert!(r.success, "error: {:?}", r.error);
    assert_eq!(response.status, 418);
}

#[tokio::test]
async fn test_execute_decode_with_config_and_missing() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"ctx.output = { code: "0", data: "decoded", msg: "" };"#;
    engine
        .save_script(ScriptType::Decode, "dcfg", script)
        .await
        .unwrap();

    let cfg = UnifiedConfig::default();
    let ctx = mk_ctx("dcfg", ScriptType::Decode);
    let (out, _logs) = engine
        .execute_decode_script_with_config(
            "dcfg",
            "request",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"",
            &ctx,
            &cfg,
        )
        .await
        .unwrap();
    assert_eq!(out.data, "decoded");

    // Missing decode script -> Err.
    let err = engine
        .execute_decode_script(
            "missing",
            "request",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"",
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ScriptError::NotFound(_)));
}

#[tokio::test]
async fn test_execute_parser_with_config_local() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"ctx.output = { code: "0", data: ctx.phase, msg: "" };"#;
    engine
        .save_script(ScriptType::Parser, "pcfg", script)
        .await
        .unwrap();

    let cfg = UnifiedConfig::default();
    let ctx = mk_ctx("pcfg", ScriptType::Parser);
    let (out, _) = engine
        .execute_parser_script_with_config(
            "pcfg",
            "response",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"body",
            &ctx,
            &cfg,
        )
        .await
        .unwrap();
    assert_eq!(out.data, "response");
}

#[tokio::test]
async fn test_test_script_request_with_modifications() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Request);
    let result = engine
        .test_script(
            ScriptType::Request,
            r#"request.method = "PATCH"; request.body = "b";"#,
            Some(&RequestData::default()),
            None,
            &ctx,
        )
        .await;
    assert!(result.success, "error: {:?}", result.error);
    let mods = result.request_modifications.expect("should have mods");
    assert_eq!(mods.method.as_deref(), Some("PATCH"));
    assert_eq!(mods.body.as_deref(), Some("b"));
}

#[tokio::test]
async fn test_test_script_request_no_modifications() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Request);
    let result = engine
        .test_script(
            ScriptType::Request,
            r#"log.info("no change");"#,
            None,
            None,
            &ctx,
        )
        .await;
    assert!(result.success, "error: {:?}", result.error);
    assert!(result.request_modifications.is_none());
}

#[tokio::test]
async fn test_test_script_request_error() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Request);
    let result = engine
        .test_script(
            ScriptType::Request,
            "throw new Error('bad');",
            None,
            None,
            &ctx,
        )
        .await;
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn test_test_script_response_with_modifications() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Response);
    let result = engine
        .test_script(
            ScriptType::Response,
            r#"response.status = 201; response.body = "ok";"#,
            None,
            Some(&ResponseData::default()),
            &ctx,
        )
        .await;
    assert!(result.success, "error: {:?}", result.error);
    let mods = result.response_modifications.expect("should have mods");
    assert_eq!(mods.status, Some(201));
    assert_eq!(mods.body.as_deref(), Some("ok"));
}

#[tokio::test]
async fn test_test_script_decode_branch_with_response_phase() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Decode);
    let response = ResponseData {
        body: Some("rbody".to_string()),
        ..Default::default()
    };
    let result = engine
        .test_script(
            ScriptType::Decode,
            r#"ctx.output = { code: "0", data: ctx.phase, msg: "" };"#,
            None,
            Some(&response),
            &ctx,
        )
        .await;
    assert!(result.success, "error: {:?}", result.error);
    let out = result.decode_output.expect("decode output");
    assert_eq!(out.data, "response");
}

#[tokio::test]
async fn test_test_script_decode_error_branch() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("test", ScriptType::Decode);
    let result = engine
        .test_script(
            ScriptType::Decode,
            "throw new Error('decode fail');",
            Some(&RequestData::default()),
            None,
            &ctx,
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_test_script_with_config_uses_unified() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let cfg = UnifiedConfig::default();
    let ctx = mk_ctx("test", ScriptType::Request);
    let result = engine
        .test_script_with_config(
            ScriptType::Request,
            r#"log.info("hi");"#,
            None,
            None,
            &ctx,
            &cfg,
        )
        .await;
    assert!(result.success, "error: {:?}", result.error);
}

#[tokio::test]
async fn test_invalidate_cache_and_single_entry() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Request, "c1", "// v1")
        .await
        .unwrap();
    let _ = engine.load_script(ScriptType::Request, "c1").await.unwrap();

    // Overwrite file on disk, bypassing save (so cache is stale).
    std::fs::write(temp_dir.path().join("request/c1.js"), "// v2").unwrap();

    // Single-entry invalidation forces a re-read.
    engine
        .invalidate_script_cache(ScriptType::Request, "c1")
        .await;
    let v = engine.load_script(ScriptType::Request, "c1").await.unwrap();
    assert_eq!(v, "// v2");

    // Full invalidation is a no-op smoke check.
    engine.invalidate_cache().await;
}

#[tokio::test]
async fn test_parser_remote_ref_requires_sha() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let ctx = mk_ctx("https://example.com/p.js", ScriptType::Parser);
    // Remote ref without sha256 must error before any network call.
    let err = engine
        .execute_parser_script(
            "https://example.com/p.js",
            "response",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"",
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("sha256"), "err: {err}");
}

#[tokio::test]
async fn test_parser_remote_ref_rejects_http_non_localhost() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let sha = "a".repeat(64);
    let url = format!("http://evil.example.com/p.js?sha256={sha}");
    let ctx = mk_ctx(&url, ScriptType::Parser);
    let err = engine
        .execute_parser_script(
            &url,
            "response",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"",
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("localhost"), "err: {err}");
}

#[tokio::test]
async fn test_parser_remote_ref_rejects_bad_sha_length() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    // Valid https + sha present but wrong length triggers the 64-hex check.
    let url = "https://example.com/p.js?sha256=deadbeef";
    let ctx = mk_ctx(url, ScriptType::Parser);
    let err = engine
        .execute_parser_script(
            url,
            "response",
            &RequestData::default(),
            b"",
            &ResponseData::default(),
            b"",
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("64-character hex"), "err: {err}");
}

#[tokio::test]
async fn test_execute_request_script_applies_header_modifications() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"request.headers["X-Added"] = "yes";"#;
    engine
        .save_script(ScriptType::Request, "hmod", script)
        .await
        .unwrap();

    let mut request = RequestData {
        method: "GET".to_string(),
        ..Default::default()
    };
    let ctx = mk_ctx("hmod", ScriptType::Request);
    let result = engine
        .execute_request_script("hmod", &mut request, &ctx)
        .await;
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(
        request.headers.get("X-Added").map(|s| s.as_str()),
        Some("yes")
    );
}

#[tokio::test]
async fn test_execute_response_script_applies_status_text_and_headers() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    let script = r#"
        response.statusText = "Service Unavailable";
        response.headers["X-R"] = "1";
    "#;
    engine
        .save_script(ScriptType::Response, "rhmod", script)
        .await
        .unwrap();

    let mut response = ResponseData {
        status: 200,
        status_text: "OK".to_string(),
        ..Default::default()
    };
    let ctx = mk_ctx("rhmod", ScriptType::Response);
    let result = engine
        .execute_response_script("rhmod", &mut response, &ctx)
        .await;
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(response.status_text, "Service Unavailable");
    assert_eq!(response.headers.get("X-R").map(|s| s.as_str()), Some("1"));
}

#[tokio::test]
async fn test_execute_response_script_runtime_error() {
    let temp_dir = TempDir::new().unwrap();
    let engine = mk_engine(temp_dir.path());
    engine.init().await.unwrap();

    engine
        .save_script(ScriptType::Response, "rboom", "throw new Error('rboom');")
        .await
        .unwrap();
    let mut response = ResponseData::default();
    let ctx = mk_ctx("rboom", ScriptType::Response);
    let result = engine
        .execute_response_script("rboom", &mut response, &ctx)
        .await;
    assert!(!result.success);
    assert!(result.error.unwrap().to_lowercase().contains("rboom"));
}
