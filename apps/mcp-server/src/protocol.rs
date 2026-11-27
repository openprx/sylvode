use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 Error
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: msg.into(),
            data: None,
        }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: msg.into(),
            data: None,
        }
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: msg.into(),
            data: None,
        }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
            data: None,
        }
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
            data: None,
        }
    }

    /// `-32020`: a modern HTTP request whose `Mcp-*` header is missing or disagrees with the body.
    pub fn header_mismatch(header: &str, detail: &str) -> Self {
        Self {
            code: -32020,
            message: "Header mismatch".to_string(),
            data: Some(json!({ "header": header, "detail": detail })),
        }
    }

    /// `-32021`: reserved for a missing client capability; nothing produces it yet.
    pub fn missing_client_capability(name: &str) -> Self {
        Self {
            code: -32021,
            message: "Missing client capability".to_string(),
            data: Some(json!({ "capability": name })),
        }
    }

    /// `-32022`: the protocol version a modern request names is not served.
    pub fn unsupported_protocol_version(requested: &str) -> Self {
        Self {
            code: -32022,
            message: "Unsupported protocol version".to_string(),
            data: Some(json!({
                "supported": crate::era::UNSUPPORTED_SUPPORTED_LIST,
                "requested": requested,
            })),
        }
    }

    /// `-32002`: the legacy code for a resource that does not exist. `uri` is the requested text.
    pub fn resource_not_found_legacy(uri: &str) -> Self {
        Self {
            code: -32002,
            message: "Resource not found".to_string(),
            data: Some(json!({ "uri": uri })),
        }
    }

    /// `-32602`: the modern answer for a resource that does not exist. `uri` is the requested text.
    pub fn resource_not_found(uri: &str) -> Self {
        Self {
            code: -32602,
            message: "Resource not found".to_string(),
            data: Some(json!({ "uri": uri })),
        }
    }

    /// `-32602`: the caller may not read the resource. `reason` is one of `policy`, `forbidden`,
    /// `not_available_to_connected_applications`.
    pub fn resource_refused(uri: &str, reason: &str) -> Self {
        Self {
            code: -32602,
            message: "Resource access refused".to_string(),
            data: Some(json!({ "uri": uri, "reason": reason })),
        }
    }

    /// `-32000`: the caller holds too many listen streams.
    pub fn too_many_streams(limit: u32) -> Self {
        Self {
            code: -32000,
            message: "Too many listen streams".to_string(),
            data: Some(json!({ "limit": limit })),
        }
    }
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// MCP Tool Definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// MCP tool annotations; the full listing sets `readOnlyHint` from the tool catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
}

/// MCP List Tools Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<ToolDefinition>,
}

/// MCP Call Tool Request Parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    pub arguments: Option<Value>,
}

/// MCP Call Tool Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    /// MCP clients read the camelCase `isError` flag; without the rename standard
    /// clients treat every failed tool call as a success.
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// The same JSON the first text block holds; set for modern-era callers only.
    #[serde(rename = "structuredContent", default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource { uri: String },
}

impl ToolContent {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self::Text {
            text: format!("Error: {}", msg.into()),
        }
    }
}

impl CallToolResult {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: None,
            structured_content: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::error(msg)],
            is_error: Some(true),
            structured_content: None,
        }
    }

    /// A Flow-style typed business error: `isError:true` and a first text content that is
    /// pure UTF-8 JSON shaped `{"error":{"code","message","recoverable","details"}}`
    /// (`error-mapping-v1.md`: "Flow 业务错误返回 result.isError=true，首个 text content 是
    /// UTF-8 JSON"). Deliberately distinct from [`Self::error`], whose `"Error: "` prefixed
    /// plain text every one of this server's pre-Flow tools already returns and whose
    /// callers/tests depend on: this constructor never prefixes or otherwise mangles `body`,
    /// so a client that parses the JSON gets exactly the stable `code` the contract promises.
    pub fn business_error(code: &str, message: impl Into<String>, recoverable: bool, details: &Value) -> Self {
        let body = json!({
            "error": {
                "code": code,
                "message": message.into(),
                "recoverable": recoverable,
                "details": details,
            }
        });
        Self {
            content: vec![ToolContent::Text { text: body.to_string() }],
            is_error: Some(true),
            structured_content: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CallToolResult, ToolDefinition};
    use serde_json::{Value, json};

    #[test]
    fn call_tool_error_serializes_mcp_is_error_field() {
        let value = serde_json::to_value(CallToolResult::error("boom")).unwrap_or(Value::Null);

        assert_eq!(value.get("isError").and_then(Value::as_bool), Some(true));
        assert!(
            value.get("is_error").is_none(),
            "snake_case is_error is not part of the MCP schema"
        );
    }

    #[test]
    fn call_tool_success_omits_is_error_field() {
        let value = serde_json::to_value(CallToolResult::success("ok")).unwrap_or(Value::Null);

        assert!(value.get("isError").is_none());
        assert!(value.get("is_error").is_none());
    }

    #[test]
    fn tool_definition_serializes_camel_case_input_schema() {
        let value = serde_json::to_value(ToolDefinition {
            annotations: None,
            name: "forms.get".to_string(),
            description: "desc".to_string(),
            input_schema: json!({ "type": "object" }),
        })
        .unwrap_or(Value::Null);

        assert!(value.get("inputSchema").is_some());
        assert!(value.get("input_schema").is_none());
    }
}
