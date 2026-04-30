use std::sync::Arc;

use bifrost_admin::{
    devtools::{DevtoolsMode as AdminDevtoolsMode, MatchedDevtoolsRule, RegisterPageInput},
    AdminState,
};
use bifrost_core::protocol::Protocol;
use bytes::Bytes;
use hyper::header::HeaderName;
use url::Url;

use crate::server::{DevtoolsInjectMode, DevtoolsMode, DevtoolsRule, ResolvedRules};

pub(super) const DEVTOOLS_CLIENT_REQ_ID_HEADER: &str = "x-bifrost-client-request-id";

pub(super) fn take_devtools_client_req_id(headers: &mut hyper::HeaderMap) -> Option<String> {
    let name = HeaderName::from_static(DEVTOOLS_CLIENT_REQ_ID_HEADER);
    let value = headers
        .get(&name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    headers.remove(&name);
    value
}

pub(super) fn is_devtools_client_req_id_header(name: &str) -> bool {
    name.eq_ignore_ascii_case(DEVTOOLS_CLIENT_REQ_ID_HEADER)
}

pub(super) fn bind_devtools_client_req_traffic(
    admin_state: &Option<Arc<AdminState>>,
    client_req_id: &Option<String>,
    traffic_id: &str,
) {
    if let (Some(state), Some(client_req_id)) = (admin_state.as_ref(), client_req_id.as_ref()) {
        state
            .devtools_broker
            .bind_client_req_traffic(client_req_id, traffic_id);
    }
}

pub(super) fn devtools_bridge_requested(rules: &ResolvedRules) -> bool {
    effective_devtools_rule(rules)
        .as_ref()
        .map(|rule| {
            !rule.deny
                && matches!(
                    rule.inject,
                    DevtoolsInjectMode::Auto | DevtoolsInjectMode::Bridge
                )
        })
        .unwrap_or(false)
}

fn effective_devtools_rule(rules: &ResolvedRules) -> Option<DevtoolsRule> {
    if let Some(rule) = rules.devtools.clone() {
        return Some(rule);
    }
    let matched = rules
        .rules
        .iter()
        .rev()
        .find(|rule| rule.protocol == Protocol::DevTools)?;
    Some(parse_devtools_rule_value(&matched.value))
}

fn parse_devtools_rule_value(value: &str) -> DevtoolsRule {
    let mut rule = DevtoolsRule {
        raw_value: value.to_string(),
        ..Default::default()
    };

    for part in value.split([',', '&']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, raw_value) = part.split_once('=').unwrap_or((part, "true"));
        let key = key.trim();
        let raw_value = raw_value.trim();
        match key {
            "mode" if raw_value.eq_ignore_ascii_case("control") => {
                rule.mode = DevtoolsMode::Control;
            }
            "mode" if raw_value.eq_ignore_ascii_case("read") => {
                rule.mode = DevtoolsMode::Read;
            }
            "inject" if raw_value.eq_ignore_ascii_case("bridge") => {
                rule.inject = DevtoolsInjectMode::Bridge;
            }
            "inject" if raw_value.eq_ignore_ascii_case("off") => {
                rule.inject = DevtoolsInjectMode::Off;
            }
            "inject" if raw_value.eq_ignore_ascii_case("auto") => {
                rule.inject = DevtoolsInjectMode::Auto;
            }
            "deny" => {
                rule.deny = matches!(
                    raw_value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "evaluate_allowlist" => {
                rule.evaluate_allowlist = parse_devtools_evaluate_allowlist(raw_value);
            }
            _ => {}
        }
    }

    rule
}

fn parse_devtools_evaluate_allowlist(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed);
    inner
        .split('|')
        .map(|item| item.trim().trim_matches('"').trim_matches('\''))
        .filter(|item| !item.is_empty())
        .map(|item| item.replace("\\\\", "\\"))
        .collect()
}

fn admin_devtools_mode(rule: &crate::server::DevtoolsRule) -> AdminDevtoolsMode {
    match rule.mode {
        crate::server::DevtoolsMode::Read => AdminDevtoolsMode::Read,
        crate::server::DevtoolsMode::Control => AdminDevtoolsMode::Control,
    }
}

fn devtools_matched_rule(rules: &ResolvedRules) -> Option<MatchedDevtoolsRule> {
    let effective_allowlist = effective_devtools_rule(rules)
        .map(|rule| rule.evaluate_allowlist)
        .unwrap_or_default();
    rules
        .rules
        .iter()
        .rev()
        .find(|rule| rule.protocol == Protocol::DevTools)
        .map(|rule| MatchedDevtoolsRule {
            pattern: rule.pattern.clone(),
            raw: rule.raw.clone(),
            line: rule.line,
            evaluate_allowlist: if effective_allowlist.is_empty() {
                parse_devtools_evaluate_allowlist_from_value(&rule.value)
            } else {
                effective_allowlist
            },
        })
}

fn parse_devtools_evaluate_allowlist_from_value(value: &str) -> Vec<String> {
    value
        .split([',', '&'])
        .find_map(|part| {
            let (key, raw_value) = part.trim().split_once('=')?;
            (key.trim() == "evaluate_allowlist")
                .then(|| parse_devtools_evaluate_allowlist(raw_value))
        })
        .unwrap_or_default()
}

fn origin_from_url(url: &str) -> String {
    Url::parse(url)
        .map(|parsed| {
            let scheme = parsed.scheme();
            let host = parsed.host_str().unwrap_or_default();
            if let Some(port) = parsed.port() {
                format!("{scheme}://{host}:{port}")
            } else {
                format!("{scheme}://{host}")
            }
        })
        .unwrap_or_default()
}

fn insert_devtools_bridge_script(html: &str, script: &str) -> String {
    let mut html = html.to_string();
    let lower = html.to_lowercase();

    if let Some(head_start) = lower.find("<head") {
        if let Some(head_end_offset) = lower[head_start..].find('>') {
            let insert_at = head_start + head_end_offset + 1;
            html.insert_str(insert_at, script);
            return html;
        }
    }

    if let Some(html_start) = lower.find("<html") {
        if let Some(html_end_offset) = lower[html_start..].find('>') {
            let insert_at = html_start + html_end_offset + 1;
            html.insert_str(insert_at, script);
            return html;
        }
    }

    format!("{script}{html}")
}

fn devtools_bridge_script(page_id: &str, token: &str) -> String {
    let endpoint = format!("/_bifrost/api/devtools/bridge/{page_id}");
    let endpoint_json = serde_json::to_string(&endpoint).unwrap_or_else(|_| "\"\"".to_string());
    let page_id_json = serde_json::to_string(page_id).unwrap_or_else(|_| "\"\"".to_string());
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r##"<script id="__bifrost_devtools_bridge__">
(function() {{
  if (Object.prototype.hasOwnProperty.call(window, "__BIFROST_DEVTOOLS_BRIDGE__")) return;
  const endpoint = {endpoint_json};
  const token = {token_json};
  const pageId = {page_id_json};
  const bridgeHost = "bifrost.local";
  const clientReqHeader = "x-bifrost-client-request-id";
  let bridgeSocket = null;
  let bridgeSocketReady = false;
  let bridgeSocketReconnectTimer = 0;
  let bridgeOutbox = [];
  let bridgeInflight = Object.create(null);
  let bridgeFlushTimer = 0;
  let bridgeSeq = 0;
  let lastHelloArgs = null;
  let bridgeState = "connecting";
  const tabWindowNamePrefix = "__bifrost_devtools_tab_id__:";
  const tabStorageKey = "__bifrost_devtools_tab_id__";
  let tabId = "";
  try {{
    if (window.name && window.name.indexOf(tabWindowNamePrefix) === 0) {{
      tabId = window.name.slice(tabWindowNamePrefix.length);
    }}
    if (!tabId) {{
      tabId = window.sessionStorage.getItem(tabStorageKey) || "";
    }}
    if (!tabId) {{
      tabId = "tab_" + Math.random().toString(36).slice(2) + Date.now().toString(36);
      window.sessionStorage.setItem(tabStorageKey, tabId);
    }}
    window.name = tabWindowNamePrefix + tabId;
  }} catch (_) {{
    tabId = "tab_" + Math.random().toString(36).slice(2) + Date.now().toString(36);
    try {{ window.name = tabWindowNamePrefix + tabId; }} catch (_) {{}}
  }}
  const post = function(path, payload) {{
    try {{
      const messageType = path.replace(/^\//, "").replace(/-([a-z])/g, function(_, ch) {{ return "_" + ch; }});
      const message = Object.assign({{type: messageType, token: token}}, payload || {{}});
      message.seq = ++bridgeSeq;
      bridgeOutbox.push(message);
      if (bridgeOutbox.length > 1000) bridgeOutbox = bridgeOutbox.slice(-1000);
      scheduleBridgeFlush(25);
    }} catch (_) {{}}
  }};
  const flushBridgeOutbox = function() {{
    bridgeFlushTimer = 0;
    try {{
      if (!bridgeSocketReady || !bridgeSocket || bridgeSocket.readyState !== 1) {{
        connectBridgeSocket();
        return;
      }}
      const batch = bridgeOutbox.splice(0, 50);
      batch.forEach(function(message) {{
        try {{
          bridgeInflight[message.seq] = message;
          bridgeSocket.send(JSON.stringify(message));
        }} catch (_) {{
          delete bridgeInflight[message.seq];
          bridgeOutbox.unshift(message);
        }}
      }});
      if (bridgeOutbox.length) scheduleBridgeFlush(16);
    }} catch (_) {{}}
  }};
  const scheduleBridgeFlush = function(delay) {{
    if (bridgeFlushTimer) return;
    bridgeFlushTimer = window.setTimeout(flushBridgeOutbox, delay || 25);
  }};
  const closeBridge = function() {{
    try {{
      const message = JSON.stringify({{type: "close", token: token}});
      if (bridgeSocket && bridgeSocket.readyState === 1) {{
        bridgeSocket.send(message);
        bridgeSocket.close();
      }}
    }} catch (_) {{}}
  }};
  const shim = Object.freeze({{
    page_id: pageId,
    tab_id: tabId,
    get state() {{ return bridgeState; }},
    send: function(msgId, payload) {{
      if (msgId !== "ping") {{
        console.warn("[Bifrost DevTools] dropped page bridge shim send", msgId);
        return false;
      }}
      return true;
    }}
  }});
  Object.defineProperty(window, "__BIFROST_DEVTOOLS_BRIDGE__", {{
    value: shim,
    enumerable: false,
    configurable: false,
    writable: false
  }});
  window.addEventListener("message", function(event) {{
    if (event.source !== window || event.origin !== window.location.origin) {{
      console.warn("[Bifrost DevTools] dropped postMessage from invalid source or origin", event.origin);
      return;
    }}
    const data = event.data || {{}};
    if (!data || data.__bifrost_devtools_bridge__ !== true) return;
    console.warn("[Bifrost DevTools] dropped unsupported postMessage", data.type || "");
  }}, false);
  window.addEventListener("pagehide", closeBridge, false);
  let nextNodeId = 1;
  let nodeMap = Object.create(null);
  let domRefreshTimer = 0;
  let storageRefreshTimer = 0;
  let lastDomSignature = "";
  let lastStorageSnapshot = "";
  let highlightedNode = null;
  let highlightOverlay = null;
  const observedResources = Object.create(null);
  let consoleBuffer = [];
  let networkBuffer = [];
  const internalNodeIds = {{
    "__bifrost_devtools_bridge__": true,
    "__bifrost_devtools_highlight__": true
  }};
  const isBridgeInternalNode = function(node) {{
    if (!node) return false;
    if (node.nodeType !== Node.ELEMENT_NODE) node = node.parentElement;
    if (!node) return false;
    if (internalNodeIds[node.id]) return true;
    try {{
      return !!(node.closest && node.closest("#__bifrost_devtools_bridge__,#__bifrost_devtools_highlight__"));
    }} catch (_) {{
      return false;
    }}
  }};
  const externalChildNodes = function(node) {{
    return Array.prototype.slice.call(node.childNodes || []).filter(function(child) {{
      return !isBridgeInternalNode(child);
    }});
  }};
  const sanitizedOuterHTML = function() {{
    try {{
      if (!document.documentElement) return "";
      const clone = document.documentElement.cloneNode(true);
      Array.prototype.slice.call(clone.querySelectorAll("#__bifrost_devtools_bridge__,#__bifrost_devtools_highlight__")).forEach(function(node) {{
        if (node.parentNode) node.parentNode.removeChild(node);
      }});
      return clone.outerHTML.slice(0, 1048576);
    }} catch (_) {{
      return document.documentElement ? document.documentElement.outerHTML.slice(0, 1048576) : "";
    }}
  }};
  const attrList = function(node) {{
    const attrs = [];
    if (!node || !node.attributes) return attrs;
    for (let i = 0; i < node.attributes.length; i++) {{
      attrs.push(node.attributes[i].name, node.attributes[i].value);
    }}
    return attrs;
  }};
  const serializeNode = function(node) {{
    const id = nextNodeId++;
    nodeMap[id] = node;
    const children = externalChildNodes(node);
    if (node.nodeType === Node.DOCUMENT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 9,
        nodeName: "#document",
        localName: "",
        nodeValue: "",
        documentURL: location.href,
        baseURL: document.baseURI || location.href,
        xmlVersion: "",
        compatibilityMode: document.compatMode === "BackCompat" ? "QuirksMode" : "NoQuirksMode",
        children: children.map(serializeNode)
      }};
    }}
    if (node.nodeType === Node.TEXT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 3,
        nodeName: "#text",
        localName: "",
        nodeValue: (node.nodeValue || "").slice(0, 4096)
      }};
    }}
    if (node.nodeType === Node.COMMENT_NODE) {{
      return {{
        nodeId: id,
        backendNodeId: id,
        nodeType: 8,
        nodeName: "#comment",
        localName: "",
        nodeValue: (node.nodeValue || "").slice(0, 4096)
      }};
    }}
    return {{
      nodeId: id,
      backendNodeId: id,
      nodeType: node.nodeType,
      nodeName: node.nodeName || "",
      localName: node.localName || "",
      nodeValue: node.nodeValue || "",
      attributes: attrList(node),
      childNodeCount: children.length,
      children: children.slice(0, 2500).map(serializeNode)
    }};
  }};
  const storageSnapshot = function() {{
    const collect = function(storage) {{
      const entries = [];
      try {{
        for (let i = 0; i < storage.length; i++) {{
          const key = storage.key(i);
          entries.push([key, storage.getItem(key)]);
        }}
      }} catch (_) {{}}
      return entries;
    }};
    const cookies = [];
    try {{
      if (document.cookie) {{
        document.cookie.split(";").forEach(function(part) {{
          const idx = part.indexOf("=");
          if (idx > -1) cookies.push([part.slice(0, idx).trim(), part.slice(idx + 1)]);
        }});
      }}
    }} catch (_) {{}}
    return {{
      local_storage: collect(window.localStorage),
      session_storage: collect(window.sessionStorage),
      cookies: cookies
    }};
  }};
  const rememberNetwork = function(event) {{
    if (!event || !event.url || event.url.indexOf("/_bifrost/api/devtools/bridge/") !== -1) return null;
    const method = event.method || "GET";
    const performanceOnly = !event.client_req_id && (!event.status || event.status === 0);
    if (performanceOnly) {{
      for (let i = networkBuffer.length - 1; i >= Math.max(0, networkBuffer.length - 80); i--) {{
        const item = networkBuffer[i];
        if (item && item.client_req_id && item.url === event.url && item.method === method) return null;
      }}
    }} else if (event.client_req_id) {{
      for (let i = networkBuffer.length - 1; i >= Math.max(0, networkBuffer.length - 80); i--) {{
        const item = networkBuffer[i];
        if (item && !item.client_req_id && (!item.status || item.status === 0) && item.url === event.url && item.method === method) {{
          networkBuffer.splice(i, 1);
          break;
        }}
      }}
    }}
	    const normalized = {{
	      url: event.url,
	      method: method,
	      status: event.status,
	      type: event.type || "Other",
	      query_params: event.query_params || queryPairs(event.url),
	      request_headers: sanitizeHeaderPairs(event.request_headers || []),
	      response_headers: sanitizeHeaderPairs(event.response_headers || []),
	      from_cache: typeof event.from_cache === "boolean" ? event.from_cache : null,
	      client_req_id: event.client_req_id || null
	    }};
    networkBuffer.push(normalized);
    if (networkBuffer.length > 1000) networkBuffer = networkBuffer.slice(-1000);
    return normalized;
  }};
  const performanceNetwork = function() {{
    try {{
      return performance.getEntriesByType("resource").filter(function(entry) {{
        return entry.name && entry.name.indexOf("/_bifrost/api/devtools/bridge/") === -1;
      }}).slice(-100).filter(function(entry) {{
        const key = entry.name + "::" + entry.startTime;
        if (observedResources[key]) return false;
        observedResources[key] = true;
        return true;
      }}).map(function(entry) {{
	        return rememberNetwork({{
	          url: entry.name,
	          method: "GET",
	          status: 0,
	          type: entry.initiatorType || "Other",
	          query_params: queryPairs(entry.name),
	          from_cache: entry.transferSize === 0 && entry.encodedBodySize > 0
	        }});
      }}).filter(Boolean);
    }} catch (_) {{
      return [];
    }}
  }};
  const normalizeSnapshotScope = function(scope) {{
    if (scope === "dom.snapshot") return "elements";
    if (scope === "console.messages") return "console";
    if (scope === "cookie" || scope === "local_storage" || scope === "session_storage") return "storage";
    return scope || "full";
  }};
  const hello = function(scope, networkEvents) {{
    scope = normalizeSnapshotScope(scope);
    lastHelloArgs = {{scope: scope, networkEvents: networkEvents || []}};
    const includeDom = scope === "full" || scope === "elements";
    const includeStorage = scope === "full" || scope === "storage";
    const includeConsole = scope === "full" || scope === "console";
    const includeNetwork = scope === "full" || scope === "network";
    if (includeDom) {{
      nextNodeId = 1;
      nodeMap = Object.create(null);
    }}
    const payload = {{
      scope: scope,
      tab_id: tabId,
      title: document.title || null,
      url: location.href,
      user_agent: navigator.userAgent
    }};
    (networkEvents || []).forEach(rememberNetwork);
    performanceNetwork();
    if (includeDom) {{
      const domTree = serializeNode(document);
      const domSignature = JSON.stringify(domTree);
      lastDomSignature = domSignature;
      payload.dom_snapshot = sanitizedOuterHTML();
      payload.dom_tree = domTree;
    }}
    if (includeStorage) payload.storage = storageSnapshot();
    if (includeConsole) payload.console = consoleBuffer.slice(-500);
    if (includeNetwork) payload.network = networkBuffer.slice(-1000);
    post("/hello", payload);
  }};
  const sendStorageIfChanged = function() {{
    try {{
      const snapshot = storageSnapshot();
      const serialized = JSON.stringify(snapshot);
      if (serialized === lastStorageSnapshot) return;
      lastStorageSnapshot = serialized;
      hello("storage", []);
    }} catch (_) {{}}
  }};
  const scheduleStorageRefresh = function(delay) {{
    if (storageRefreshTimer) return;
    storageRefreshTimer = window.setTimeout(function() {{
      storageRefreshTimer = 0;
      sendStorageIfChanged();
    }}, delay || 50);
  }};
  try {{
    Object.defineProperty(window, "__BIFROST_DEVTOOLS_BRIDGE_SYNC_STORAGE__", {{
      value: function() {{ scheduleStorageRefresh(1); }},
      enumerable: false,
      configurable: false,
      writable: false
    }});
  }} catch (_) {{}}
  const scheduleDomRefresh = function(delay) {{
    if (domRefreshTimer) return;
    domRefreshTimer = window.setTimeout(function() {{
      domRefreshTimer = 0;
      hello("elements", performanceNetwork());
    }}, delay || 250);
  }};
  const isExternalStructuralMutation = function(mutation) {{
    if (!mutation || mutation.type !== "childList") return false;
    if (isBridgeInternalNode(mutation.target)) return false;
    const added = Array.prototype.slice.call(mutation.addedNodes || []);
    const removed = Array.prototype.slice.call(mutation.removedNodes || []);
    return added.concat(removed).some(function(node) {{
      return !isBridgeInternalNode(node);
    }});
  }};
  const ensureHighlightOverlay = function() {{
    if (highlightOverlay && document.documentElement.contains(highlightOverlay)) return highlightOverlay;
    highlightOverlay = document.createElement("div");
    highlightOverlay.id = "__bifrost_devtools_highlight__";
    highlightOverlay.style.cssText = [
      "position:fixed",
      "z-index:2147483647",
      "pointer-events:none",
      "border:2px solid #1677ff",
      "box-shadow:0 0 0 99999px rgba(22,119,255,0.08),0 0 0 1px rgba(255,255,255,0.85) inset",
      "border-radius:2px",
      "box-sizing:border-box",
      "display:none"
    ].join(";");
    (document.documentElement || document.body).appendChild(highlightOverlay);
    return highlightOverlay;
  }};
  const updateHighlightOverlay = function() {{
    if (!highlightedNode || !highlightedNode.getBoundingClientRect) return;
    const rect = highlightedNode.getBoundingClientRect();
    const overlay = ensureHighlightOverlay();
    if (rect.width <= 0 && rect.height <= 0) {{
      overlay.style.display = "none";
      return;
    }}
    overlay.style.display = "block";
    overlay.style.left = Math.max(0, rect.left) + "px";
    overlay.style.top = Math.max(0, rect.top) + "px";
    overlay.style.width = Math.max(1, rect.width) + "px";
    overlay.style.height = Math.max(1, rect.height) + "px";
  }};
  const highlightNode = function(nodeId) {{
    const node = nodeMap[nodeId];
    highlightedNode = node && node.nodeType === Node.TEXT_NODE ? node.parentElement : node;
    updateHighlightOverlay();
  }};
  const hideHighlight = function() {{
    highlightedNode = null;
    if (highlightOverlay) highlightOverlay.style.display = "none";
  }};
  const handleOverlayCommand = function(command) {{
    if (!command) return;
    if (command.type === "highlight_node") highlightNode(command.node_id);
    if (command.type === "hide_highlight") hideHighlight();
  }};
  const truncateText = function(value, limit) {{
    value = String(value);
    limit = limit || 160;
    return value.length > limit ? value.slice(0, limit - 1) + "..." : value;
  }};
  const rawConsoleArg = function(value) {{
    try {{
      if (typeof value === "string") return value;
      if (value === undefined) return "undefined";
      if (typeof value === "function") return String(value);
      const seen = new WeakSet();
      return JSON.stringify(value, function(_, item) {{
        if (typeof item === "bigint") return String(item) + "n";
        if (typeof item === "function") return String(item);
        if (typeof item === "object" && item !== null) {{
          if (seen.has(item)) return "[Circular]";
          seen.add(item);
        }}
        return item;
      }});
    }} catch (_) {{
      try {{ return String(value); }} catch (_) {{ return "[Unserializable]"; }}
    }}
  }};
  const consolePreview = function(value) {{
    try {{
      if (value === null) return "null";
      if (value === undefined) return "undefined";
      if (typeof value === "string") return JSON.stringify(truncateText(value, 120));
      if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") return String(value);
      if (typeof value === "function") return truncateText(String(value), 120);
      if (Array.isArray(value)) {{
        return "Array(" + value.length + ") [" + value.slice(0, 3).map(consolePreview).join(", ") + (value.length > 3 ? ", ..." : "") + "]";
      }}
      if (value instanceof Error) return value.name + ": " + value.message;
      if (value instanceof Date) return "Date " + value.toISOString();
      if (value && value.nodeType === 1) return "<" + String(value.tagName || "element").toLowerCase() + (value.id ? "#" + value.id : "") + ">";
      const keys = Object.keys(value || {{}});
      return "Object {{" + keys.slice(0, 4).map(function(key) {{
        return key + ": " + consolePreview(value[key]);
      }}).join(", ") + (keys.length > 4 ? ", ..." : "") + "}}";
    }} catch (_) {{
      return "[Preview unavailable]";
    }}
  }};
  const serializeConsoleArg = function(value, depth, seen) {{
    depth = depth || 0;
    seen = seen || new WeakSet();
    const valueType = value === null ? "null" : typeof value;
    const result = {{
      type: valueType,
      preview: consolePreview(value),
      raw: rawConsoleArg(value)
    }};
    if (value === null || value === undefined || valueType === "string" || valueType === "number" || valueType === "boolean") {{
      result.value = value;
      return result;
    }}
    if (valueType === "bigint") {{
      result.value = String(value) + "n";
      return result;
    }}
    if (valueType === "function") {{
      result.subtype = "function";
      result.value = truncateText(String(value), 4096);
      return result;
    }}
    if (value instanceof Date) result.subtype = "date";
    if (value instanceof Error) result.subtype = "error";
    if (Array.isArray(value)) result.subtype = "array";
    if (value && value.nodeType) result.subtype = "node";
    if (depth >= 3 || !value || valueType !== "object") return result;
    if (seen.has(value)) {{
      result.subtype = "circular";
      result.preview = "[Circular]";
      return result;
    }}
    seen.add(value);
    try {{
      const names = Array.isArray(value)
        ? value.map(function(_, index) {{ return String(index); }})
        : Object.keys(value);
      result.properties = names.slice(0, 80).map(function(name) {{
        let child;
        try {{ child = value[name]; }} catch (error) {{ child = String(error); }}
        return {{name: name, value: serializeConsoleArg(child, depth + 1, seen)}};
      }});
      if (Array.isArray(value)) {{
        result.properties.push({{name: "length", value: {{type: "number", value: value.length, preview: String(value.length), raw: String(value.length)}}}});
      }}
      if (names.length > 80) result.overflow = names.length - 80;
    }} catch (_) {{}}
    return result;
  }};
  const serializeConsoleArgs = function(args) {{
    return Array.prototype.slice.call(args).map(function(value) {{
      return serializeConsoleArg(value, 0, new WeakSet());
    }});
  }};
  const stringifyArgs = function(args) {{
    return Array.prototype.slice.call(args).map(function(value) {{
      return consolePreview(value);
    }}).join(" ");
  }};
  ["log", "info", "warn", "error", "debug"].forEach(function(level) {{
    const original = console[level];
    console[level] = function() {{
      try {{
        const serializedArgs = serializeConsoleArgs(arguments);
        const raw = serializedArgs.map(function(arg) {{ return arg.raw || arg.preview || ""; }}).join(" ");
        const message = {{level: level, text: stringifyArgs(arguments), at_ms: Date.now(), args: serializedArgs, raw: raw}};
        consoleBuffer.push(message);
        if (consoleBuffer.length > 500) consoleBuffer = consoleBuffer.slice(-500);
        post("/console", {{level: level, text: message.text, at_ms: message.at_ms, args: message.args, raw: message.raw}});
      }} catch (_) {{}}
      return original.apply(console, arguments);
    }};
  }});
  const recordNetwork = function(event) {{
    const normalized = rememberNetwork(event);
    if (!normalized) return;
    post("/network", {{event: normalized}});
  }};
  const nextClientReqId = function() {{
    return pageId + "-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
  }};
	  const canAttachClientReqHeader = function(url) {{
	    try {{
	      const parsed = new URL(url, location.href);
	      return parsed.origin === location.origin;
	    }} catch (_) {{
	      return false;
	    }}
	  }};
	  const sanitizeHeaderPairs = function(pairs) {{
	    const output = [];
	    try {{
	      (pairs || []).forEach(function(pair) {{
	        if (!pair || pair.length < 2) return;
	        const key = String(pair[0] || "");
	        if (!key || key.toLowerCase() === clientReqHeader.toLowerCase()) return;
	        output.push([key, String(pair[1] == null ? "" : pair[1])]);
	      }});
	    }} catch (_) {{}}
	    return output.slice(0, 120);
	  }};
	  const headerPairsFromHeaders = function(headers) {{
	    const pairs = [];
	    try {{
	      if (!headers) return pairs;
	      const normalized = new Headers(headers);
	      normalized.forEach(function(value, key) {{
	        pairs.push([String(key), String(value)]);
	      }});
	    }} catch (_) {{}}
	    return sanitizeHeaderPairs(pairs);
	  }};
	  const queryPairs = function(url) {{
	    const pairs = [];
	    try {{
	      const parsed = new URL(url, location.href);
	      parsed.searchParams.forEach(function(value, key) {{
	        pairs.push([String(key), String(value)]);
	      }});
	    }} catch (_) {{}}
	    return pairs.slice(0, 120);
	  }};
	  const xhrResponseHeaders = function(xhr) {{
	    const pairs = [];
	    try {{
	      const raw = xhr.getAllResponseHeaders && xhr.getAllResponseHeaders();
	      if (!raw) return pairs;
	      raw.trim().split(/[\r\n]+/).forEach(function(line) {{
	        const index = line.indexOf(":");
	        if (index <= 0) return;
	        pairs.push([line.slice(0, index).trim(), line.slice(index + 1).trim()]);
	      }});
	    }} catch (_) {{}}
	    return sanitizeHeaderPairs(pairs);
	  }};
	  try {{
	    if (window.PerformanceObserver) {{
	      const resourceObserver = new PerformanceObserver(function(list) {{
	        list.getEntries().forEach(function(entry) {{
	          if (!entry || !entry.name || entry.name.indexOf("/_bifrost/api/devtools/bridge/") !== -1) return;
	          const key = entry.name + "::" + entry.startTime;
	          if (observedResources[key]) return;
	          observedResources[key] = true;
	          recordNetwork({{url: entry.name, method: "GET", status: 0, type: entry.initiatorType || "Other", query_params: queryPairs(entry.name), from_cache: entry.transferSize === 0 && entry.encodedBodySize > 0}});
	        }});
	      }});
      resourceObserver.observe({{type: "resource", buffered: true}});
    }}
  }} catch (_) {{}}
  try {{
    if (window.Storage && window.Storage.prototype) {{
      ["setItem", "removeItem", "clear"].forEach(function(method) {{
        const original = window.Storage.prototype[method];
        if (typeof original !== "function") return;
        window.Storage.prototype[method] = function() {{
          const result = original.apply(this, arguments);
          scheduleStorageRefresh(50);
          return result;
        }};
      }});
    }}
  }} catch (_) {{}}
  window.addEventListener("storage", function() {{ scheduleStorageRefresh(50); }}, true);
  const remoteObject = function(value) {{
    if (value === undefined) return {{type: "undefined", description: "undefined"}};
    if (value === null) return {{type: "object", subtype: "null", value: null, description: "null"}};
    const valueType = typeof value;
    if (valueType === "string" || valueType === "boolean" || valueType === "number") {{
      return {{type: valueType, value: value, description: String(value)}};
    }}
    if (valueType === "bigint") {{
      return {{type: "bigint", unserializableValue: String(value) + "n", description: String(value) + "n"}};
    }}
    if (valueType === "function") {{
      return {{type: "function", description: String(value).slice(0, 4096)}};
    }}
    let description = "";
    try {{ description = JSON.stringify(value); }} catch (_) {{ description = String(value); }}
    return {{type: "object", description: (description || String(value)).slice(0, 4096)}};
  }};
  const handleEvalCommand = function(command) {{
    if (!command) return;
    try {{
      const value = (0, eval)(command.expression);
      Promise.resolve(value).then(function(resolved) {{
        post("/eval-result", {{eval_id: command.eval_id, result: remoteObject(resolved)}});
      }}, function(error) {{
        post("/eval-result", {{eval_id: command.eval_id, exception: String(error && error.stack || error)}});
      }});
    }} catch (error) {{
      post("/eval-result", {{eval_id: command.eval_id, exception: String(error && error.stack || error)}});
    }}
  }};
  const connectBridgeSocket = function() {{
    try {{
      if (!window.WebSocket || bridgeSocketReconnectTimer || bridgeSocketReady) return;
      const protocol = location.protocol === "https:" ? "wss:" : "ws:";
      bridgeSocket = new WebSocket(protocol + "//" + bridgeHost + endpoint + "/ws");
      bridgeSocket.onopen = function() {{
        bridgeSocketReady = true;
        bridgeState = "connected";
        if (bridgeOutbox.length === 0 && lastHelloArgs) hello(lastHelloArgs.scope, lastHelloArgs.networkEvents);
        scheduleBridgeFlush(1);
      }};
      bridgeSocket.onmessage = function(event) {{
        try {{
          const message = JSON.parse(event.data || "{{}}");
          if (message.type === "ack" && message.seq) delete bridgeInflight[message.seq];
          if (message.type === "eval") handleEvalCommand(message.command);
          if (message.type === "overlay") handleOverlayCommand(message.command);
          if (message.type === "snapshot_request") {{
            hello(message.scope || "full", performanceNetwork());
          }}
        }} catch (_) {{}}
      }};
      bridgeSocket.onclose = function() {{
        bridgeSocketReady = false;
        bridgeSocket = null;
        bridgeState = "connecting";
        const retry = Object.keys(bridgeInflight).map(function(key) {{ return bridgeInflight[key]; }});
        bridgeInflight = Object.create(null);
        if (retry.length) bridgeOutbox = retry.concat(bridgeOutbox).slice(-1000);
        bridgeSocketReconnectTimer = window.setTimeout(function() {{
          bridgeSocketReconnectTimer = 0;
          connectBridgeSocket();
        }}, 2000);
      }};
      bridgeSocket.onerror = function() {{
        try {{ bridgeSocket.close(); }} catch (_) {{}}
      }};
    }} catch (_) {{
      bridgeSocketReconnectTimer = window.setTimeout(function() {{
        bridgeSocketReconnectTimer = 0;
        connectBridgeSocket();
      }}, 2000);
    }}
  }};
  const originalFetch = window.fetch;
  if (originalFetch) {{
	    window.fetch = function(input, init) {{
	      const url = typeof input === "string" ? input : (input && input.url) || "";
	      const method = (init && init.method) || (input && input.method) || "GET";
	      let clientReqId = null;
	      let nextInput = input;
	      let nextInit = init;
	      let requestHeaders = [];
	      try {{
	        if (typeof Request !== "undefined" && input instanceof Request) {{
	          requestHeaders = headerPairsFromHeaders(init && init.headers ? init.headers : input.headers);
	        }} else {{
	          requestHeaders = headerPairsFromHeaders(init && init.headers ? init.headers : undefined);
	        }}
	        if (canAttachClientReqHeader(url)) {{
	          clientReqId = nextClientReqId();
	          if (typeof Request !== "undefined" && input instanceof Request) {{
	            const headers = new Headers(init && init.headers ? init.headers : input.headers);
	            headers.set(clientReqHeader, clientReqId);
            nextInput = new Request(input, Object.assign({{}}, init || {{}}, {{headers: headers}}));
            nextInit = undefined;
          }} else {{
            const headers = new Headers(init && init.headers ? init.headers : undefined);
            headers.set(clientReqHeader, clientReqId);
            nextInit = Object.assign({{}}, init || {{}}, {{headers: headers}});
          }}
        }}
      }} catch (_) {{
        clientReqId = null;
        nextInput = input;
	        nextInit = init;
	      }}
	      return originalFetch.call(this, nextInput, nextInit).then(function(response) {{
	        try {{ recordNetwork({{url: response.url || url, method: method, status: response.status, type: "Fetch", client_req_id: clientReqId, query_params: queryPairs(response.url || url), request_headers: requestHeaders, response_headers: headerPairsFromHeaders(response.headers), from_cache: response.status === 304}}); }} catch (_) {{}}
	        return response;
	      }});
	    }};
	  }}
  const OriginalXHR = window.XMLHttpRequest;
  if (OriginalXHR) {{
    window.XMLHttpRequest = function() {{
      const xhr = new OriginalXHR();
	      let url = "";
	      let method = "GET";
	      let clientReqId = null;
	      const requestHeaders = [];
	      const open = xhr.open;
	      xhr.open = function(m, u) {{
	        method = m || "GET";
	        url = u || "";
	        return open.apply(xhr, arguments);
	      }};
	      const setRequestHeader = xhr.setRequestHeader;
	      xhr.setRequestHeader = function(name, value) {{
	        try {{ requestHeaders.push([String(name), String(value)]); }} catch (_) {{}}
	        return setRequestHeader.apply(xhr, arguments);
	      }};
	      const send = xhr.send;
	      xhr.send = function() {{
	        try {{
	          if (canAttachClientReqHeader(url)) {{
	            clientReqId = nextClientReqId();
            xhr.setRequestHeader(clientReqHeader, clientReqId);
          }}
        }} catch (_) {{
          clientReqId = null;
        }}
	        return send.apply(xhr, arguments);
	      }};
	      xhr.addEventListener("loadend", function() {{
	        try {{ recordNetwork({{url: xhr.responseURL || url, method: method, status: xhr.status, type: "XHR", client_req_id: clientReqId, query_params: queryPairs(xhr.responseURL || url), request_headers: requestHeaders, response_headers: xhrResponseHeaders(xhr), from_cache: xhr.status === 304}}); }} catch (_) {{}}
	      }});
      return xhr;
    }};
  }}
  try {{
    const observer = new MutationObserver(function(mutations) {{
      if (mutations.some(isExternalStructuralMutation)) scheduleDomRefresh(250);
    }});
    observer.observe(document.documentElement || document, {{
      childList: true,
      subtree: true
    }});
  }} catch (_) {{}}
  window.addEventListener("resize", updateHighlightOverlay, true);
  window.addEventListener("scroll", updateHighlightOverlay, true);
  if (document.readyState === "loading") {{
    document.addEventListener("DOMContentLoaded", function() {{
      lastStorageSnapshot = JSON.stringify(storageSnapshot());
      connectBridgeSocket();
      hello("full", performanceNetwork());
    }}, {{once: true}});
  }} else {{
    lastStorageSnapshot = JSON.stringify(storageSnapshot());
    connectBridgeSocket();
    hello("full", performanceNetwork());
  }}
}})();
</script>"##
    )
}

pub(super) fn maybe_inject_devtools_bridge_html(
    body: Bytes,
    content_type: &str,
    rules: &ResolvedRules,
    state: Option<&AdminState>,
    record_url: &str,
    traffic_id: &str,
) -> Bytes {
    if !content_type.to_ascii_lowercase().starts_with("text/html")
        || !devtools_bridge_requested(rules)
    {
        return body;
    }
    let Some(state) = state else {
        return body;
    };
    let Some(devtools_rule) = effective_devtools_rule(rules) else {
        return body;
    };

    let input = RegisterPageInput {
        url: record_url.to_string(),
        origin: origin_from_url(record_url),
        traffic_id: traffic_id.to_string(),
        mode: admin_devtools_mode(&devtools_rule),
        matched_rule: devtools_matched_rule(rules),
    };
    let (page_id, token) = state.devtools_broker.register_page_candidate(input);
    let script = devtools_bridge_script(&page_id, &token);
    let html = String::from_utf8_lossy(&body);
    Bytes::from(insert_devtools_bridge_script(&html, &script))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn test_bare_devtools_rule_defaults_to_control_bridge() {
        let rule = parse_devtools_rule_value("");
        assert_eq!(rule.mode, DevtoolsMode::Control);
        assert_eq!(rule.inject, DevtoolsInjectMode::Auto);
        assert!(!rule.deny);
    }

    #[test]
    fn test_devtools_client_request_id_header_is_internal_only() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            HeaderName::from_static(DEVTOOLS_CLIENT_REQ_ID_HEADER),
            HeaderValue::from_static("pg_1-req"),
        );
        headers.insert("x-user-header", HeaderValue::from_static("visible"));

        assert_eq!(
            take_devtools_client_req_id(&mut headers),
            Some("pg_1-req".to_string())
        );
        assert!(!headers.contains_key(DEVTOOLS_CLIENT_REQ_ID_HEADER));
        assert_eq!(
            super::super::headers_to_pairs(&headers),
            vec![("x-user-header".to_string(), "visible".to_string())]
        );
    }
}
