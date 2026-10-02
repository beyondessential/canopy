//! Grading a check through its instances.
//!
//! Every check is graded here, whether it reports several instances of its
//! condition or none: a check without instances is graded as its own single
//! instance, with an empty key and no label (see [`CheckInstance::plain`]).
//! That is what lets one rule be written for a check and hold whether or not
//! the check reports instances, and whether it starts or stops reporting them
//! (spec CHK, "Checks with instances").
//!
//! [`grade_instances`] is the one grading path. It is pure: the catalog entry
//! and scoped chain are loaded once per check into a [`CheckGrading`], and the
//! caller supplies the rule context it has (the report's fields, the target's
//! tags). Canopy's own filings reach it through
//! [`crate::issues::file_check_instances`]; push ingestion calls it directly.

use std::collections::{BTreeMap, HashMap};

use commons_errors::Result;
use commons_types::namespace::Namespace;
use commons_types::status::CheckResult;
use diesel_async::AsyncPgConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::check_policies::{
	CheckPolicy, EvaluationContext, FilingScope, FleetGrading, ScopedCheckPolicy,
};
use crate::issues::Issue;

/// One instance of a check the target has several of: one of a machine's
/// backup types, one of its restore replicas, one of a central's devices.
///
/// Instances exist so a check can stay a single named category (see the Names
/// section of the CHK spec). Never spell an instance into the check name.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckInstance {
	/// Which instance this is: unique within the check on its target, and the
	/// same from one filing to the next. It is the instance's identity rather
	/// than one of its fields, so no rule reads it; an instance silence names
	/// it. Empty only for the single instance of a check without instances.
	pub key: String,
	/// How the instance is named to an operator. Falls back to the key (see
	/// [`Self::name`]).
	pub label: Option<String>,
	/// What this instance observed this pass. Never broken: brokenness is the
	/// whole check's (see [`CheckOutcome::Broken`]).
	pub observed: CheckResult,
	/// This instance's own fields. Rules reach them as `check.<field>`, over
	/// the fields the check shares across its instances.
	pub detail: Option<Value>,
}

impl CheckInstance {
	/// The single instance a check without instances is graded as.
	pub fn plain(observed: CheckResult, detail: Option<Value>) -> Self {
		Self {
			key: String::new(),
			label: None,
			observed,
			detail,
		}
	}

	/// How this instance is named to an operator: its label, or its key
	/// without one.
	pub fn name(&self) -> &str {
		self.label.as_deref().unwrap_or(&self.key)
	}
}

/// What grading one instance produced.
#[derive(Debug, Clone, PartialEq)]
pub struct GradedInstance {
	pub key: String,
	pub label: Option<String>,
	pub observed: CheckResult,
	pub effective: CheckResult,
	pub detail: Option<Value>,
}

impl GradedInstance {
	/// How this instance is named to an operator: its label, or its key
	/// without one.
	pub fn name(&self) -> &str {
		self.label.as_deref().unwrap_or(&self.key)
	}
}

/// What a filing observed of one check this pass.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckOutcome {
	/// The check ran, and these are its instances: the complete set, so an
	/// instance it held before and does not name now has recovered. A check
	/// without instances is the one [`CheckInstance::plain`]. An empty set says
	/// the check has no instances at all, and it passes.
	Instances(Vec<CheckInstance>),
	/// The check itself could not run. Brokenness belongs to the whole check
	/// and never to one instance, and a broken check says nothing about which
	/// instances exist: it holds the instances its stored state held, each
	/// presented as broken, and recovers none of them.
	// spec: CHK#checks-with-instances
	Broken,
}

/// One check's policy, loaded once and applied to each of its instances: the
/// fleet catalog entry and every scoped transform covering the filing's
/// target, instance-keyed ones included.
#[derive(Debug, Clone, Default)]
pub struct CheckGrading {
	/// The catalog entry. `None` for a check with no catalog row yet.
	pub fleet: Option<FleetGrading>,
	/// The scoped chain, in application order (see
	/// [`ScopedCheckPolicy::chain_for`]).
	pub chain: Vec<ScopedCheckPolicy>,
}

impl CheckGrading {
	/// Load one check's catalog entry and scoped chain for a filing at `scope`.
	pub async fn load(
		conn: &mut AsyncPgConnection,
		source: &str,
		namespace: &Namespace,
		check: &str,
		scope: FilingScope,
	) -> Result<Self> {
		Ok(Self {
			fleet: CheckPolicy::fleet_grading(conn, source, namespace, check).await?,
			chain: ScopedCheckPolicy::chain_for(conn, source, namespace, check, scope).await?,
		})
	}
}

/// What a rule evaluated for one of the check's instances reads, beyond the
/// instance itself.
#[derive(Debug, Clone, Copy)]
pub struct GradingContext<'a> {
	pub source: &'a str,
	pub check: &'a str,
	/// The report's own fields (`status.<field>`). Empty for a filing that
	/// comes from no report, as Canopy's own determinations do.
	pub status_extra: &'a Map<String, Value>,
	/// The target's effective tags (`tag.<key>`).
	pub tags: &'a HashMap<String, Value>,
}

/// What grading a check through its instances produced.
#[derive(Debug, Clone, PartialEq)]
pub struct GradedCheck {
	/// The most urgent observed result across every instance, silenced ones
	/// included: silencing changes what Canopy acts on, never what it saw.
	pub observed: CheckResult,
	/// The most urgent effective result across the instances that were not
	/// skipped; skipped when every instance was. For a broken check whose
	/// policy left it broken, the contribution retained from its last
	/// definite result (see [`grade_instances`]).
	pub effective: CheckResult,
	/// Whether an effective failure of this check escalates, read from the
	/// instances that were not skipped.
	pub escalates: bool,
	/// Every instance as graded, in the order given (a broken check's in the
	/// order it held them).
	pub instances: Vec<GradedInstance>,
	/// The fields the check shares across its instances.
	pub shared: Option<Map<String, Value>>,
	/// Whether the check itself could not run this pass.
	pub broken: bool,
}

impl GradedCheck {
	/// Whether this is a check without instances: its single instance has the
	/// empty key and no label.
	pub fn is_plain(&self) -> bool {
		matches!(self.instances.as_slice(), [only] if only.key.is_empty() && only.label.is_none())
	}

	/// The instances in trouble, most urgent first: everything neither passed
	/// nor skipped. A skipped instance is silenced rather than healthy, so it
	/// is never counted here.
	pub fn degraded(&self) -> Vec<&GradedInstance> {
		let mut degraded: Vec<&GradedInstance> = self
			.instances
			.iter()
			.filter(|i| !matches!(i.effective, CheckResult::Passed | CheckResult::Skipped))
			.collect();
		degraded.sort_by_key(|i| i.effective.urgency_rank());
		degraded
	}

	/// How many instances were not skipped.
	pub fn considered(&self) -> usize {
		self.instances
			.iter()
			.filter(|i| i.effective != CheckResult::Skipped)
			.count()
	}

	/// The detail the check's state stores: a plain check's fields as they
	/// were reported, so a plain check's stored detail is exactly what it
	/// always was, or the fields an instanced check shares.
	pub fn detail(&self) -> Option<Value> {
		if self.is_plain() {
			let only = &self.instances[0];
			return match (&self.shared, &only.detail) {
				(None, detail) => detail.clone(),
				(Some(shared), None) => Some(Value::Object(shared.clone())),
				(Some(shared), Some(detail)) => {
					Some(Value::Object(merged(Some(shared), Some(detail))))
				}
			};
		}
		self.shared.clone().map(Value::Object)
	}

	/// The instances the check's state stores, or `None` for a check without
	/// them: every instance it holds, by key, with its label, both results and
	/// its own fields. Everything that presents an instance (which ones are
	/// degraded or silenced, which keys a silence no longer matches, what a
	/// broken check held) reads it back from there.
	pub fn stored_instances(&self) -> Option<StoredInstances> {
		if self.is_plain() {
			return None;
		}
		Some(StoredInstances(
			self.instances
				.iter()
				.map(|i| {
					(
						i.key.clone(),
						StoredInstance {
							label: i.label.clone(),
							observed: i.observed,
							effective: i.effective,
							detail: i.detail.clone(),
						},
					)
				})
				.collect(),
		))
	}

	/// The message Canopy writes for a check with instances, from its graded
	/// instances: the degraded ones named by label, so an instance a silence
	/// or rule has taken out is never counted in it.
	///
	/// A check without instances is described by whoever filed it; this is for
	/// the instanced form, and for re-grading a stored state that no filer is
	/// present to describe.
	// spec: CHK#checks-with-instances
	pub fn message(&self, check: &str) -> String {
		if self.broken {
			return match self.instances.len() {
				0 | 1 => format!("{check} could not run"),
				n => format!("{check} could not run, so none of its {n} instances is confirmed"),
			};
		}
		let degraded = self.degraded();
		let considered = self.considered();
		match degraded.as_slice() {
			[] => format!("{check}: no instance is degraded"),
			[one] if considered == 1 => format!("{check} is {} for {}", one.effective, one.name()),
			many => format!(
				"{check} is degraded for {} of {considered} instances: {}",
				many.len(),
				many.iter()
					.map(|i| format!("{} ({})", i.name(), i.effective))
					.collect::<Vec<_>>()
					.join(", "),
			),
		}
	}
}

/// A check state's instances (`issues.instances`), by key, as
/// [`GradedCheck::stored_instances`] writes them. A state holds them only if
/// its check has instances; a plain check's state has none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoredInstances(pub BTreeMap<String, StoredInstance>);

/// One instance as a check's state stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredInstance {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	pub observed: CheckResult,
	pub effective: CheckResult,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub detail: Option<Value>,
}

impl StoredInstances {
	/// How many instances are neither passed nor skipped.
	pub fn degraded(&self) -> usize {
		self.0
			.values()
			.filter(|i| !matches!(i.effective, CheckResult::Passed | CheckResult::Skipped))
			.count()
	}

	/// The instances as last observed, for grading them again (an instance
	/// silence set or lifted since).
	///
	/// A broken check's instances were observed broken; they come back as
	/// such, for the caller to re-grade as [`CheckOutcome::Broken`].
	pub fn observed_instances(&self) -> Vec<CheckInstance> {
		self.0
			.iter()
			.map(|(key, i)| CheckInstance {
				key: key.clone(),
				label: i.label.clone(),
				observed: i.observed,
				detail: i.detail.clone(),
			})
			.collect()
	}
}

/// A check's instances and the inputs they were graded with, as a check
/// state keeps them.
#[derive(Debug, Clone, PartialEq)]
pub struct InstancedState {
	pub instances: StoredInstances,
	pub inputs: GradingInputs,
}

/// What the rules grading a check's instances read beyond each instance: the
/// report's fields and the target's tags, as the filing gave them
/// (`issues.grading_context`).
///
/// A state with instances keeps the inputs its last filing graded them with,
/// so re-grading it after an instance silence changes nothing a rule reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GradingInputs {
	/// What rules read as `status.<field>`.
	#[serde(default)]
	pub status: Map<String, Value>,
	/// What rules read as `tag.<key>`.
	#[serde(default)]
	pub tags: HashMap<String, Value>,
}

impl GradingInputs {
	/// The inputs a filing grades with, kept.
	pub fn of(ctx: &GradingContext<'_>) -> Self {
		Self {
			status: ctx.status_extra.clone(),
			tags: ctx.tags.clone(),
		}
	}

	/// These inputs as a rule context for `source`'s `check`.
	pub fn context<'a>(&'a self, source: &'a str, check: &'a str) -> GradingContext<'a> {
		GradingContext {
			source,
			check,
			status_extra: &self.status,
			tags: &self.tags,
		}
	}
}

/// Grade a check through its instances.
///
/// Each instance is graded through the check's whole policy, its catalog entry
/// and then every scoped transform that applies to it: those with no instance
/// key apply to every instance, one with a key only to the instance with that
/// key. A rule evaluated for an instance reads the instance's fields merged
/// over the check's `shared` ones (the instance's winning), the instance's own
/// observed result as `check.result`, the report's fields as `status.*`, and
/// the target's tags. It never reads the key.
///
/// The check settles on the most urgent instance that was not skipped, and is
/// skipped when all of them were; a skipped instance is silenced rather than
/// healthy, so it neither drags the check down nor props it up. A check with no
/// instances passes.
///
/// A broken check (see [`CheckOutcome::Broken`]) is graded as broken in every
/// instance its stored state `prior` holds, each with the fields it was last
/// stored with; a check that held none is graded as its single plain
/// instance. Where policy leaves it broken, it retains the contribution of
/// its last definite result: an open effective failure stays a failure, with
/// the escalation it had, and anything else is broken, which counts as a
/// warning. `prior` is only read for a broken check.
// spec: CHK#checks-with-instances
// spec: CHK#stability
pub fn grade_instances(
	grading: &CheckGrading,
	ctx: &GradingContext<'_>,
	shared: Option<&Map<String, Value>>,
	outcome: &CheckOutcome,
	prior: Option<&Issue>,
) -> GradedCheck {
	let (instances, broken) = match outcome {
		CheckOutcome::Instances(instances) => {
			debug_assert!(
				instances.iter().all(|i| i.observed != CheckResult::Broken),
				"brokenness is the whole check's: file CheckOutcome::Broken rather than a broken instance",
			);
			debug_assert!(
				{
					let mut keys: Vec<&str> = instances.iter().map(|i| i.key.as_str()).collect();
					keys.sort_unstable();
					keys.windows(2).all(|w| w[0] != w[1])
				},
				"an instance key is unique within its check",
			);
			(instances.clone(), false)
		}
		CheckOutcome::Broken => {
			let held = prior
				.and_then(Issue::stored_instances)
				.map(|held| held.observed_instances())
				.unwrap_or_default();
			let held = if held.is_empty() {
				vec![CheckInstance::plain(CheckResult::Broken, None)]
			} else {
				held.into_iter()
					.map(|i| CheckInstance {
						observed: CheckResult::Broken,
						..i
					})
					.collect()
			};
			(held, true)
		}
	};

	let mut graded_instances = Vec::with_capacity(instances.len());
	let mut escalates = false;
	for instance in instances {
		let mut check_extra = merged(shared, instance.detail.as_ref());
		check_extra.insert(
			"result".into(),
			Value::String(instance.observed.to_string()),
		);
		let eval = EvaluationContext {
			status_extra: ctx.status_extra,
			check_extra: &check_extra,
			tags: ctx.tags,
		};
		let fleet = CheckPolicy::grade(
			grading.fleet.as_ref(),
			ctx.source,
			ctx.check,
			instance.observed,
			&eval,
		);
		let graded =
			CheckPolicy::chain_scoped_for_instance(fleet, &grading.chain, &instance.key, &eval);
		if graded.effective != CheckResult::Skipped {
			escalates |= graded.escalates;
		}
		graded_instances.push(GradedInstance {
			key: instance.key,
			label: instance.label,
			observed: instance.observed,
			effective: graded.effective,
			detail: instance.detail,
		});
	}

	let most_urgent =
		|results: &mut dyn Iterator<Item = CheckResult>| results.min_by_key(|r| r.urgency_rank());
	let (observed, mut effective) = if graded_instances.is_empty() {
		(CheckResult::Passed, CheckResult::Passed)
	} else {
		(
			most_urgent(&mut graded_instances.iter().map(|i| i.observed))
				.unwrap_or(CheckResult::Skipped),
			most_urgent(
				&mut graded_instances
					.iter()
					.map(|i| i.effective)
					.filter(|r| *r != CheckResult::Skipped),
			)
			.unwrap_or(CheckResult::Skipped),
		)
	};

	// An effective broken result neither confirms nor clears the previous
	// definite one: an open failure keeps contributing as a failure.
	// spec: CHK#stability
	if broken
		&& effective == CheckResult::Broken
		&& let Some(prior) =
			prior.filter(|p| p.active && p.effective_result == Some(CheckResult::Failed))
	{
		effective = CheckResult::Failed;
		escalates = prior.escalates;
	}

	GradedCheck {
		observed,
		effective,
		escalates,
		instances: graded_instances,
		shared: shared.cloned(),
		broken,
	}
}

/// An instance's fields merged over the check's shared ones, the instance's
/// winning where both carry a field. A detail that is not an object carries
/// no fields a rule can name.
fn merged(shared: Option<&Map<String, Value>>, own: Option<&Value>) -> Map<String, Value> {
	let mut fields = shared.cloned().unwrap_or_default();
	if let Some(Value::Object(own)) = own {
		for (k, v) in own {
			fields.insert(k.clone(), v.clone());
		}
	}
	fields
}
