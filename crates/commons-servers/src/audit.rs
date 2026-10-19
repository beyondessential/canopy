//! Capturing the audit log at the request boundary.
//!
//! Spec: `.workhorse/specs/platform/audit-log.md` (id `AUD`).
//!
//! Every handler declares, beside its grade, whether it is audited or silent
//! (see `canopy_utoipa_axum::routes!`). That declaration is an OpenAPI operation
//! extension, which [`AuditMap`] reads back at startup, the way the private
//! server's safety grades are read back.
//!
//! [`enforce`] is the layer. For an audited route it starts an [`AuditSession`]
//! and leaves it in the request's extensions; the [`Audit`] extractor hands it to
//! the handler, which does its work on the transaction the session holds and
//! records an [`Action`] for each thing it does. When the handler is done the
//! layer commits that transaction if the response succeeded and every action
//! recorded was one the route declared, and rolls it back otherwise, so an
//! audited handler that records nothing, or something it did not declare, fails
//! and changes nothing.
//!
//! An entry can also be committed ahead of the handler's own transaction, as
//! pending, for an action with an effect outside Canopy's records and for an
//! audited read. The handler settles it; for a read the layer settles it on the
//! handler's behalf, since a read has no effect to be unsure of.

use std::{
	collections::HashMap,
	sync::{Arc, Mutex},
};

use axum::{
	extract::{FromRequestParts, MatchedPath, Request},
	http::{Method, request::Parts},
	middleware::Next,
	response::Response,
};
use canopy_utoipa_axum::{AUDIT_EXTENSION, AUDIT_OPTIONAL_EXTENSION, AuditExtractor};
use commons_errors::{AppError, Result};
use database::{
	Db, PooledConnection,
	audit::{self, Action, Actor, Outcome, Pending},
};
use diesel_async::{
	AnsiTransactionManager, AsyncConnection, AsyncPgConnection, TransactionManager,
};
use serde_json::Value;

use crate::{effect::EffectFailed, tailscale_auth::TailscaleUser};

/// The actions each handler declares, keyed by the method and routed path the
/// request matched. A handler that is silent declares none.
#[derive(Debug, Default)]
pub struct AuditMap(HashMap<(Method, String), Declared>);

/// The actions a handler declares, and whether it must record one.
#[derive(Debug, Clone, Default)]
pub struct Declared {
	pub actions: Vec<String>,
	/// A handler that may record these without having to: it records none on a
	/// request that takes no action.
	pub optional: bool,
}

impl AuditMap {
	/// Read every operation's declaration out of the built OpenAPI document.
	///
	/// An operation carrying no declaration is absent from the map rather than
	/// defaulting to silent, so [`Self::declared`] can tell "silent" apart from
	/// "not declared at all", which [`enforce`] refuses.
	pub fn from_openapi(api: &utoipa::openapi::OpenApi) -> Self {
		let mut map = HashMap::new();
		for (path, item) in &api.paths.paths {
			let operations = [
				(Method::GET, &item.get),
				(Method::PUT, &item.put),
				(Method::POST, &item.post),
				(Method::DELETE, &item.delete),
				(Method::OPTIONS, &item.options),
				(Method::HEAD, &item.head),
				(Method::PATCH, &item.patch),
				(Method::TRACE, &item.trace),
			];
			for (method, operation) in operations {
				let Some(operation) = operation else { continue };
				let Some(actions) = operation
					.extensions
					.as_ref()
					.and_then(|ext| ext.get(AUDIT_EXTENSION))
					.and_then(|value| value.as_array())
				else {
					continue;
				};
				let actions = actions
					.iter()
					.filter_map(|action| action.as_str().map(str::to_owned))
					.collect();
				let optional = operation
					.extensions
					.as_ref()
					.and_then(|ext| ext.get(AUDIT_OPTIONAL_EXTENSION))
					.and_then(|value| value.as_bool())
					.unwrap_or(false);
				map.insert((method, path.clone()), Declared { actions, optional });
			}
		}
		Self(map)
	}

	/// The actions a request matching this method and path declares: empty for
	/// a silent handler, `None` where the handler declares nothing.
	pub fn declared(&self, method: &Method, path: &str) -> Option<&Declared> {
		self.0.get(&(method.clone(), path.to_owned()))
	}

	/// Whether any method on this path declares something.
	pub fn knows_path(&self, path: &str) -> bool {
		self.0.keys().any(|(_, known)| known == path)
	}

	/// How many handlers carry a declaration.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}

	/// How many of them are audited rather than silent.
	pub fn audited(&self) -> usize {
		self.0
			.values()
			.filter(|declared| !declared.actions.is_empty())
			.count()
	}

	/// Every action name any handler declares, for checking against the
	/// catalogue.
	pub fn action_names(&self) -> impl Iterator<Item = &str> {
		self.0
			.values()
			.flat_map(|declared| declared.actions.iter())
			.map(String::as_str)
	}
}

/// Where the actor of an audited request comes from.
#[derive(Clone, Copy, Debug)]
pub enum ActorSource {
	/// An operator, by the tailnet identity the request carries.
	Operator,
	/// Whoever the handler authenticates, which it names with
	/// [`Audit::acting_as`].
	Handler,
}

/// A device as an actor, and the application it acted for where it acted for
/// one.
pub fn device_actor(device: &database::Device, application: Option<uuid::Uuid>) -> Actor {
	Actor::Device {
		id: device.id,
		name: None,
		role: device.role,
		application,
	}
}

/// What [`enforce`] needs to decide a request.
#[derive(Clone)]
pub struct AuditState {
	pub db: Db,
	pub map: Arc<AuditMap>,
	pub actor: ActorSource,
	/// Where the routes are mounted beneath, where the declarations are of the
	/// routes without it: a nested router's matched path carries the mount.
	pub mount: &'static str,
}

/// An entry committed ahead whose read the layer settles if the handler does not.
struct ReadEntry {
	pending: Pending,
	settled: bool,
}

/// One audited request: the transaction its handler works in, and what it has
/// recorded.
pub struct AuditSession {
	db: Db,
	declared: Vec<String>,
	optional: bool,
	actor: Mutex<Option<Actor>>,
	recorded: Mutex<Vec<&'static str>>,
	unchanged: std::sync::atomic::AtomicBool,
	reads: Mutex<Vec<ReadEntry>>,
	conn: tokio::sync::Mutex<Option<PooledConnection>>,
}

impl AuditSession {
	fn new(db: Db, declared: Declared, actor: Option<Actor>) -> Self {
		Self {
			db,
			declared: declared.actions,
			optional: declared.optional,
			actor: Mutex::new(actor),
			recorded: Mutex::default(),
			unchanged: Default::default(),
			reads: Mutex::default(),
			conn: tokio::sync::Mutex::new(None),
		}
	}

	fn actor(&self) -> Result<Actor> {
		self.actor
			.lock()
			.unwrap_or_else(|held| held.into_inner())
			.clone()
			.ok_or_else(|| AppError::custom("an audited action has no actor to record"))
	}

	/// Refuse an action the route does not declare before it is committed ahead,
	/// since an entry committed ahead stands whatever the layer decides after.
	fn check_declared(&self, name: &str) -> Result<()> {
		if self.declared.iter().any(|declared| declared == name) {
			Ok(())
		} else {
			Err(AppError::custom(format!(
				"this handler recorded the action {name}, which its route does not declare"
			)))
		}
	}

	fn note(&self, name: &'static str) {
		self.recorded
			.lock()
			.unwrap_or_else(|held| held.into_inner())
			.push(name);
	}

	fn mark_settled(&self, pending: &Pending) {
		for read in self
			.reads
			.lock()
			.unwrap_or_else(|held| held.into_inner())
			.iter_mut()
		{
			if read.pending.id == pending.id {
				read.settled = true;
			}
		}
	}

	/// What the handler recorded that the route did not declare, and whether it
	/// recorded anything at all.
	fn check(&self) -> Result<()> {
		let recorded = self
			.recorded
			.lock()
			.unwrap_or_else(|held| held.into_inner());
		let unchanged = self.unchanged.load(std::sync::atomic::Ordering::Relaxed);
		if unchanged && !recorded.is_empty() {
			return Err(AppError::custom(
				"this handler declared that it changed nothing, and recorded an action",
			));
		}
		if recorded.is_empty() && !self.optional && !unchanged {
			return Err(AppError::custom(
				"this audited handler completed without recording an action",
			));
		}
		if let Some(undeclared) = recorded
			.iter()
			.find(|name| !self.declared.iter().any(|declared| declared == **name))
		{
			return Err(AppError::custom(format!(
				"this handler recorded the action {undeclared}, which its route does not declare"
			)));
		}
		Ok(())
	}

	fn unsettled_reads(&self) -> Vec<Pending> {
		self.reads
			.lock()
			.unwrap_or_else(|held| held.into_inner())
			.iter()
			.filter(|read| !read.settled)
			.map(|read| read.pending.clone())
			.collect()
	}
}

/// The audit extractor: the handle an audited handler works through.
///
/// Taking it is what makes a handler able to record, and `routes!` checks at
/// compile time that an audited handler takes it. On a silent route it is
/// refused.
#[derive(Clone)]
pub struct Audit(Arc<AuditSession>);

impl AuditExtractor for Audit {}

impl<S> FromRequestParts<S> for Audit
where
	S: Send + Sync,
{
	type Rejection = AppError;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self> {
		parts
			.extensions
			.get::<Arc<AuditSession>>()
			.cloned()
			.map(Self)
			.ok_or_else(|| {
				AppError::custom(
					"this handler takes the audit extractor but is not declared audited",
				)
			})
	}
}

impl Audit {
	/// Name the actor, on a surface where the handler is what authenticates
	/// them, such as a device.
	pub fn acting_as(&self, actor: Actor) {
		*self.0.actor.lock().unwrap_or_else(|held| held.into_inner()) = Some(actor);
	}

	/// Declare that the request changed nothing, which is a valid outcome of an
	/// audited handler with no entry, such as a save that sends what is already
	/// held. A handler that records nothing and does not say so fails, and one
	/// that says so and records an action fails too.
	pub fn unchanged(&self) {
		self.0
			.unchanged
			.store(true, std::sync::atomic::Ordering::Relaxed);
	}

	/// The transaction the handler works in. Its changes and the entries it
	/// records commit together once the handler succeeds, and roll back
	/// together otherwise.
	///
	/// The transaction is held across everything the handler does with it, so a
	/// handler asks for it once, and holds no other lock on it meanwhile.
	pub async fn begin(&self) -> Result<AuditTx<'_>> {
		let mut guard = self.0.conn.lock().await;
		if guard.is_none() {
			let mut conn = self.0.db.get().await?;
			AnsiTransactionManager::begin_transaction(&mut *conn)
				.await
				.map_err(AppError::from)?;
			*guard = Some(conn);
		}
		Ok(AuditTx {
			guard,
			session: &self.0,
		})
	}

	/// Commit an entry ahead of the action, as pending, on a connection of its
	/// own so it stands whatever becomes of the handler's transaction. The
	/// handler settles it: done in its transaction, failed with
	/// [`Self::settle_failed`].
	pub async fn commit_ahead(&self, action: Action) -> Result<Pending> {
		self.0.check_declared(action.name())?;
		if self.0.unchanged.load(std::sync::atomic::Ordering::Relaxed) {
			return Err(AppError::custom(
				"this handler declared that it changed nothing, and recorded an action",
			));
		}
		let actor = self.0.actor()?;
		let mut conn = self.0.db.get().await?;
		let pending = audit::commit_ahead(&mut conn, &actor, &action).await?;
		self.0.note(action.name());
		Ok(pending)
	}

	/// Commit an entry ahead of an audited read, which the layer settles done
	/// when the handler succeeds and failed when it errors, unless the handler
	/// settles it itself first.
	pub async fn read(&self, action: Action) -> Result<Pending> {
		let pending = self.commit_ahead(action).await?;
		self.0
			.reads
			.lock()
			.unwrap_or_else(|held| held.into_inner())
			.push(ReadEntry {
				pending: pending.clone(),
				settled: false,
			});
		Ok(pending)
	}

	/// Settle the entry committed ahead of an effect that failed: failed where
	/// nothing was done, and otherwise left to lapse and read as unconfirmed.
	/// Returns the effect's error, for the handler to return.
	pub async fn effect_failed(&self, pending: &Pending, failed: EffectFailed) -> AppError {
		if failed.nothing_done
			&& let Err(error) = self.settle_failed(pending, None).await
		{
			// Left pending, it lapses and reads as unconfirmed, which is not
			// wrong, only less than known.
			tracing::error!(%error, entry = %pending.id, "could not settle a failed effect");
		}
		failed.error
	}

	/// Settle a pending entry failed, on a connection of its own, since the
	/// handler's transaction is rolled back. Only for an action that knows its
	/// effect did not take place.
	pub async fn settle_failed(&self, pending: &Pending, extra: Option<Value>) -> Result<()> {
		settle_alone(&self.0.db, pending, Outcome::Failed, extra).await?;
		self.0.mark_settled(pending);
		Ok(())
	}
}

/// The handler's transaction. Dereferences to the connection, so model
/// functions take it as they would a connection of their own.
pub struct AuditTx<'a> {
	guard: tokio::sync::MutexGuard<'a, Option<PooledConnection>>,
	session: &'a AuditSession,
}

impl std::ops::Deref for AuditTx<'_> {
	type Target = AsyncPgConnection;

	fn deref(&self) -> &Self::Target {
		self.guard
			.as_deref()
			.expect("begin() leaves a connection in the session")
	}
}

impl std::ops::DerefMut for AuditTx<'_> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		self.guard
			.as_deref_mut()
			.expect("begin() leaves a connection in the session")
	}
}

impl AuditTx<'_> {
	/// Record an action in this transaction, so the entry and the change it
	/// records stand or fall together.
	pub async fn record(&mut self, action: Action) -> Result<()> {
		let actor = self.session.actor()?;
		audit::record(&mut *self, &actor, &action).await?;
		self.session.note(action.name());
		Ok(())
	}

	/// Settle a pending entry done, in this transaction, with the rest of the
	/// action's work. `extra` is merged into its detail.
	pub async fn settle_done(&mut self, pending: &Pending, extra: Option<Value>) -> Result<()> {
		audit::settle(&mut *self, pending, Outcome::Done, extra).await?;
		self.session.mark_settled(pending);
		Ok(())
	}
}

/// Settle a pending entry in a transaction of its own.
async fn settle_alone(
	db: &Db,
	pending: &Pending,
	outcome: Outcome,
	extra: Option<Value>,
) -> Result<()> {
	let mut conn = db.get().await?;
	conn.transaction::<_, AppError, _>(async |tx| audit::settle(tx, pending, outcome, extra).await)
		.await
}

/// Settle what a read left pending, for the handler. Quietly: the response is
/// already decided, and an entry left to lapse is shown as unconfirmed.
async fn settle_reads(session: &AuditSession, outcome: Outcome) {
	for pending in session.unsettled_reads() {
		if let Err(error) = settle_alone(&session.db, &pending, outcome, None).await {
			tracing::error!(%error, entry = %pending.id, "could not settle an audited read");
		}
	}
}

/// Decide one request against the audit declaration of its handler.
///
/// A route declaring no audit actions is silent and passes through. One
/// declaring actions is run under an [`AuditSession`], and held to them.
pub async fn enforce(
	axum::extract::State(state): axum::extract::State<AuditState>,
	request: Request,
	next: Next,
) -> Result<Response> {
	let path = request
		.extensions()
		.get::<MatchedPath>()
		.map(|matched| matched.as_str().to_owned())
		.unwrap_or_else(|| request.uri().path().to_owned());
	let path = path
		.strip_prefix(state.mount)
		.map(str::to_owned)
		.unwrap_or(path);

	let Some(declared) = state.map.declared(request.method(), &path) else {
		// A route that declares nothing is the server failing to recognise its
		// own route, not one that is silent: refused, because letting it
		// through is the one outcome that would leave a hole in the log. Routes
		// deliberately outside the document, such as pages, are mounted outside
		// this layer. A path the surface serves under another method is the
		// router's to refuse.
		if state.map.knows_path(&path) {
			return Ok(next.run(request).await);
		}
		tracing::error!(%path, method = %request.method(), "no audit declaration for this route; refusing");
		return Err(AppError::custom(format!(
			"no audit declaration is made for {path}"
		)));
	};
	if declared.actions.is_empty() {
		return Ok(next.run(request).await);
	}
	let declared = declared.clone();

	let (mut parts, body) = request.into_parts();
	let actor = match state.actor {
		ActorSource::Operator => {
			let user = TailscaleUser::from_request_parts(&mut parts, &()).await?;
			Some(Actor::operator(user.login, Some(user.name)))
		}
		ActorSource::Handler => None,
	};
	let session = Arc::new(AuditSession::new(state.db.clone(), declared, actor));
	parts.extensions.insert(session.clone());

	let response = next.run(Request::from_parts(parts, body)).await;
	let succeeded = !(response.status().is_client_error() || response.status().is_server_error());

	let checked = if succeeded { session.check() } else { Ok(()) };

	let mut held = session.conn.lock().await;
	let finished = match held.take() {
		Some(mut conn) => {
			let conn: &mut AsyncPgConnection = &mut conn;
			if succeeded && checked.is_ok() {
				AnsiTransactionManager::commit_transaction(conn)
					.await
					.map_err(AppError::from)
			} else {
				// The connection is dropped back to the pool either way; one
				// that fails to roll back is broken and the pool discards it.
				let _ = AnsiTransactionManager::rollback_transaction(conn).await;
				Ok(())
			}
		}
		None => Ok(()),
	};
	drop(held);

	let outcome = if succeeded && checked.is_ok() && finished.is_ok() {
		Outcome::Done
	} else {
		Outcome::Failed
	};
	settle_reads(&session, outcome).await;

	checked?;
	finished?;
	Ok(response)
}
