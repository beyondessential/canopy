//! Who is calling. The gates in front of the interface say who it is by putting
//! a [`Caller`] in the request's extensions, and a tool that writes reads it
//! there to decide whether the caller may, and to record the write as theirs.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Access tokens"
//! and "Reads and writes".

use std::{fmt, pin::Pin, sync::Arc};

use commons_errors::AppError;
use database::{
	audit::Actor,
	mcp_tokens::{GRANT_NOTES, GRANT_TRIAGE},
	notes::AuthorKind,
	notes_log::Author,
};
use rmcp::{handler::server::tool::Extension, model::ErrorData as McpError};
use uuid::Uuid;

type AdminFuture = Pin<Box<dyn Future<Output = Result<bool, AppError>> + Send>>;

/// Works out whether an operator is an administrator, when a write asks. A
/// read never does, so it costs reads nothing and cannot fail them.
#[derive(Clone)]
pub struct AdminCheck(Arc<dyn Fn() -> AdminFuture + Send + Sync>);

impl AdminCheck {
	pub fn new<F, Fut>(check: F) -> Self
	where
		F: Fn() -> Fut + Send + Sync + 'static,
		Fut: Future<Output = Result<bool, AppError>> + Send + 'static,
	{
		Self(Arc::new(move || Box::pin(check())))
	}

	async fn resolve(&self) -> Result<bool, AppError> {
		(self.0)().await
	}
}

impl fmt::Debug for AdminCheck {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("AdminCheck")
	}
}

/// A caller the gates have authenticated.
#[derive(Clone, Debug)]
pub enum Caller {
	/// A tailnet user, on the operator surface.
	Operator {
		login: String,
		name: String,
		/// Whether they are an administrator, which writes require.
		admin: AdminCheck,
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

	/// The name a note or an edit is attributed to: an operator's login, a
	/// token's name.
	pub fn name(&self) -> &str {
		match self {
			Self::Operator { login, .. } => login,
			Self::Token { name, .. } => name,
		}
	}

	/// Whether a note the caller writes is an operator's or an agent's.
	pub fn note_author_kind(&self) -> AuthorKind {
		match self {
			Self::Operator { .. } => AuthorKind::Operator,
			Self::Token { .. } => AuthorKind::Agent,
		}
	}

	/// The caller as the author of a change to notes or memory.
	pub fn notes_author(&self) -> Author<'_> {
		match self {
			Self::Operator { login, .. } => Author::operator(login),
			Self::Token { name, .. } => Author::token(name),
		}
	}

	/// Whether the caller may write notes and incident memory: an administrator
	/// on the operator surface, a token with the notes grant on the other.
	pub async fn may_note(&self) -> Result<bool, AppError> {
		match self {
			Self::Operator { admin, .. } => admin.resolve().await,
			Self::Token { grants, .. } => Ok(grants.iter().any(|grant| grant == GRANT_NOTES)),
		}
	}

	/// Whether the caller may record vulnerability triage decisions: an
	/// administrator on the operator surface, a token with the triage grant on
	/// the other.
	pub async fn may_triage(&self) -> Result<bool, AppError> {
		match self {
			Self::Operator { admin, .. } => admin.resolve().await,
			Self::Token { grants, .. } => Ok(grants.iter().any(|grant| grant == GRANT_TRIAGE)),
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
