//! Package authoring: submit a signed `.svpkg` that a person approves.

use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

/// The package limit of the API upload route: a signed package is at most 32 MiB.
const MAX_PACKAGE_BYTES: usize = 32 * 1024 * 1024;

pub fn upload_package_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "packages.upload".to_string(),
        description: "Submit a signed .svpkg package (maximum 32 MiB) for a person to approve. The server \
verifies it against the publisher keys a person registered for this workspace (an unsigned package, an \
unknown or revoked key and a malformed package are refused with the same codes as the console upload), keeps \
the bytes with a package_upload draft and answers the draft. Nothing is stored as a package and nothing is \
installed: a workspace owner or administrator approves the upload draft in the console, which stores the \
package, and a person then reviews its installation separately. You build and sign the package yourself with \
your own key (see the guide, \"Build a plugin package\"). A newer upload of yours for the same package id and \
version replaces your older pending one; at most 3 of your uploads and 10 uploads of the workspace wait, and a pending upload expires after 7 days. One upload per bot runs at a time, at most 10 attempts a minute. Needs a bot of the whole \
workspace with write permission."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "package_base64": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The signed .svpkg file, standard base64"
                }
            },
            "required": ["package_base64"]
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadPackageInput {
    package_base64: String,
}

pub async fn upload_package(client: &OpenPrClient, args: serde_json::Value) -> CallToolResult {
    let input: UploadPackageInput = match serde_json::from_value(args) {
        Ok(input) => input,
        Err(err) => return CallToolResult::error(format!("Invalid input: {err}")),
    };
    // The encoded form of the limit, padding included: refuse before decoding anything larger.
    if input.package_base64.len() > MAX_PACKAGE_BYTES.div_ceil(3) * 4 {
        return CallToolResult::error("the package is larger than 32 MiB".to_string());
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(input.package_base64.trim()) {
        Ok(bytes) => bytes,
        Err(err) => return CallToolResult::error(format!("Invalid base64 content: {err}")),
    };
    if bytes.is_empty() || bytes.len() > MAX_PACKAGE_BYTES {
        return CallToolResult::error("the package must be between 1 byte and 32 MiB".to_string());
    }
    match client.submit_package_upload(bytes).await {
        Ok(draft) => CallToolResult::success(serde_json::to_string_pretty(&draft).unwrap_or_default()),
        Err(err) => CallToolResult::error(err),
    }
}
