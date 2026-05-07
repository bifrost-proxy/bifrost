use bifrost_core::{
    get_filter_value_specs, get_syntax_info, PatternInfo, ProtocolInfo, ProtocolValueSpec,
    TemplateVariableInfo,
};
use hyper::{body::Incoming, Method, Request, Response};
use serde::Serialize;
use std::collections::HashMap;

use super::{json_response, method_not_allowed, BoxBody};
use crate::state::SharedAdminState;

#[derive(Debug, Serialize)]
pub struct ScriptListItem {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ScriptsInfo {
    pub request_scripts: Vec<ScriptListItem>,
    pub response_scripts: Vec<ScriptListItem>,
    pub decode_scripts: Vec<ScriptListItem>,
    pub parser_scripts: Vec<ScriptListItem>,
}

#[derive(Debug, Serialize)]
pub struct UnifiedSyntaxInfo {
    pub protocols: Vec<ProtocolInfo>,
    pub template_variables: Vec<TemplateVariableInfo>,
    pub patterns: Vec<PatternInfo>,
    pub protocol_aliases: HashMap<String, String>,
    pub scripts: ScriptsInfo,
    pub filter_specs: Vec<ProtocolValueSpec>,
}

pub async fn handle_syntax(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    if path == "/api/syntax" || path == "/api/syntax/" {
        if req.method() == Method::GET {
            get_unified_syntax(state).await
        } else {
            method_not_allowed()
        }
    } else {
        method_not_allowed()
    }
}

async fn get_unified_syntax(state: SharedAdminState) -> Response<BoxBody> {
    let base_info = get_syntax_info();
    let filter_specs = get_filter_value_specs();

    let scripts_info = if let Some(ref script_manager) = state.script_manager {
        let manager = script_manager.read().await;
        let engine = manager.engine();

        let request_scripts = engine
            .list_scripts(bifrost_script::ScriptType::Request)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| ScriptListItem {
                name: s.name,
                description: s.description,
            })
            .collect();

        let response_scripts = engine
            .list_scripts(bifrost_script::ScriptType::Response)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| ScriptListItem {
                name: s.name,
                description: s.description,
            })
            .collect();

        let mut decode_scripts: Vec<ScriptListItem> = vec![
            ScriptListItem {
                name: "utf8".to_string(),
                description: Some("Built-in UTF-8 (lossy) decoder".to_string()),
            },
            ScriptListItem {
                name: "default".to_string(),
                description: Some("Alias of built-in UTF-8 decoder".to_string()),
            },
            ScriptListItem {
                name: "bp".to_string(),
                description: Some("Run parser scripts bound by bp:// rules".to_string()),
            },
        ];

        let mut user_decode_scripts: Vec<ScriptListItem> = engine
            .list_scripts(bifrost_script::ScriptType::Decode)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| ScriptListItem {
                name: s.name,
                description: s.description,
            })
            .collect();
        decode_scripts.append(&mut user_decode_scripts);

        let parser_scripts = engine
            .list_scripts(bifrost_script::ScriptType::Parser)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| ScriptListItem {
                name: s.name,
                description: s.description,
            })
            .collect();

        ScriptsInfo {
            request_scripts,
            response_scripts,
            decode_scripts,
            parser_scripts,
        }
    } else {
        ScriptsInfo {
            request_scripts: Vec::new(),
            response_scripts: Vec::new(),
            decode_scripts: vec![
                ScriptListItem {
                    name: "utf8".to_string(),
                    description: Some("Built-in UTF-8 (lossy) decoder".to_string()),
                },
                ScriptListItem {
                    name: "default".to_string(),
                    description: Some("Alias of built-in UTF-8 decoder".to_string()),
                },
                ScriptListItem {
                    name: "bp".to_string(),
                    description: Some("Run parser scripts bound by bp:// rules".to_string()),
                },
            ],
            parser_scripts: Vec::new(),
        }
    };

    let unified = UnifiedSyntaxInfo {
        protocols: base_info.protocols,
        template_variables: base_info.template_variables,
        patterns: base_info.patterns,
        protocol_aliases: base_info.protocol_aliases,
        scripts: scripts_info,
        filter_specs,
    };

    json_response(&unified)
}
