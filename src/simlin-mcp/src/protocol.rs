// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP JSON-RPC 2.0 protocol types and stdio server.
//!
//! Implements the Model Context Protocol over newline-delimited JSON on
//! stdin/stdout, following the same wire format as go-claudecode/mcp.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::Registry;
use crate::transport::Transport;

// ── JSON-RPC error codes ─────────────────────────────────────────────

pub const ERR_PARSE: i64 = -32700;
pub const ERR_INVALID_REQUEST: i64 = -32600;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
#[allow(dead_code)] // standard JSON-RPC error code, used by future tool error paths
pub const ERR_INTERNAL: i64 = -32603;

/// MCP protocol version we advertise.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

// ── JSON-RPC types ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct Request {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
}

#[derive(Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

fn result_response(id: Value, result: Value) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Response {
    Response {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.to_string(),
            data,
        }),
    }
}

// ── MCP protocol types ───────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: &'static str,
    server_info: ServerInfo,
    capabilities: ServerCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(Serialize)]
struct ServerCapabilities {
    tools: ToolsCapability,
    resources: ResourcesCapability,
}

#[derive(Serialize)]
struct ToolsCapability {}

#[derive(Serialize)]
struct ResourcesCapability {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListToolsResult {
    tools: Vec<ToolDefinition>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CallToolResult {
    content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_content: Option<Value>,
    is_error: bool,
}

#[derive(Serialize)]
struct ContentBlock {
    r#type: &'static str,
    text: String,
}

#[derive(Serialize)]
struct ListResourcesResult {
    resources: Vec<ResourceMetadata>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceMetadata {
    uri: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
}

#[derive(Serialize)]
struct ReadResourceResult {
    contents: Vec<ResourceContent>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceContent {
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    text: String,
}

// ── Server ───────────────────────────────────────────────────────────

pub struct ServerConfig {
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
}

/// Serve MCP requests, reading newline-delimited JSON-RPC from `input`
/// and writing responses to `output`.
///
/// Used by the synchronous test suite; production code uses `serve_async`
/// with a `Transport` implementation.
#[cfg(test)]
pub fn serve(
    config: &ServerConfig,
    registry: &Registry,
    input: &mut dyn std::io::BufRead,
    output: &mut dyn std::io::Write,
) -> anyhow::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = input.read_line(&mut line)?;
        if n == 0 {
            // EOF
            return Ok(());
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = error_response(
                    Value::Null,
                    ERR_PARSE,
                    "parse error",
                    Some(Value::String(e.to_string())),
                );
                write_response(output, &resp)?;
                continue;
            }
        };

        // Validate JSON-RPC 2.0
        let valid_jsonrpc = req.jsonrpc.as_deref().is_some_and(|v| v == "2.0");
        let has_method = req.method.as_ref().is_some_and(|m| !m.is_empty());

        if !valid_jsonrpc || !has_method {
            let id = req.id.unwrap_or(Value::Null);
            let resp = error_response(id, ERR_INVALID_REQUEST, "invalid request", None);
            write_response(output, &resp)?;
            continue;
        }

        // Notifications (no id) are fire-and-forget
        let is_notification = req.id.is_none();
        if is_notification {
            // We handle notifications/initialized silently; ignore others.
            continue;
        }

        let id = req.id.unwrap_or(Value::Null);
        let method = req.method.as_deref().unwrap_or("");
        let resp = dispatch(config, registry, id.clone(), method, req.params);
        write_response(output, &resp)?;
    }
}

/// Serve MCP requests asynchronously, reading from and writing to `transport`.
///
/// Returns `Ok(())` when the transport signals EOF (recv returns None).
/// Each received line is parsed as JSON-RPC and dispatched synchronously;
/// the response is sent back through the transport.
pub async fn serve_async(
    transport: &mut impl Transport,
    config: &ServerConfig,
    registry: &Registry,
) -> anyhow::Result<()> {
    while let Some(line) = transport.recv().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = error_response(
                    Value::Null,
                    ERR_PARSE,
                    "parse error",
                    Some(Value::String(e.to_string())),
                );
                transport.send(serde_json::to_string(&resp)?).await?;
                continue;
            }
        };

        let valid_jsonrpc = req.jsonrpc.as_deref().is_some_and(|v| v == "2.0");
        let has_method = req.method.as_ref().is_some_and(|m| !m.is_empty());

        if !valid_jsonrpc || !has_method {
            let id = req.id.unwrap_or(Value::Null);
            let resp = error_response(id, ERR_INVALID_REQUEST, "invalid request", None);
            transport.send(serde_json::to_string(&resp)?).await?;
            continue;
        }

        // Notifications (no id) are fire-and-forget
        if req.id.is_none() {
            continue;
        }

        let id = req.id.unwrap_or(Value::Null);
        let method = req.method.as_deref().unwrap_or("");
        let resp = dispatch(config, registry, id, method, req.params);
        transport.send(serde_json::to_string(&resp)?).await?;
    }
    Ok(())
}

fn dispatch(
    config: &ServerConfig,
    registry: &Registry,
    id: Value,
    method: &str,
    params: Option<Value>,
) -> Response {
    match method {
        "initialize" => handle_initialize(config, id, params),
        "ping" => result_response(id, serde_json::json!({})),
        "tools/list" => handle_list_tools(registry, id),
        "tools/call" => handle_call_tool(registry, id, params),
        "resources/list" => handle_resources_list(id),
        "resources/read" => handle_resources_read(id, params),
        _ => error_response(id, ERR_METHOD_NOT_FOUND, "method not found", None),
    }
}

fn handle_initialize(config: &ServerConfig, id: Value, params: Option<Value>) -> Response {
    // Validate required params
    if params.is_none() {
        return error_response(id, ERR_INVALID_PARAMS, "missing params", None);
    }

    let result = InitializeResult {
        protocol_version: PROTOCOL_VERSION,
        server_info: ServerInfo {
            name: config.name.clone(),
            version: config.version.clone(),
        },
        capabilities: ServerCapabilities {
            tools: ToolsCapability {},
            resources: ResourcesCapability {},
        },
        instructions: config.instructions.clone(),
    };

    result_response(id, serde_json::to_value(result).unwrap())
}

fn handle_list_tools(registry: &Registry, id: Value) -> Response {
    let result = ListToolsResult {
        tools: registry.definitions(),
    };
    result_response(id, serde_json::to_value(result).unwrap())
}

fn handle_call_tool(registry: &Registry, id: Value, params: Option<Value>) -> Response {
    let params = match params {
        Some(p) => p,
        None => return error_response(id, ERR_INVALID_PARAMS, "missing params", None),
    };

    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return error_response(id, ERR_INVALID_PARAMS, "missing tool name", None),
    };

    let tool = match registry.get(name) {
        Some(t) => t,
        None => return error_response(id, ERR_INVALID_PARAMS, "tool not found", None),
    };

    // Normalize arguments: null or missing → {}
    let arguments = match params.get("arguments") {
        Some(Value::Null) | None => Value::Object(serde_json::Map::new()),
        Some(v) => v.clone(),
    };

    match tool.call(arguments) {
        Ok(result_value) => {
            let text = serde_json::to_string(&result_value).unwrap_or_default();
            let call_result = CallToolResult {
                content: vec![ContentBlock {
                    r#type: "text",
                    text,
                }],
                structured_content: Some(result_value),
                is_error: false,
            };
            result_response(id, serde_json::to_value(call_result).unwrap())
        }
        Err(e) => {
            let error_text = format!("{e}");
            // When a handler bails with a JSON-serialized message (e.g. from
            // EditModel's error gate), preserve the structure so LLM clients
            // can programmatically inspect error details.
            let structured_content = serde_json::from_str::<serde_json::Value>(&error_text)
                .unwrap_or_else(|_| serde_json::json!({ "error": &error_text }));
            let call_result = CallToolResult {
                content: vec![ContentBlock {
                    r#type: "text",
                    text: error_text,
                }],
                structured_content: Some(structured_content),
                is_error: true,
            };
            result_response(id, serde_json::to_value(call_result).unwrap())
        }
    }
}

// ── Resource handlers ───────────────────────────────────────────────

const ERR_RESOURCE_NOT_FOUND: i64 = -32002;

fn handle_resources_list(id: Value) -> Response {
    let resources: Vec<ResourceMetadata> = crate::resource::list()
        .iter()
        .map(|entry| ResourceMetadata {
            uri: entry.metadata.uri.to_string(),
            name: entry.metadata.name.to_string(),
            description: entry.metadata.description.map(String::from),
            mime_type: entry.metadata.mime_type.map(String::from),
        })
        .collect();
    let result = ListResourcesResult { resources };
    result_response(id, serde_json::to_value(result).unwrap())
}

fn handle_resources_read(id: Value, params: Option<Value>) -> Response {
    let params = match params {
        Some(p) => p,
        None => return error_response(id, ERR_INVALID_PARAMS, "missing params", None),
    };
    let uri = match params.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return error_response(id, ERR_INVALID_PARAMS, "missing uri parameter", None),
    };

    match crate::resource::get(uri) {
        Some(entry) => {
            let result = ReadResourceResult {
                contents: vec![ResourceContent {
                    uri: entry.metadata.uri.to_string(),
                    mime_type: entry.metadata.mime_type.map(String::from),
                    text: entry.content.to_string(),
                }],
            };
            result_response(id, serde_json::to_value(result).unwrap())
        }
        None => error_response(
            id,
            ERR_RESOURCE_NOT_FOUND,
            &format!("resource not found: {uri}"),
            None,
        ),
    }
}

#[cfg(test)]
fn write_response(output: &mut dyn std::io::Write, resp: &Response) -> std::io::Result<()> {
    serde_json::to_writer(&mut *output, resp)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Registry, Tool};
    use crate::transport::Transport;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes input"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "msg": { "type": "string" }
                }
            })
        }
        fn call(&self, input: Value) -> anyhow::Result<Value> {
            Ok(input)
        }
    }

    fn test_config() -> ServerConfig {
        ServerConfig {
            name: "test-server".to_string(),
            version: "0.1.0".to_string(),
            instructions: None,
        }
    }

    fn roundtrip(registry: &Registry, config: &ServerConfig, request: &str) -> Value {
        let mut input = std::io::Cursor::new(format!("{request}\n"));
        let mut output = Vec::new();
        serve(config, registry, &mut input, &mut output).unwrap();
        let response_str = String::from_utf8(output).unwrap();
        serde_json::from_str(response_str.trim()).unwrap()
    }

    #[test]
    fn test_initialize() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
        );

        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], "test-server");
    }

    #[test]
    fn test_ping() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        );

        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[test]
    fn test_list_tools() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        );

        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["description"], "echoes input");
    }

    #[test]
    fn test_call_tool() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"msg":"hello"}}}"#,
        );

        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(
            resp["result"]["structuredContent"],
            serde_json::json!({"msg": "hello"})
        );
    }

    #[test]
    fn test_call_unknown_tool() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nonexistent"}}"#,
        );

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_INVALID_PARAMS);
    }

    #[test]
    fn test_null_arguments_normalized() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"echo","arguments":null}}"#,
        );

        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(resp["result"]["structuredContent"], serde_json::json!({}));
    }

    #[test]
    fn test_invalid_json() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(&registry, &config, r#"not valid json"#);

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_PARSE);
    }

    #[test]
    fn test_missing_jsonrpc_version() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(&registry, &config, r#"{"id":1,"method":"ping"}"#);

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_INVALID_REQUEST);
    }

    #[test]
    fn test_notification_ignored() {
        let registry = Registry::new();
        let config = test_config();

        // Notification (no id) followed by a real request
        let input = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":1,"method":"ping"}
"#;
        let mut cursor = std::io::Cursor::new(input);
        let mut output = Vec::new();
        serve(&config, &registry, &mut cursor, &mut output).unwrap();
        let response_str = String::from_utf8(output).unwrap();
        // Should only get one response (for the ping)
        let lines: Vec<&str> = response_str.trim().lines().collect();
        assert_eq!(lines.len(), 1);
        let resp: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[test]
    fn test_full_lifecycle() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"msg":"hello"}}}"#,
        ]
        .join("\n")
            + "\n";

        let mut cursor = std::io::Cursor::new(input);
        let mut output = Vec::new();
        serve(&config, &registry, &mut cursor, &mut output).unwrap();

        let response_str = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = response_str.trim().lines().collect();
        // 4 responses: initialize, ping, tools/list, tools/call
        // (notification produces no response)
        assert_eq!(lines.len(), 4);

        let r1: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(r1["id"], 1);
        assert!(r1["result"]["protocolVersion"].is_string());

        let r2: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r2["id"], 2);

        let r3: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(r3["id"], 3);
        assert!(r3["result"]["tools"].is_array());

        let r4: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(r4["id"], 4);
        assert_eq!(r4["result"]["isError"], false);
    }

    // ── async tests ──────────────────────────────────────────────────────

    /// In-memory transport for testing serve_async without real stdio.
    struct MockTransport {
        incoming: VecDeque<String>,
        outgoing: RefCell<Vec<String>>,
    }

    impl MockTransport {
        fn new(messages: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                incoming: messages.into_iter().map(str::to_string).collect(),
                outgoing: RefCell::new(Vec::new()),
            }
        }

        fn responses(&self) -> Vec<Value> {
            self.outgoing
                .borrow()
                .iter()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect()
        }
    }

    impl Transport for MockTransport {
        async fn recv(&mut self) -> Option<String> {
            self.incoming.pop_front()
        }

        async fn send(&self, message: String) -> anyhow::Result<()> {
            self.outgoing.borrow_mut().push(message);
            Ok(())
        }
    }

    fn async_roundtrip(
        registry: &Registry,
        config: &ServerConfig,
        requests: &[&'static str],
    ) -> Vec<Value> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut transport = MockTransport::new(requests.iter().copied());
        rt.block_on(serve_async(&mut transport, config, registry))
            .unwrap();
        transport.responses()
    }

    // simlin-mcp.AC1.1: initialize handshake followed by ping
    #[test]
    fn test_async_initialize_and_ping() {
        let registry = Registry::new();
        let config = test_config();

        let responses = async_roundtrip(
            &registry,
            &config,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            ],
        );

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], serde_json::json!({}));
    }

    // simlin-mcp.AC1.2: three or more sequential requests all succeed
    #[test]
    fn test_async_sequential_requests() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let responses = async_roundtrip(
            &registry,
            &config,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"msg":"world"}}}"#,
            ],
        );

        // notification produces no response, so 4 responses for 5 inputs
        assert_eq!(responses.len(), 4);
        assert_eq!(responses[0]["id"], 1);
        assert!(responses[1]["result"]["tools"].is_array());
        assert_eq!(responses[2]["result"], serde_json::json!({}));
        assert_eq!(responses[3]["result"]["isError"], false);
    }

    // simlin-mcp.AC1.3: EOF (None from recv) causes clean shutdown after processing requests
    #[tokio::test]
    async fn test_async_eof_clean_shutdown() {
        let mut registry = Registry::new();
        registry.register(Box::new(EchoTool));
        let config = test_config();

        let mut transport = MockTransport::new([
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
        ]);

        // serve_async must return Ok when the transport signals EOF, even after
        // processing one or more requests. Wrapping in a timeout catches hangs.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            serve_async(&mut transport, &config, &registry),
        )
        .await;

        assert!(
            result.is_ok(),
            "serve_async hung instead of returning on EOF"
        );
        assert!(
            result.unwrap().is_ok(),
            "serve_async returned an error on clean EOF"
        );

        let responses = transport.responses();
        // The initialize request should have received a response before EOF.
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    struct FailTool;

    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "always fails"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn call(&self, _input: Value) -> anyhow::Result<Value> {
            Err(anyhow::anyhow!("deliberate failure"))
        }
    }

    // simlin-mcp.AC4.1: tool execution error returns isError:true with error text in content
    #[tokio::test]
    async fn test_async_tool_error_returns_is_error_true() {
        let mut registry = Registry::new();
        registry.register(Box::new(FailTool));
        let config = test_config();

        let mut transport = MockTransport::new([
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fail"}}"#,
        ]);
        serve_async(&mut transport, &config, &registry)
            .await
            .unwrap();
        let responses = transport.responses();

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["result"]["isError"], true);
        let content_text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(
            content_text.contains("deliberate failure"),
            "expected 'deliberate failure' in content text, got: {content_text}"
        );
    }

    // simlin-mcp.AC4.2: malformed JSON returns -32700 parse error
    #[tokio::test]
    async fn test_async_malformed_json_returns_parse_error() {
        let registry = Registry::new();
        let config = test_config();

        let mut transport = MockTransport::new(["not valid json {"]);
        serve_async(&mut transport, &config, &registry)
            .await
            .unwrap();
        let responses = transport.responses();

        assert_eq!(responses.len(), 1);
        assert!(responses[0]["error"].is_object());
        assert_eq!(responses[0]["error"]["code"], ERR_PARSE);
    }

    // simlin-mcp.AC4.2: request missing jsonrpc field returns -32600 invalid request
    #[tokio::test]
    async fn test_async_missing_jsonrpc_returns_invalid_request() {
        let registry = Registry::new();
        let config = test_config();

        let mut transport = MockTransport::new([r#"{"id":1,"method":"ping"}"#]);
        serve_async(&mut transport, &config, &registry)
            .await
            .unwrap();
        let responses = transport.responses();

        assert_eq!(responses.len(), 1);
        assert!(responses[0]["error"].is_object());
        assert_eq!(responses[0]["error"]["code"], ERR_INVALID_REQUEST);
    }

    // simlin-mcp.AC4.3: unknown method returns -32601 method not found
    #[tokio::test]
    async fn test_async_unknown_method_returns_method_not_found() {
        let registry = Registry::new();
        let config = test_config();

        let mut transport =
            MockTransport::new([r#"{"jsonrpc":"2.0","id":1,"method":"nonexistent"}"#]);
        serve_async(&mut transport, &config, &registry)
            .await
            .unwrap();
        let responses = transport.responses();

        assert_eq!(responses.len(), 1);
        assert!(responses[0]["error"].is_object());
        assert_eq!(responses[0]["error"]["code"], ERR_METHOD_NOT_FOUND);
    }

    // mcp-publish-ready.AC3.5: JSON-serialized tool errors preserve structure in structured_content
    struct JsonErrorTool;

    impl Tool for JsonErrorTool {
        fn name(&self) -> &str {
            "json_error"
        }
        fn description(&self) -> &str {
            "returns a JSON-structured error"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        fn call(&self, _input: Value) -> anyhow::Result<Value> {
            let err_json = serde_json::json!({
                "error": "edit introduces compilation errors",
                "errors": [{ "code": "unknown_dependency", "kind": "variable" }]
            });
            Err(anyhow::anyhow!(
                "{}",
                serde_json::to_string(&err_json).unwrap()
            ))
        }
    }

    #[test]
    fn test_json_error_preserves_structured_content() {
        let mut registry = Registry::new();
        registry.register(Box::new(JsonErrorTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"json_error"}}"#,
        );

        assert_eq!(resp["result"]["isError"], true);
        let structured = &resp["result"]["structuredContent"];
        assert_eq!(structured["error"], "edit introduces compilation errors");
        let errors = structured["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"], "unknown_dependency");
        assert_eq!(errors[0]["kind"], "variable");
    }

    #[test]
    fn test_plain_error_wraps_in_error_key() {
        let mut registry = Registry::new();
        registry.register(Box::new(FailTool));
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fail"}}"#,
        );

        assert_eq!(resp["result"]["isError"], true);
        let structured = &resp["result"]["structuredContent"];
        assert_eq!(structured["error"], "deliberate failure");
    }

    // ── Resource protocol tests ─────────────────────────────────────────

    // mcp-publish-ready.AC5: initialize response includes resources capability
    #[test]
    fn test_initialize_includes_resources_capability() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}"#,
        );

        assert!(
            resp["result"]["capabilities"]["resources"].is_object(),
            "expected capabilities.resources to be an object, got: {}",
            resp["result"]["capabilities"]
        );
    }

    // mcp-publish-ready.AC5.1: resources/list returns metadata for all four skills
    #[test]
    fn test_resources_list() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
        );

        let resources = resp["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 4);

        let uris: Vec<&str> = resources
            .iter()
            .map(|r| r["uri"].as_str().unwrap())
            .collect();
        assert!(uris.contains(&"simlin://skills/pysimlin-basics"));
        assert!(uris.contains(&"simlin://skills/scenario-analysis"));
        assert!(uris.contains(&"simlin://skills/loop-dominance"));
        assert!(uris.contains(&"simlin://skills/vensim-equation-syntax"));

        for resource in resources {
            assert!(resource["name"].is_string());
            assert!(resource["description"].is_string());
            assert_eq!(resource["mimeType"], "text/markdown");
        }
    }

    // mcp-publish-ready.AC5.2: resources/read with valid URI returns content
    #[test]
    fn test_resources_read_valid_uri() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"simlin://skills/pysimlin-basics"}}"#,
        );

        let contents = resp["result"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["uri"], "simlin://skills/pysimlin-basics");
        assert_eq!(contents[0]["mimeType"], "text/markdown");
        let text = contents[0]["text"].as_str().unwrap();
        assert!(!text.is_empty(), "resource content should not be empty");
    }

    // mcp-publish-ready.AC5.3: resources/read with unknown URI returns error -32002
    #[test]
    fn test_resources_read_unknown_uri() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"simlin://skills/nonexistent"}}"#,
        );

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_RESOURCE_NOT_FOUND);
        let msg = resp["error"]["message"].as_str().unwrap();
        assert!(
            msg.contains("nonexistent"),
            "error message should mention the URI"
        );
    }

    // mcp-publish-ready.AC5.3: resources/read with missing params returns -32602
    #[test]
    fn test_resources_read_missing_params() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/read"}"#,
        );

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_INVALID_PARAMS);
    }

    // mcp-publish-ready.AC5.3: resources/read with missing uri field returns -32602
    #[test]
    fn test_resources_read_missing_uri_field() {
        let registry = Registry::new();
        let config = test_config();

        let resp = roundtrip(
            &registry,
            &config,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{}}"#,
        );

        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], ERR_INVALID_PARAMS);
    }
}
