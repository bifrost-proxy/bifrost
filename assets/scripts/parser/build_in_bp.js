/*
 * Bifrost BP parser adapter for remote hosting.
 *
 *   bp://https://.../build_in_bp.js?sha256=<hex> decode://bp
 */
(function () {
  "use strict";

  var BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  var DEFAULT_BAM_SEARCH_URL_B64 = "aHR0cHM6Ly9ieXRlYXBpLmJ5dGVkLm9yZy9hcGkvZW5kcG9pbnQvcXVlcnk=";
  var DEFAULT_BAM_PARSE_URL_B64 = "aHR0cHM6Ly9tYWd3LmJ5dGVkLm9yZy9odHRwLXNlcnZpY2UvYmluYXJ5X3Rvb2xzL3BhcnNl";
  var DEFAULT_BIFROST_BAM_TOKEN_URL_B64 = "aHR0cHM6Ly9iaWZyb3N0LmJ5dGVkYW5jZS5uZXQvdjQvc3NvL2luZm8=";
  var DEFAULT_BAM_BASE_URL_B64 = "aHR0cHM6Ly9ieXRlYXBpLmJ5dGVkLm9yZw==";
  var BAM_SEARCH_URL = b64ToText(DEFAULT_BAM_SEARCH_URL_B64);
  var BAM_PARSE_URL = b64ToText(DEFAULT_BAM_PARSE_URL_B64);
  var BIFROST_BAM_TOKEN_URL = b64ToText(DEFAULT_BIFROST_BAM_TOKEN_URL_B64);

  function ok(data) {
    ctx.output = { code: "0", data: typeof data === "string" ? data : JSON.stringify(data), msg: "" };
  }

  function fail(error) {
    ctx.output = { code: "1", data: "", msg: String(error && error.message ? error.message : error) };
  }

  function trim(value) {
    return String(value == null ? "" : value).replace(/^\s+|\s+$/g, "");
  }

  function parseQuery(scriptName) {
    var text = String(scriptName || "");
    var queryStart = text.indexOf("?");
    if (queryStart < 0) {
      return {};
    }
    var queryEnd = text.indexOf("#", queryStart + 1);
    var query = text.slice(queryStart + 1, queryEnd >= 0 ? queryEnd : text.length);
    var out = {};
    query.split("&").forEach(function (part) {
      if (!part) return;
      var eq = part.indexOf("=");
      var key = eq >= 0 ? part.slice(0, eq) : part;
      var value = eq >= 0 ? part.slice(eq + 1) : "";
      out[decodeURIComponent(key)] = decodeURIComponent(value.replace(/\+/g, "%20"));
    });
    return out;
  }

  function obj2query(obj) {
    var result = [];
    Object.keys(obj || {}).forEach(function (key) {
      if (typeof obj[key] !== "undefined") {
        result.push(encodeURIComponent(key) + "=" + encodeURIComponent(String(obj[key] == null ? "" : obj[key])));
      }
    });
    return result.join("&");
  }

  function bodyBase64() {
    if (ctx.phase === "request" || ctx.phase === "websocket_send") {
      return request && request.bodyBase64 ? request.bodyBase64 : "";
    }
    return response && response.bodyBase64 ? response.bodyBase64 : "";
  }

  function currentRequest() {
    if (ctx.phase === "request" || ctx.phase === "websocket_send") return request || {};
    return response && response.request ? response.request : {};
  }

  function requestPattern(params) {
    if (params.pattern) return params.pattern;
    if (ctx.phase === "request" || ctx.phase === "websocket_send") {
      return request && request.path ? request.path : "/";
    }
    return response && response.request && response.request.path ? response.request.path : "/";
  }

  function ruleValue(params) {
    if (params.rule) return params.rule;
    if (params.psm) return params.psm;
    if (params.idl && (params.struct || params.message)) return params.idl + ":" + (params.struct || params.message);
    return "";
  }

  function boolParam(value, defaultValue) {
    if (typeof value === "undefined" || value === null || value === "") return defaultValue;
    var text = String(value).toLowerCase();
    return !(text === "0" || text === "false" || text === "no" || text === "off");
  }

  function readConfigFile(path) {
    try {
      if (!path || !file.exists(path)) return {};
      var text = trim(file.readText(path));
      if (!text) return {};
      try {
        return JSON.parse(text);
      } catch (_) {
        return { raw: text };
      }
    } catch (_) {
      return {};
    }
  }

  function bifrostBamTokenUrl(params) {
    if (params.bamAuthUrl) return params.bamAuthUrl;
    if (params.bifrostInfoUrl) return params.bifrostInfoUrl;
    if (params.bifrostBaseUrl) return trim(params.bifrostBaseUrl).replace(/\/+$/g, "") + "/v4/sso/info";
    return BIFROST_BAM_TOKEN_URL;
  }

  function fetchBamTokenWithSyncToken(syncToken, params) {
    if (!syncToken) return "";
    var resp = JSON.parse(net.fetch(bifrostBamTokenUrl(params), JSON.stringify({
      method: "GET",
      headers: {
        "x-bifrost-token": syncToken,
        "accept": "application/json",
      },
      timeoutMs: Number(params.bamAuthTimeoutMs || params.timeoutMs || 5000),
    })));
    var json = parseFetchBody(resp);
    ensureBamOk(json);
    var data = json && json.data ? json.data : {};
    var token = data.bam_token || data.bamToken || "";
    if (!token) throw new Error("Bifrost sync info response missing bam_token");
    return token;
  }

  function loadBamToken(params) {
    if (params.bamToken) return params.bamToken;

    var config = {};
    var tokenFile = params.bamTokenFile || params.syncTokenFile || "db/config.json";
    if (tokenFile) config = readConfigFile(tokenFile);

    var direct = config.bam_token || config.bamToken || config.raw;
    if (direct && String(direct).indexOf("ak=") >= 0) return direct;

    if (!boolParam(params.autoBamToken, true)) return direct || "";

    var syncToken = params.syncToken || params.bifrostToken || config.sync_token || config.syncToken || config.token;
    if (!syncToken && config.raw && !params.bamTokenFile) syncToken = config.raw;
    if (syncToken) return fetchBamTokenWithSyncToken(syncToken, params);

    return direct || "";
  }

  function parseFetchBody(resp) {
    var text = resp && resp.body ? resp.body : "";
    try {
      return JSON.parse(text);
    } catch (_) {
      if (!resp || !resp.ok) throw new Error(text || "request failed");
      return text;
    }
  }

  function ensureBamOk(json) {
    if (!json || typeof json !== "object") return;
    var status = typeof json.status_code !== "undefined" ? json.status_code : json.code;
    if (typeof status === "undefined" || status === null || status === "" || String(status) === "0") return;
    throw new Error(json.msg || json.message || ("BAM request failed: " + status));
  }

  function bamSearch(pattern, token, params) {
    var url = (params.bamSearchUrl || BAM_SEARCH_URL) + "?" + obj2query({ name: pattern });
    var resp = JSON.parse(net.fetch(url, JSON.stringify({
      method: "GET",
      headers: token ? { cookie: token } : {},
      timeoutMs: Number(params.timeoutMs || 5000),
    })));
    var json = parseFetchBody(resp);
    ensureBamOk(json);
    var data = json && json.data ? json.data : [];
    return data[0] && data[0].psm ? data[0].psm : "";
  }

  function parseByBam(rule, params, body) {
    var token = loadBamToken(params);
    var pattern = requestPattern(params);
    var psm = trim(rule || params.psm);
    if (!psm) psm = bamSearch(pattern, token, params);
    if (!psm) throw new Error("BAM psm is empty");
    var type = params.type || ctx.phase || "response";
    if (type === "websocket_send") type = "request";
    if (type === "websocket_recv") type = "response";
    var url = (params.bamParseUrl || BAM_PARSE_URL) + "?" + obj2query({ type: type, psm: psm, path: pattern });
    var resp = JSON.parse(net.fetch(url, JSON.stringify({
      method: "POST",
      headers: token ? { cookie: token } : {},
      bodyBase64: body,
      timeoutMs: Number(params.timeoutMs || 5000),
    })));
    var json = parseFetchBody(resp);
    ensureBamOk(json);
    return json && json.data ? json.data.parse_result : json;
  }

  function bamGetJson(url, token, params) {
    var resp = JSON.parse(net.fetch(url, JSON.stringify({
      method: "GET",
      headers: token ? { cookie: token, accept: "application/json" } : { accept: "application/json" },
      timeoutMs: Number(params.timeoutMs || 5000),
    })));
    var json = parseFetchBody(resp);
    ensureBamOk(json);
    return json;
  }

  function bamBase(params) {
    return trim(params.bamBaseUrl || b64ToText(DEFAULT_BAM_BASE_URL_B64)).replace(/\/+$/g, "");
  }

  function bamVersionQuery(params) {
    var out = { psm: params.psm };
    if (params.version) out.version = params.version;
    if (params.branch) out.branch = params.branch;
    return out;
  }

  function findEndpointInfo(token, params) {
    var base = bamBase(params);
    var endpointId = params.endpointId || params.endpoint_id;
    if (!endpointId) {
      var listQuery = bamVersionQuery(params);
      var listUrl = (params.bamEndpointListUrl || (base + "/api/endpoint/list")) + "?" + obj2query(listQuery);
      var listJson = bamGetJson(listUrl, token, params);
      var endpoints = listJson && listJson.data ? listJson.data : [];
      var method = trim(params.method || params.rpcMethod || params.rpc_method);
      var path = trim(params.path || params.pattern);
      for (var i = 0; i < endpoints.length; i++) {
        var ep = endpoints[i] || {};
        if ((method && ep.rpc_method === method) || (path && ep.path === path)) {
          endpointId = ep.endpoint_id;
          break;
        }
      }
    }
    if (!endpointId) throw new Error("BAM endpoint_id is empty; pass endpointId or method/path");
    var infoQuery = bamVersionQuery(params);
    infoQuery.endpoint_id = endpointId;
    infoQuery.schema = "ref";
    var infoUrl = (params.bamEndpointInfoUrl || (base + "/api/endpoint/info")) + "?" + obj2query(infoQuery);
    var infoJson = bamGetJson(infoUrl, token, params);
    if (!infoJson || !infoJson.data) throw new Error("BAM endpoint info response missing data");
    return infoJson.data;
  }

  function loadBamRefSchema(token, params) {
    if (params.refschemaFile) {
      var fileJson = JSON.parse(file.readText(params.refschemaFile));
      return fileJson.data || fileJson;
    }
    var query = bamVersionQuery(params);
    query.raw_field = params.rawField || params.raw_field || "1";
    var url = (params.bamRefSchemaUrl || (bamBase(params) + "/api/service/refschema")) + "?" + obj2query(query);
    var json = bamGetJson(url, token, params);
    if (!json || !json.data || !json.data.structs) throw new Error("BAM refschema response missing structs");
    return json.data;
  }

  function thriftPhaseType(params) {
    var value = params.schemaType || params.schema_type || params.type || ctx.phase || "request";
    if (value === "websocket_send") value = "request";
    if (value === "websocket_recv") value = "response";
    if (value === "req") value = "request";
    if (value === "resp") value = "response";
    return value;
  }

  function makeThriftRoot(endpointInfo, params, schemaType) {
    if (params.struct || params.message) {
      return { name: params.struct || params.message, unwrap: false };
    }
    var method = trim(params.method || params.rpcMethod || params.rpc_method || endpointInfo.rpc_method);
    if (!method) throw new Error("thrift parser requires method/rpc_method");
    if (schemaType === "response") {
      var resp = endpointInfo.resp_schema && (endpointInfo.resp_schema["200"] || endpointInfo.resp_schema[200]);
      var respType = resp && resp.resp_type;
      if (!respType) throw new Error("BAM endpoint info missing resp_type");
      return {
        name: "__bifrost." + method + "_result",
        unwrap: true,
        fieldName: "success",
        fieldId: 0,
        fieldType: respType,
      };
    }
    var reqType = endpointInfo.req_schema && endpointInfo.req_schema.req_type;
    if (!reqType) throw new Error("BAM endpoint info missing req_type");
    return {
      name: "__bifrost." + method + "_args",
      unwrap: true,
      fieldName: "req",
      fieldId: 1,
      fieldType: reqType,
    };
  }

  function endpointBodyChildren(endpointInfo, schemaType) {
    if (schemaType === "response") {
      var resp = endpointInfo.resp_schema && (endpointInfo.resp_schema["200"] || endpointInfo.resp_schema[200]);
      return resp && (resp.body_param || resp.body || resp.query_param) || null;
    }
    var req = endpointInfo.req_schema || {};
    return req.body_param || req.body || req.query_param || null;
  }

  function thriftSchemaMap(refschema, root, endpointInfo, schemaType) {
    var structs = refschema.structs || {};
    var endpointChildren = endpointBodyChildren(endpointInfo, schemaType);
    if (root.unwrap && root.fieldType && endpointChildren && !structs[root.fieldType]) {
      structs[root.fieldType] = {
        type: "object",
        children: endpointChildren,
      };
    }
    if (root.unwrap) {
      structs[root.name] = {
        type: "object",
        children: {},
      };
      structs[root.name].children[root.fieldName] = {
        type: "object",
        field_id: root.fieldId,
        raw_name: root.fieldName,
        raw_type: root.fieldType,
        ref_name: root.fieldType,
      };
    }
    return structs;
  }

  function ThriftReader(bytes) {
    this.bytes = bytes || [];
    this.pos = 0;
  }

  ThriftReader.prototype.left = function () {
    return this.bytes.length - this.pos;
  };
  ThriftReader.prototype.u8 = function () {
    if (this.pos >= this.bytes.length) throw new Error("unexpected eof");
    return this.bytes[this.pos++] & 255;
  };
  ThriftReader.prototype.i16 = function () {
    var n = (this.u8() << 8) | this.u8();
    return n & 0x8000 ? n - 0x10000 : n;
  };
  ThriftReader.prototype.i32 = function () {
    var n = ((this.u8() << 24) | (this.u8() << 16) | (this.u8() << 8) | this.u8()) | 0;
    return n;
  };
  ThriftReader.prototype.u32 = function () {
    return this.i32() >>> 0;
  };
  ThriftReader.prototype.i64 = function () {
    var hi = this.i32();
    var lo = this.u32();
    var sign = hi < 0 ? "-" : "";
    if (hi === 0) return String(lo);
    if (hi === -1 && lo !== 0) return String(lo - 0x100000000);
    return sign + "0x" + ((hi < 0 ? (~hi) >>> 0 : hi) >>> 0).toString(16) + ("00000000" + lo.toString(16)).slice(-8);
  };
  ThriftReader.prototype.bytesN = function (n) {
    if (n < 0 || this.pos + n > this.bytes.length) throw new Error("invalid length " + n);
    var out = this.bytes.slice(this.pos, this.pos + n);
    this.pos += n;
    return out;
  };
  ThriftReader.prototype.string = function () {
    return utf8(this.bytesN(this.i32()));
  };

  function typeName(field) {
    return field && (field.ref_name || field.raw_type || field.type);
  }

  function fieldById(structSchema, id) {
    var children = (structSchema && structSchema.children) || {};
    var keys = Object.keys(children);
    for (var i = 0; i < keys.length; i++) {
      var f = children[keys[i]];
      if (Number(f.field_id) === Number(id)) {
        f.__name = f.raw_name || keys[i];
        return f;
      }
    }
    return { __name: String(id), field_id: id };
  }

  function decodeThriftValue(reader, wireType, field, structs) {
    if (wireType === 2) return reader.u8() === 1;
    if (wireType === 3) return reader.u8();
    if (wireType === 6) return reader.i16();
    if (wireType === 8) return reader.i32();
    if (wireType === 10) return reader.i64();
    if (wireType === 11) return reader.string();
    if (wireType === 12) return decodeThriftStruct(reader, typeName(field), structs);
    if (wireType === 13) {
      var keyType = reader.u8();
      var valType = reader.u8();
      var size = reader.i32();
      var map = {};
      var valField = field && field.children ? field.children._val : null;
      for (var i = 0; i < size; i++) {
        var k = String(decodeThriftValue(reader, keyType, field && field.children ? field.children._key : null, structs));
        map[k] = decodeThriftValue(reader, valType, valField, structs);
      }
      return map;
    }
    if (wireType === 14 || wireType === 15) {
      var elemType = reader.u8();
      var count = reader.i32();
      var arr = [];
      var elemField = field && field.children ? field.children.elem || field.children._elem : null;
      for (var j = 0; j < count; j++) arr.push(decodeThriftValue(reader, elemType, elemField, structs));
      return arr;
    }
    throw new Error("unsupported thrift wire type " + wireType);
  }

  function skipThriftValue(reader, wireType) {
    decodeThriftValue(reader, wireType, null, {});
  }

  function decodeThriftStruct(reader, name, structs) {
    var schema = structs[name] || structs[String(name || "").replace(/^\.\//, "")] || { children: {} };
    var out = {};
    while (reader.left() > 0) {
      var wireType = reader.u8();
      if (wireType === 0) break;
      var id = reader.i16();
      var field = fieldById(schema, id);
      try {
        out[field.__name] = decodeThriftValue(reader, wireType, field, structs);
      } catch (e) {
        skipThriftValue(reader, wireType);
        out[field.__name] = { error: String(e && e.message ? e.message : e) };
      }
    }
    return out;
  }

  function locateThriftMessage(bytes, method) {
    for (var i = 0; i + 8 < bytes.length; i++) {
      if (bytes[i] === 0x80 && bytes[i + 1] === 0x01 && bytes[i + 2] === 0x00 && bytes[i + 3] >= 1 && bytes[i + 3] <= 4) {
        if (!method) return i;
        var r = new ThriftReader(bytes.slice(i));
        r.i32();
        var name = r.string();
        if (name === method) return i;
      }
    }
    return -1;
  }

  function parseThriftMessagePrefix(reader) {
    if (reader.left() < 4) return null;
    var pos = reader.pos;
    var versionType = reader.i32();
    if ((versionType >>> 16) !== 0x8001) {
      reader.pos = pos;
      return null;
    }
    var name = reader.string();
    var seqid = reader.i32();
    return { name: name, type: versionType & 0xff, seqid: seqid };
  }

  function parseByBamThrift(params, body) {
    var token = loadBamToken(params);
    if (!params.psm) throw new Error("thrift BAM parser requires psm");
    var endpointInfo = params.endpointInfoFile ? (JSON.parse(file.readText(params.endpointInfoFile)).data || JSON.parse(file.readText(params.endpointInfoFile))) : findEndpointInfo(token, params);
    var schemaType = thriftPhaseType(params);
    var root = makeThriftRoot(endpointInfo, params, schemaType);
    var structs = thriftSchemaMap(loadBamRefSchema(token, params), root, endpointInfo, schemaType);
    var bytes = b64ToBytes(body);
    var method = trim(params.method || params.rpcMethod || params.rpc_method || endpointInfo.rpc_method);
    var msgOffset = locateThriftMessage(bytes, method);
    if (msgOffset > 0) bytes = bytes.slice(msgOffset);
    var reader = new ThriftReader(bytes);
    var message = parseThriftMessagePrefix(reader);
    var data = decodeThriftStruct(reader, root.name, structs);
    if (root.unwrap && data[root.fieldName]) data = data[root.fieldName];
    return {
      protocol: "thrift-binary",
      psm: params.psm,
      method: method,
      schema_type: schemaType,
      message: message,
      data: data,
    };
  }

  function b64ToBytes(input) {
    var clean = String(input || "").replace(/[^A-Za-z0-9+/=]/g, "");
    var out = [];
    var buf = 0;
    var bits = 0;
    for (var i = 0; i < clean.length; i++) {
      var c = clean.charAt(i);
      if (c === "=") break;
      var v = BASE64.indexOf(c);
      if (v < 0) continue;
      buf = (buf << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out.push((buf >> bits) & 0xff);
      }
    }
    return out;
  }

  function b64ToText(input) {
    return utf8(b64ToBytes(input));
  }

  function utf8(bytes) {
    var s = "";
    for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    try {
      return decodeURIComponent(escape(s));
    } catch (_) {
      return s;
    }
  }

  function bodyText(body) {
    if (!body) return "";
    return utf8(b64ToBytes(body));
  }

  function tryJson(text) {
    var trimmed = trim(text);
    if (!trimmed) return {};
    try {
      return JSON.parse(trimmed);
    } catch (_) {
      return text;
    }
  }

  function parseHttpRpc(params, body) {
    var token = loadBamToken(params);
    var endpointInfo = null;
    try {
      endpointInfo = findEndpointInfo(token, params);
    } catch (error) {
      if (boolParam(params.requireEndpoint, true)) throw error;
    }

    var req = currentRequest();
    var phase = thriftPhaseType(params);
    var text = bodyText(body);
    var parsed = tryJson(text);
    if (typeof parsed === "string" && text) {
      try {
        var bamParsed = parseByBam(ruleValue(params), params, body);
        parsed = typeof bamParsed === "string" ? tryJson(bamParsed) : bamParsed;
      } catch (_) {
        parsed = { raw: text };
      }
    }

    return {
      protocol: "http-rpc",
      serializer: endpointInfo && (phase === "response" ? endpointInfo.resp_serializer : endpointInfo.serializer),
      psm: params.psm || "",
      method: (endpointInfo && (endpointInfo.rpc_method || endpointInfo.name)) || params.method || params.rpcMethod || "",
      endpoint_id: endpointInfo && endpointInfo.endpoint_id,
      endpoint_path: (endpointInfo && endpointInfo.path) || params.path || "",
      http: {
        method: req.method || "",
        host: req.host || "",
        path: req.path || "",
        url: req.url || "",
      },
      schema_type: phase,
      data: parsed,
    };
  }

  function stripComments(text) {
    return String(text || "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n\r]*/g, "");
  }

  function braceEnd(text, open) {
    var depth = 0;
    for (var i = open; i < text.length; i++) {
      if (text.charAt(i) === "{") depth++;
      if (text.charAt(i) === "}") {
        depth--;
        if (depth === 0) return i;
      }
    }
    return -1;
  }

  function removeNested(body) {
    var out = body;
    var re = /\b(message|enum)\s+[A-Za-z_]\w*\s*\{/;
    var m;
    while ((m = re.exec(out))) {
      var open = m.index + m[0].length - 1;
      var close = braceEnd(out, open);
      if (close < 0) break;
      out = out.slice(0, m.index) + out.slice(close + 1);
    }
    return out;
  }

  function parseEnum(body) {
    var values = {};
    var re = /([A-Za-z_]\w*)\s*=\s*(-?\d+)\s*(?:\[[^\]]*\])?\s*;/g;
    var m;
    while ((m = re.exec(body))) values[String(Number(m[2]))] = m[1];
    return values;
  }

  function parseFields(body) {
    var fields = {};
    var clean = removeNested(body);
    var mapRe = /\bmap\s*<\s*([A-Za-z_][\w.]*)\s*,\s*([A-Za-z_][\w.]*)\s*>\s+([A-Za-z_]\w*)\s*=\s*(\d+)(?:\s*\[[^\]]*\])?\s*;/g;
    var m;
    while ((m = mapRe.exec(clean))) {
      fields[Number(m[4])] = { name: m[3], type: "map", keyType: m[1], valueType: m[2], repeated: true, map: true };
    }
    var fieldRe = /\b(optional|required|repeated)?\s*([A-Za-z_][\w.]*)\s+([A-Za-z_]\w*)\s*=\s*(\d+)(?:\s*\[[^\]]*\])?\s*;/g;
    while ((m = fieldRe.exec(clean))) {
      fields[Number(m[4])] = { name: m[3], type: m[2], repeated: m[1] === "repeated", map: false };
    }
    return fields;
  }

  function parseScope(src, schema, prefix) {
    var i = 0;
    while (i < src.length) {
      var msg = /\bmessage\s+([A-Za-z_]\w*)\s*\{/.exec(src.slice(i));
      var en = /\benum\s+([A-Za-z_]\w*)\s*\{/.exec(src.slice(i));
      var item = null;
      var kind = "";
      if (msg && (!en || msg.index <= en.index)) {
        item = msg; kind = "message";
      } else if (en) {
        item = en; kind = "enum";
      } else {
        break;
      }
      var open = i + item.index + item[0].length - 1;
      var close = braceEnd(src, open);
      if (close < 0) break;
      var name = prefix ? prefix + "." + item[1] : item[1];
      var body = src.slice(open + 1, close);
      if (kind === "enum") {
        schema.enums[name] = parseEnum(body);
      } else {
        schema.messages[name] = parseFields(body);
        parseScope(body, schema, name);
      }
      i = close + 1;
    }
  }

  function parseProto(text) {
    var src = stripComments(text);
    var pkg = /\bpackage\s+([A-Za-z_][\w.]*)\s*;/.exec(src);
    var schema = { packageName: pkg ? pkg[1] : "", messages: {}, enums: {} };
    parseScope(src, schema, "");
    if (schema.packageName) {
      Object.keys(schema.messages).forEach(function (name) {
        if (name.indexOf(schema.packageName + ".") !== 0) schema.messages[schema.packageName + "." + name] = schema.messages[name];
      });
      Object.keys(schema.enums).forEach(function (name) {
        if (name.indexOf(schema.packageName + ".") !== 0) schema.enums[schema.packageName + "." + name] = schema.enums[name];
      });
    }
    return schema;
  }

  function Reader(bytes) {
    this.bytes = bytes || [];
    this.pos = 0;
    this.len = this.bytes.length;
  }

  Reader.prototype.eof = function () { return this.pos >= this.len; };
  Reader.prototype.byte = function () {
    if (this.pos >= this.len) throw new Error("protobuf read past end");
    return this.bytes[this.pos++] & 0xff;
  };
  Reader.prototype.bytesN = function (n) {
    var end = this.pos + n;
    if (end > this.len) throw new Error("protobuf length exceeds input");
    var out = this.bytes.slice(this.pos, end);
    this.pos = end;
    return out;
  };
  Reader.prototype.varint = function () {
    var shift = 0n;
    var result = 0n;
    for (;;) {
      var b = BigInt(this.byte());
      result |= (b & 0x7fn) << shift;
      if ((b & 0x80n) === 0n) return result;
      shift += 7n;
      if (shift > 70n) throw new Error("protobuf varint too long");
    }
  };
  Reader.prototype.u32 = function () {
    var b = this.bytesN(4);
    return (b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24)) >>> 0;
  };
  Reader.prototype.u64 = function () {
    var b = this.bytesN(8), out = 0n;
    for (var i = 7; i >= 0; i--) out = (out << 8n) | BigInt(b[i]);
    return out;
  };

  var SCALAR = {
    double: true, float: true, int32: true, uint32: true, sint32: true,
    fixed32: true, sfixed32: true, int64: true, uint64: true, sint64: true,
    fixed64: true, sfixed64: true, bool: true, string: true, bytes: true,
  };

  function signed32(v) {
    var n = Number(v & 0xffffffffn);
    return n > 0x7fffffff ? n - 0x100000000 : n;
  }
  function signed64(v) { return v > 0x7fffffffffffffffn ? v - 0x10000000000000000n : v; }
  function zig32(v) { var n = Number(v & 0xffffffffn); return (n >>> 1) ^ -(n & 1); }
  function zig64(v) { return (v >> 1n) ^ (-(v & 1n)); }
  function f32(u) { var b = new ArrayBuffer(4), v = new DataView(b); v.setUint32(0, u, true); return v.getFloat32(0, true); }
  function f64(u) {
    var b = new ArrayBuffer(8), v = new DataView(b);
    v.setUint32(0, Number(u & 0xffffffffn), true);
    v.setUint32(4, Number((u >> 32n) & 0xffffffffn), true);
    return v.getFloat64(0, true);
  }

  function packedWire(type) {
    if (type === "double" || type === "fixed64" || type === "sfixed64") return 1;
    if (type === "float" || type === "fixed32" || type === "sfixed32") return 5;
    return 0;
  }

  function scalar(reader, type, wire) {
    if (type === "double") return f64(reader.u64());
    if (type === "float") return f32(reader.u32());
    if (type === "fixed32" || type === "uint32") return wire === 5 ? reader.u32() : Number(reader.varint() & 0xffffffffn);
    if (type === "sfixed32" || type === "int32") return wire === 5 ? signed32(BigInt(reader.u32())) : signed32(reader.varint());
    if (type === "sint32") return zig32(reader.varint());
    if (type === "fixed64" || type === "uint64") return (wire === 1 ? reader.u64() : reader.varint()).toString();
    if (type === "sfixed64" || type === "int64") return signed64(wire === 1 ? reader.u64() : reader.varint()).toString();
    if (type === "sint64") return zig64(reader.varint()).toString();
    if (type === "bool") return reader.varint() !== 0n;
    if (type === "string") return utf8(reader.bytesN(Number(reader.varint())));
    if (type === "bytes") return reader.bytesN(Number(reader.varint()));
    throw new Error("unsupported scalar type: " + type);
  }

  function skip(reader, wire) {
    if (wire === 0) reader.varint();
    else if (wire === 1) reader.bytesN(8);
    else if (wire === 2) reader.bytesN(Number(reader.varint()));
    else if (wire === 5) reader.bytesN(4);
    else throw new Error("unsupported wire type: " + wire);
  }

  function resolve(schema, current, type) {
    if (SCALAR[type]) return { kind: "scalar", name: type };
    var names = [type.replace(/^\./, "")];
    if (current.indexOf(".") >= 0) {
      var parts = current.split(".");
      while (parts.length > 0) {
        parts.pop();
        names.push(parts.concat([type]).join("."));
      }
    }
    if (schema.packageName) names.push(schema.packageName + "." + type.replace(/^\./, ""));
    for (var i = 0; i < names.length; i++) {
      if (schema.messages[names[i]]) return { kind: "message", name: names[i] };
      if (schema.enums[names[i]]) return { kind: "enum", name: names[i] };
    }
    return { kind: "unknown", name: type };
  }

  function add(out, field, value) {
    if (field.repeated) {
      if (!out[field.name]) out[field.name] = [];
      out[field.name].push(value);
    } else {
      out[field.name] = value;
    }
  }

  function decodeMessage(schema, name, bytes) {
    var fields = schema.messages[name];
    if (!fields) throw new Error("message not found in proto: " + name);
    var reader = new Reader(bytes);
    var out = {};
    while (!reader.eof()) {
      var tag = Number(reader.varint());
      var fieldNo = tag >>> 3;
      var wire = tag & 7;
      var field = fields[fieldNo];
      if (!field) { skip(reader, wire); continue; }
      var type = resolve(schema, name, field.type);
      if (wire === 2 && field.repeated && type.kind === "scalar" && field.type !== "string" && field.type !== "bytes") {
        var packed = new Reader(reader.bytesN(Number(reader.varint())));
        while (!packed.eof()) add(out, field, scalar(packed, field.type, packedWire(field.type)));
        continue;
      }
      if (type.kind === "message") {
        add(out, field, decodeMessage(schema, type.name, reader.bytesN(Number(reader.varint()))));
      } else if (type.kind === "enum") {
        var n = Number(reader.varint());
        add(out, field, schema.enums[type.name][String(n)] || n);
      } else if (type.kind === "scalar") {
        add(out, field, scalar(reader, field.type, wire));
      } else {
        skip(reader, wire);
      }
    }
    return out;
  }

  function readProto(path) {
    if (path.indexOf("file://") === 0) return file.readText(decodeURIComponent(path.slice(7)));
    return file.readText(path);
  }

  function parseByProto(rule, params, body) {
    var pos = rule.indexOf(":");
    var idl = params.idl || rule.slice(0, pos);
    var struct = params.struct || params.message || rule.slice(pos + 1);
    var schema = parseProto(readProto(idl));
    var message = struct.replace(/^\./, "");
    if (!schema.messages[message] && schema.packageName && schema.messages[schema.packageName + "." + message]) {
      message = schema.packageName + "." + message;
    }
    return JSON.stringify(decodeMessage(schema, message, b64ToBytes(body)));
  }

  try {
    var params = parseQuery(ctx.scriptName);
    var rule = ruleValue(params);
    if (!rule) throw new Error("missing rule/psm or idl+struct parameters");
    var body = bodyBase64();
    var protocol = String(params.protocol || params.format || "").toLowerCase();
    if (protocol === "thrift" || protocol === "thrift-binary" || protocol === "kitex") {
      ok(parseByBamThrift(params, body));
    } else if (protocol === "http-rpc" || protocol === "httprpc" || protocol === "rpc-http") {
      ok(parseHttpRpc(params, body));
    } else if (rule.indexOf(":") >= 0 || (params.idl && (params.struct || params.message))) {
      ok(parseByProto(rule, params, body));
    } else {
      ok(parseByBam(rule, params, body));
    }
  } catch (error) {
    fail(error);
  }
})();
