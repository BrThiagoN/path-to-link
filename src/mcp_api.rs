use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::{AppState, CreateShareRequest};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShareFileArgs {
    /// Absolute path of a file below one of the configured allowed roots.
    pub path: String,
    /// Link lifetime in seconds. Defaults to 900 and cannot exceed 86400.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RevokeShareArgs {
    /// Share ID returned by share_file.
    pub id: String,
}

#[derive(Clone)]
pub struct PathToLinkMcp {
    state: AppState,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl PathToLinkMcp {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl PathToLinkMcp {
    #[tool(
        description = "Create or refresh the single temporary HTTP link for an allowed local file. Repeated calls for the same canonical path reuse the link and renew its lifetime. The returned URL includes an access token; treat it as a secret."
    )]
    async fn share_file(
        &self,
        Parameters(args): Parameters<ShareFileArgs>,
    ) -> Result<CallToolResult, McpError> {
        let share = self
            .state
            .create_share(CreateShareRequest {
                path: args.path,
                expires_in: args.expires_in,
            })
            .await
            .map_err(|message| McpError::invalid_params(message, None))?;
        let content = ContentBlock::json(share)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "List currently active Path-to-Link shares, including their URLs and remaining lifetimes."
    )]
    async fn list_shares(&self) -> Result<CallToolResult, McpError> {
        let content = ContentBlock::json(serde_json::json!({
            "shares": self.state.list_shares().await,
        }))
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        description = "Immediately revoke a share by its ID. Future HTTP requests using its URL will fail."
    )]
    async fn revoke_share(
        &self,
        Parameters(args): Parameters<RevokeShareArgs>,
    ) -> Result<CallToolResult, McpError> {
        let revoked = self.state.revoke_share(&args.id).await;
        let content = ContentBlock::json(serde_json::json!({
            "id": args.id,
            "revoked": revoked,
        }))
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }
}

#[tool_handler]
impl ServerHandler for PathToLinkMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Create temporary HTTP URLs for files under explicitly configured local roots. Sharing a path again renews its active link. Links expire automatically and can be revoked. The HTTP server runs alongside this MCP stdio process.",
            )
    }
}
