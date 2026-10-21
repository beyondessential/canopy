//! The catalogue of Ansible variables and playbooks infra publishes, and what
//! Canopy last made of reading it.
//!
//! The catalogue is a JSON Schema of an environment's variables with
//! annotations under `x-canopy`. Canopy keeps the last copy that read as a
//! catalogue and holds the variables it describes to it.
// spec: ANSVAR#catalogue

use std::{
	collections::BTreeMap,
	fmt::Display,
	sync::{Arc, Mutex},
};

use commons_errors::{AppError, Result};
use diesel::{prelude::*, upsert::excluded};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use jsonschema::Validator;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
	ansible_leases::RunIntent, ansible_variables::VariableScope, schema::ansible_catalogue,
};

/// The three places a variable is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
	Group,
	Environment,
	Machine,
}

impl ScopeKind {
	fn parse(name: &str) -> Option<Self> {
		match name {
			"group" => Some(Self::Group),
			"environment" => Some(Self::Environment),
			"machine" => Some(Self::Machine),
			_ => None,
		}
	}
}

impl Display for ScopeKind {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Group => "group",
			Self::Environment => "environment",
			Self::Machine => "machine",
		})
	}
}

impl From<VariableScope> for ScopeKind {
	fn from(scope: VariableScope) -> Self {
		match scope {
			VariableScope::Group { .. } => Self::Group,
			VariableScope::Environment { .. } => Self::Environment,
			VariableScope::Machine { .. } => Self::Machine,
		}
	}
}

/// A variable the catalogue describes.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogueVariable {
	/// The variable's name.
	pub name: String,
	/// What the variable means.
	pub description: Option<String>,
	/// The variable's JSON Schema, as the catalogue gives it, which is what a
	/// form for its value is shaped by.
	#[schema(value_type = serde_json::Value)]
	pub schema: Value,
	/// The value a playbook uses where the variable is not set.
	#[schema(value_type = Option<serde_json::Value>)]
	pub default: Option<Value>,
	/// Whether its value is a secret.
	pub secret: bool,
	/// The scopes it may be set at, every scope where absent.
	pub scopes: Option<Vec<ScopeKind>>,
	/// The application types it configures, any where absent.
	pub applications: Option<Vec<String>>,
	/// Whether the inventory plugin sets it for itself, so it is never stored.
	pub reserved: bool,
}

/// What a run of a playbook is started as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
	Install,
	Upgrade,
}

/// A playbook a run can start.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CataloguePlaybook {
	/// The playbook's file name.
	pub name: String,
	/// What the playbook does.
	pub description: Option<String>,
	/// The intent a run of it is taken with.
	pub intent: RunIntent,
	/// The run button it backs, where it backs one.
	pub preset: Option<Preset>,
	/// The application types it configures, every type where absent.
	pub applications: Option<Vec<String>>,
	/// The tags it takes.
	pub tags: Vec<String>,
}

/// A catalogue that has been read and checked.
pub struct Catalogue {
	variables: BTreeMap<String, CatalogueVariable>,
	playbooks: Vec<CataloguePlaybook>,
	tags: BTreeMap<String, String>,
	validator: Validator,
}

impl std::fmt::Debug for Catalogue {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Catalogue")
			.field("variables", &self.variables.len())
			.field("playbooks", &self.playbooks.len())
			.finish()
	}
}

/// Why a document is not a catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotACatalogue(pub String);

impl Display for NotACatalogue {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for NotACatalogue {}

fn invalid(what: impl Into<String>) -> NotACatalogue {
	NotACatalogue(what.into())
}

const X_CANOPY: &str = "x-canopy";

impl Catalogue {
	/// Read a document as a catalogue: a JSON Schema that is itself valid, with
	/// annotations of the shape Canopy reads.
	pub fn parse(document: &Value) -> std::result::Result<Self, NotACatalogue> {
		let root = document
			.as_object()
			.ok_or_else(|| invalid("the catalogue is not a JSON object"))?;
		let properties = match root.get("properties") {
			Some(Value::Object(properties)) => properties,
			Some(_) => return Err(invalid("`properties` is not an object")),
			None => return Err(invalid("the catalogue has no `properties`")),
		};

		jsonschema::meta::validate(document)
			.map_err(|error| invalid(format!("not a valid JSON Schema: {error}")))?;

		// A value is checked as the one property of an environment, so rules
		// that the whole environment must carry some variable would refuse it.
		let mut checked = root.clone();
		checked.remove("required");
		let validator = jsonschema::options()
			.with_draft(jsonschema::Draft::Draft202012)
			.should_validate_formats(true)
			.build(&Value::Object(checked))
			.map_err(|error| invalid(format!("the schema cannot be compiled: {error}")))?;

		let mut variables = BTreeMap::new();
		for (name, schema) in properties {
			variables.insert(name.clone(), variable(name, schema)?);
		}

		let (playbooks, tags) = match root.get(X_CANOPY) {
			None => (Vec::new(), BTreeMap::new()),
			Some(Value::Object(top)) => (playbooks(top)?, tag_notes(top)?),
			Some(_) => return Err(invalid("the top-level `x-canopy` is not an object")),
		};

		Ok(Self {
			variables,
			playbooks,
			tags,
			validator,
		})
	}

	pub fn variables(&self) -> impl Iterator<Item = &CatalogueVariable> {
		self.variables.values()
	}

	pub fn variable(&self, name: &str) -> Option<&CatalogueVariable> {
		self.variables.get(name)
	}

	pub fn playbooks(&self) -> &[CataloguePlaybook] {
		&self.playbooks
	}

	pub fn playbook(&self, name: &str) -> Option<&CataloguePlaybook> {
		self.playbooks.iter().find(|playbook| playbook.name == name)
	}

	/// What the catalogue says of tags that have a documented meaning.
	pub fn tag_notes(&self) -> &BTreeMap<String, String> {
		&self.tags
	}

	/// Whether the catalogue calls the variable secret.
	pub fn is_secret(&self, name: &str) -> bool {
		self.variables
			.get(name)
			.is_some_and(|variable| variable.secret)
	}

	/// Whether the catalogue reserves the variable for the inventory plugin.
	pub fn is_reserved(&self, name: &str) -> bool {
		self.variables
			.get(name)
			.is_some_and(|variable| variable.reserved)
	}

	/// What stops a variable being set as `name` at `scope` whatever its value
	/// is: a reserved name, or a scope the catalogue does not allow it at.
	pub fn placement_violations(&self, scope: ScopeKind, name: &str) -> Vec<String> {
		let Some(variable) = self.variables.get(name) else {
			return Vec::new();
		};
		if variable.reserved {
			return vec![format!(
				"{name:?} is set by a run for itself, so it cannot be set"
			)];
		}
		if let Some(scopes) = &variable.scopes
			&& !scopes.contains(&scope)
		{
			return vec![format!(
				"{name:?} can be set only at {}, not at {scope}",
				join_scopes(scopes)
			)];
		}
		Vec::new()
	}

	/// What stops `value` being set as `name` at `scope`, if anything: a
	/// placement the catalogue refuses, or a value that does not match its
	/// type. A name the catalogue does not describe is not held to anything.
	pub fn violations(&self, scope: ScopeKind, name: &str, value: &Value) -> Vec<String> {
		let placement = self.placement_violations(scope, name);
		if !placement.is_empty() || !self.variables.contains_key(name) {
			return placement;
		}

		let mut instance = Map::new();
		instance.insert(name.to_owned(), value.clone());
		let instance = Value::Object(instance);
		self.validator
			.iter_errors(&instance)
			.map(|error| {
				let at = error.instance_path().to_string();
				// The path leads with the variable's own name, which the caller
				// has already said, so what follows it is where in the value.
				let within = at.strip_prefix(&format!("/{name}")).unwrap_or(&at);
				if within.is_empty() {
					format!("{name:?}: {error}")
				} else {
					format!("{name:?} at {within}: {error}")
				}
			})
			.collect()
	}
}

fn join_scopes(scopes: &[ScopeKind]) -> String {
	scopes
		.iter()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join(" or ")
}

fn variable(name: &str, schema: &Value) -> std::result::Result<CatalogueVariable, NotACatalogue> {
	let here = |what: &str| invalid(format!("variable {name:?}: {what}"));
	let schema_object = schema.as_object();
	let annotations = match schema_object.and_then(|schema| schema.get(X_CANOPY)) {
		None => None,
		Some(Value::Object(annotations)) => Some(annotations),
		Some(_) => return Err(here("`x-canopy` is not an object")),
	};

	let flag = |key: &str| match annotations.and_then(|a| a.get(key)) {
		None => Ok(false),
		Some(Value::Bool(flag)) => Ok(*flag),
		Some(_) => Err(here(&format!("`x-canopy.{key}` is not a boolean"))),
	};
	let strings = |key: &str| -> std::result::Result<Option<Vec<String>>, NotACatalogue> {
		match annotations.and_then(|a| a.get(key)) {
			None => Ok(None),
			Some(Value::Array(items)) => items
				.iter()
				.map(|item| {
					item.as_str()
						.map(str::to_owned)
						.ok_or_else(|| here(&format!("`x-canopy.{key}` holds a non-string")))
				})
				.collect::<std::result::Result<Vec<_>, _>>()
				.map(Some),
			Some(_) => Err(here(&format!("`x-canopy.{key}` is not a list"))),
		}
	};

	let scopes = strings("scope")?
		.map(|names| {
			names
				.iter()
				.map(|name| {
					ScopeKind::parse(name).ok_or_else(|| {
						here(&format!(
							"`x-canopy.scope` names {name:?}, which is not group, environment or machine"
						))
					})
				})
				.collect::<std::result::Result<Vec<_>, _>>()
		})
		.transpose()?;

	Ok(CatalogueVariable {
		name: name.to_owned(),
		description: schema_object
			.and_then(|schema| schema.get("description"))
			.and_then(Value::as_str)
			.map(str::to_owned),
		schema: schema.clone(),
		default: schema_object
			.and_then(|schema| schema.get("default"))
			.cloned(),
		secret: flag("secret")?,
		scopes,
		applications: strings("applications")?,
		reserved: flag("reserved")?,
	})
}

fn playbooks(
	top: &Map<String, Value>,
) -> std::result::Result<Vec<CataloguePlaybook>, NotACatalogue> {
	let Some(playbooks) = top.get("playbooks") else {
		return Ok(Vec::new());
	};
	let playbooks = playbooks
		.as_object()
		.ok_or_else(|| invalid("`x-canopy.playbooks` is not an object"))?;

	let mut out = Vec::with_capacity(playbooks.len());
	for (name, playbook) in playbooks {
		let here = |what: &str| invalid(format!("playbook {name:?}: {what}"));
		let playbook = playbook
			.as_object()
			.ok_or_else(|| here("is not an object"))?;
		let intent = match playbook.get("intent").and_then(Value::as_str) {
			Some("configure") => RunIntent::Configure,
			Some("upgrade") => RunIntent::Upgrade,
			_ => return Err(here("`intent` is not configure or upgrade")),
		};
		let preset = match playbook.get("preset") {
			None => None,
			Some(Value::String(preset)) if preset == "install" => Some(Preset::Install),
			Some(Value::String(preset)) if preset == "upgrade" => Some(Preset::Upgrade),
			Some(_) => return Err(here("`preset` is not install or upgrade")),
		};
		let list = |key: &str| -> std::result::Result<Option<Vec<String>>, NotACatalogue> {
			match playbook.get(key) {
				None => Ok(None),
				Some(Value::Array(items)) => items
					.iter()
					.map(|item| {
						item.as_str()
							.map(str::to_owned)
							.ok_or_else(|| here(&format!("`{key}` holds a non-string")))
					})
					.collect::<std::result::Result<Vec<_>, _>>()
					.map(Some),
				Some(_) => Err(here(&format!("`{key}` is not a list"))),
			}
		};
		out.push(CataloguePlaybook {
			name: name.clone(),
			description: playbook
				.get("description")
				.and_then(Value::as_str)
				.map(str::to_owned),
			intent,
			preset,
			applications: list("applications")?,
			tags: list("tags")?.unwrap_or_default(),
		});
	}
	Ok(out)
}

fn tag_notes(
	top: &Map<String, Value>,
) -> std::result::Result<BTreeMap<String, String>, NotACatalogue> {
	let Some(tags) = top.get("tags") else {
		return Ok(BTreeMap::new());
	};
	tags.as_object()
		.ok_or_else(|| invalid("`x-canopy.tags` is not an object"))?
		.iter()
		.map(|(tag, note)| {
			note.as_str()
				.map(|note| (tag.clone(), note.to_owned()))
				.ok_or_else(|| invalid(format!("tag {tag:?}: the note is not a string")))
		})
		.collect()
}

/// The digest a catalogue document is known by.
pub fn digest(document: &Value) -> String {
	hex::encode(Sha256::digest(document.to_string().as_bytes()))
}

/// Why the latest attempt to read the catalogue did not replace it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
	/// The repository could not be reached.
	Unreachable,
	/// What it published does not read as a catalogue.
	Invalid,
}

impl Problem {
	fn as_str(self) -> &'static str {
		match self {
			Self::Unreachable => "unreachable",
			Self::Invalid => "invalid",
		}
	}

	fn parse(value: &str) -> Option<Self> {
		match value {
			"unreachable" => Some(Self::Unreachable),
			"invalid" => Some(Self::Invalid),
			_ => None,
		}
	}
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = ansible_catalogue)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct Row {
	document: Option<Value>,
	digest: Option<String>,
	source_commit: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	read_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	first_checked_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	checked_at: Timestamp,
	problem: Option<String>,
	problem_detail: Option<String>,
}

/// What Canopy holds of the catalogue and of its attempts to read it.
#[derive(Clone, Debug)]
pub struct CatalogueState {
	/// The last copy that read as a catalogue.
	pub document: Option<Value>,
	pub digest: Option<String>,
	/// The infra commit that copy was read at.
	pub source_commit: Option<String>,
	/// When that copy was read.
	pub read_at: Option<Timestamp>,
	/// When Canopy first tried to read one.
	pub first_checked_at: Timestamp,
	/// When it last tried.
	pub checked_at: Timestamp,
	/// Why the last attempt did not replace the copy, `None` where it did.
	pub problem: Option<Problem>,
	pub problem_detail: Option<String>,
}

impl From<Row> for CatalogueState {
	fn from(row: Row) -> Self {
		Self {
			document: row.document,
			digest: row.digest,
			source_commit: row.source_commit,
			read_at: row.read_at,
			first_checked_at: row.first_checked_at,
			checked_at: row.checked_at,
			problem: row.problem.as_deref().and_then(Problem::parse),
			problem_detail: row.problem_detail,
		}
	}
}

impl CatalogueState {
	/// What is held, `None` before Canopy has tried to read one.
	pub async fn get(db: &mut AsyncPgConnection) -> Result<Option<Self>> {
		ansible_catalogue::table
			.select(Row::as_select())
			.first(db)
			.await
			.optional()
			.map(|row| row.map(Self::from))
			.map_err(AppError::from)
	}

	/// Replace the held copy with one that has just been read as a catalogue.
	/// Answers whether it differs from the copy it replaced.
	pub async fn record_read(
		db: &mut AsyncPgConnection,
		document: &Value,
		source_commit: Option<&str>,
		now: Timestamp,
	) -> Result<bool> {
		let digest = digest(document);
		let before = Self::get(db).await?.and_then(|state| state.digest);
		let now: jiff_diesel::Timestamp = now.into();
		diesel::insert_into(ansible_catalogue::table)
			.values((
				ansible_catalogue::document.eq(document),
				ansible_catalogue::digest.eq(&digest),
				ansible_catalogue::source_commit.eq(source_commit),
				ansible_catalogue::read_at.eq(now),
				ansible_catalogue::first_checked_at.eq(now),
				ansible_catalogue::checked_at.eq(now),
			))
			.on_conflict(ansible_catalogue::id)
			.do_update()
			.set((
				ansible_catalogue::document.eq(excluded(ansible_catalogue::document)),
				ansible_catalogue::digest.eq(excluded(ansible_catalogue::digest)),
				ansible_catalogue::source_commit.eq(excluded(ansible_catalogue::source_commit)),
				ansible_catalogue::read_at.eq(excluded(ansible_catalogue::read_at)),
				ansible_catalogue::checked_at.eq(excluded(ansible_catalogue::checked_at)),
				ansible_catalogue::problem.eq(None::<String>),
				ansible_catalogue::problem_detail.eq(None::<String>),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(before.as_deref() != Some(digest.as_str()))
	}

	/// Note that an attempt did not read a catalogue, leaving the held copy as
	/// it is.
	pub async fn record_problem(
		db: &mut AsyncPgConnection,
		problem: Problem,
		detail: &str,
		now: Timestamp,
	) -> Result<()> {
		let now: jiff_diesel::Timestamp = now.into();
		diesel::insert_into(ansible_catalogue::table)
			.values((
				ansible_catalogue::first_checked_at.eq(now),
				ansible_catalogue::checked_at.eq(now),
				ansible_catalogue::problem.eq(problem.as_str()),
				ansible_catalogue::problem_detail.eq(detail),
			))
			.on_conflict(ansible_catalogue::id)
			.do_update()
			.set((
				ansible_catalogue::checked_at.eq(excluded(ansible_catalogue::checked_at)),
				ansible_catalogue::problem.eq(excluded(ansible_catalogue::problem)),
				ansible_catalogue::problem_detail.eq(excluded(ansible_catalogue::problem_detail)),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}

/// A catalogue read from the database, and what it was read at.
#[derive(Clone, Debug)]
pub struct Loaded {
	pub catalogue: Arc<Catalogue>,
	pub digest: String,
	pub source_commit: Option<String>,
	pub read_at: Timestamp,
}

static COMPILED: Mutex<Option<(String, Arc<Catalogue>)>> = Mutex::new(None);

impl Loaded {
	/// The catalogue Canopy holds, `None` while it has never read one. The
	/// compiled form is kept for as long as the held copy is the same, so a
	/// request does not recompile the schema.
	pub async fn get(db: &mut AsyncPgConnection) -> Result<Option<Self>> {
		let Some(state) = CatalogueState::get(db).await? else {
			return Ok(None);
		};
		let (Some(document), Some(digest), Some(read_at)) =
			(state.document, state.digest, state.read_at)
		else {
			return Ok(None);
		};

		let cached = COMPILED
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner())
			.as_ref()
			.filter(|(held, _)| *held == digest)
			.map(|(_, catalogue)| catalogue.clone());
		let catalogue = match cached {
			Some(catalogue) => catalogue,
			None => {
				// Only a copy that read as a catalogue is ever stored, so this
				// failing means the stored one was not written by `record_read`.
				let catalogue = Arc::new(Catalogue::parse(&document).map_err(|error| {
					AppError::custom(format!("the held catalogue does not read: {error}"))
				})?);
				*COMPILED
					.lock()
					.unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((digest.clone(), catalogue.clone()));
				catalogue
			}
		};
		Ok(Some(Self {
			catalogue,
			digest,
			source_commit: state.source_commit,
			read_at,
		}))
	}
}
