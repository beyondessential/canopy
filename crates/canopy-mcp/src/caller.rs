//! Who is calling. The gates in front of the interface say who it is by putting
//! a [`Caller`] in the request's extensions, and a tool that writes reads it
//! there to decide whether the caller may, and to record the write as theirs.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Access tokens"
//! and "Reads and writes".

use database::{audit::Actor, mcp_tokens::GRANT_TRIAGE};
use rmcp::{handler::server::tool::Extension, model::ErrorData as McpError};
use uuid::Uuid;

/// A caller the gates have authenticated.
#[derive(Clone, Debug)]
pub enum Caller {
	/// A tailnet user, on the operator surface.
	Operator {
		login: String,
		name: String,
		/// Whether they are an administrator, which writes require.
		admin: bool,
	},
	/// A fleet query access token, on the internet-facing surface.
	Token {
		id: Uuid,
		name: String,
		grants: Vec<String>,
	},
}

impl Caller {
	/// The actor a write is recorded under.
	pub fn actor(&self) -> Actor {
		match self {
			Self::Operator { login, name, .. } => Actor::operator(login, Some(name.clone())),
			Self::Token { id, name, .. } => Actor::Token {
				id: *id,
				name: name.clone(),
			},
		}
	}

	/// Whether the caller may record vulnerability triage decisions: an
	/// administrator on the operator surface, a token with the triage grant on
	/// the other.
	pub fn may_triage(&self) -> bool {
		match self {
			Self::Operator { admin, .. } => *admin,
			Self::Token { grants, .. } => grants.iter().any(|grant| grant == GRANT_TRIAGE),
		}
	}
}

/// The caller of the request a tool is answering, as the gate in front of the
/// interface identified them. A request that reached a tool without being
/// identified is refused.
pub(crate) fn of(parts: Extension<http::request::Parts>) -> Result<Caller, McpError> {
	let Extension(parts) = parts;
	parts
		.extensions
		.get::<Caller>()
		.cloned()
		.ok_or_else(|| McpError::invalid_request("the caller could not be identified", None))
}
