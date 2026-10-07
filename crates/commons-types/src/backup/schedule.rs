//! Backup schedules: manual-only, an interval, or a cron expression read in a
//! timezone.
//!
//! This is the pure part of scheduling. It knows what makes an expression
//! acceptable, which instants it fires at once read in a zone, how a firing
//! becomes a due window, and how a machine's zone is worked out. It reads no
//! database; the layers a schedule resolves through, and when it took effect,
//! are the database crate's to assemble into an [`EffectiveSchedule`].

use std::fmt;

use cronexpr::{CronTimesIter, Crontab, MakeTimestamp};
use jiff::{SignedDuration, Timestamp, Zoned, tz::TimeZone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod windows_zones;

/// The shortest interval, and the shortest gap between two cron firings.
// spec: BKO#scheduling
pub const MIN_GAP: SignedDuration = SignedDuration::from_hours(1);

/// How far into its window a machine's backup may be delayed to spread a group
/// sharing one schedule, before being cut down to a quarter of the window.
const SPREAD_CAP: SignedDuration = SignedDuration::from_mins(15);

/// How far back, in days, to look for the firings before a moment, trying the
/// shortest first so a dense expression doesn't walk years of firings.
const LOOKBACK_DAYS: [i64; 6] = [2, 14, 62, 400, 1500, 3000];

/// An expression's firings are checked from this moment: the start of a leap
/// year, so a 29 February expression fires within the four years searched.
const VALIDATION_START: &str = "2024-01-01T00:00:00Z";

/// The longest expression accepted. A fully spelled-out minute list fits with
/// room to spare; the bound keeps validation, which reparses the expression
/// for every value `H` could take, cheap.
const MAX_EXPRESSION_LEN: usize = 512;

/// What a schedule can be refused for.
// spec: BKO#cron-expressions
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
	#[error("an interval must be at least an hour")]
	IntervalTooShort,
	#[error("a cron schedule needs an expression")]
	EmptyExpression,
	#[error("a cron expression is at most {MAX_EXPRESSION_LEN} characters")]
	TooLong,
	#[error(
		"a cron expression has five fields (minute, hour, day of month, month, day of week), \
		 but this one has {0}"
	)]
	FieldCount(usize),
	#[error("the timezone is set separately, so the expression must have only the five fields")]
	ZoneField,
	#[error("`H` is only accepted as a whole field on its own, but the {field} field is `{value}`")]
	HashMisused { field: &'static str, value: String },
	#[error("{0}")]
	Syntax(String),
	#[error("this expression never fires")]
	NeverFires,
	#[error("this expression fires less than an hour apart (every {minutes} minutes)")]
	TooFrequent { minutes: i64 },
	#[error("unknown timezone `{0}`")]
	UnknownZone(String),
}

/// What a schedule is, as an operator sets it at one layer.
// spec: BKO#scheduling
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
	/// Backed up only on an explicit request.
	Manual,
	/// Backed up once this many seconds have passed since the last success.
	Interval {
		#[schema(format = "int64")]
		seconds: i64,
	},
	/// Backed up in the windows a cron expression opens, read in `zone` when
	/// given, otherwise in the machine's own timezone.
	Cron {
		/// Five fields: minute, hour, day of month, month, day of week.
		expression: String,
		/// An IANA timezone name.
		zone: Option<String>,
	},
}

impl Schedule {
	/// Read a schedule out of the three columns every layer stores it in. A
	/// layer holds at most one of an interval and a cron expression, and
	/// neither means manual-only.
	pub fn from_columns(
		interval_secs: Option<i64>,
		cron: Option<String>,
		zone: Option<String>,
	) -> Self {
		match (interval_secs, cron) {
			(_, Some(expression)) => Self::Cron { expression, zone },
			(Some(seconds), None) => Self::Interval { seconds },
			(None, None) => Self::Manual,
		}
	}

	/// The columns a layer stores this in: interval seconds, cron expression,
	/// zone. The zone is only stored beside a cron expression.
	pub fn to_columns(&self) -> (Option<i64>, Option<String>, Option<String>) {
		match self {
			Self::Manual => (None, None, None),
			Self::Interval { seconds } => (Some(*seconds), None, None),
			Self::Cron { expression, zone } => (None, Some(expression.clone()), zone.clone()),
		}
	}

	/// Refuse a schedule that cannot be run, saying why.
	pub fn validate(&self) -> Result<(), ScheduleError> {
		match self {
			Self::Manual => Ok(()),
			Self::Interval { seconds } if *seconds < MIN_GAP.as_secs() => {
				Err(ScheduleError::IntervalTooShort)
			}
			Self::Interval { .. } => Ok(()),
			Self::Cron { expression, zone } => {
				CronExpr::parse(expression)?;
				if let Some(zone) = zone {
					TimeZone::get(zone).map_err(|_| ScheduleError::UnknownZone(zone.clone()))?;
				}
				Ok(())
			}
		}
	}
}

/// Which layer a machine's schedule for a type comes from.
// spec: BKO#scheduling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleLayer {
	/// The machine's own override.
	Machine,
	/// The `(group, type)` override.
	Group,
	/// The fleet-wide default for the type.
	Fleet,
}

impl ScheduleLayer {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Machine => "machine",
			Self::Group => "group",
			Self::Fleet => "fleet",
		}
	}
}

/// Where the zone a cron schedule is read in came from.
// spec: BKO#cron-expressions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZoneSource {
	/// Named by the schedule itself.
	Schedule,
	/// The operating system timezone the machine reports.
	Machine,
	/// UTC, because the machine has reported no timezone.
	UnreportedUtc,
	/// UTC, because the machine reported a timezone Canopy does not recognise.
	UnrecognisedUtc,
}

impl ZoneSource {
	/// Whether this is UTC by default rather than by choice, which an operator
	/// is told about.
	pub fn is_fallback(self) -> bool {
		matches!(self, Self::UnreportedUtc | Self::UnrecognisedUtc)
	}
}

/// A zone a cron schedule is read in, and how it was settled on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ZoneUsed {
	/// An IANA timezone name.
	pub name: String,
	/// Where the zone came from, which says whether UTC is a choice or a
	/// fallback.
	pub source: ZoneSource,
}

/// Settle the zone to read a cron schedule in: the schedule's own, else the
/// machine's reported one (read as its IANA equivalent when it is a Windows
/// zone name), else UTC.
// spec: BKO#cron-expressions
pub fn resolve_zone(schedule_zone: Option<&str>, reported: Option<&str>) -> ZoneUsed {
	let utc = |source| ZoneUsed {
		name: "UTC".into(),
		source,
	};
	if let Some(zone) = schedule_zone {
		return if TimeZone::get(zone).is_ok() {
			ZoneUsed {
				name: zone.into(),
				source: ZoneSource::Schedule,
			}
		} else {
			utc(ZoneSource::UnrecognisedUtc)
		};
	}
	let Some(reported) = reported.map(str::trim).filter(|r| !r.is_empty()) else {
		return utc(ZoneSource::UnreportedUtc);
	};
	if TimeZone::get(reported).is_ok() {
		return ZoneUsed {
			name: reported.into(),
			source: ZoneSource::Machine,
		};
	}
	match windows_to_iana(reported) {
		Some(iana) if TimeZone::get(iana).is_ok() => ZoneUsed {
			name: iana.into(),
			source: ZoneSource::Machine,
		},
		_ => utc(ZoneSource::UnrecognisedUtc),
	}
}

/// The IANA zone a Windows zone name stands for, matched without regard to
/// case.
pub fn windows_to_iana(name: &str) -> Option<&'static str> {
	windows_zones::WINDOWS_TO_IANA
		.iter()
		.find(|(windows, _)| windows.eq_ignore_ascii_case(name))
		.map(|(_, iana)| *iana)
}

/// What a machine's schedule for one type resolved to.
// spec: BKO#scheduling
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EffectiveSchedule {
	/// What the machine follows: manual-only, an interval, or a cron
	/// expression.
	pub schedule: Schedule,
	/// The layer it comes from; none when no layer sets one, which is
	/// manual-only.
	pub layer: Option<ScheduleLayer>,
	/// The zone it is read in, for a cron schedule.
	pub zone: Option<ZoneUsed>,
	/// When this schedule took effect: a firing from before then opens no
	/// window. None when it has always applied.
	pub since: Option<Timestamp>,
	/// When a change to a layer last altered what this resolves to. Unlike
	/// `since`, a change in the timezone the machine reports doesn't move it,
	/// so it bounds which firings count as missed: a zone change moves when
	/// firings fall, not how many a machine has missed.
	pub layers_since: Option<Timestamp>,
}

impl EffectiveSchedule {
	pub fn manual() -> Self {
		Self {
			schedule: Schedule::Manual,
			layer: None,
			zone: None,
			since: None,
			layers_since: None,
		}
	}

	/// The expression made runnable for one machine's type, when this is a cron
	/// schedule. An error is a stored expression that can no longer be read.
	///
	/// The expression was validated when it was set, so it is only read here,
	/// not validated again: validation tries every value `H` could take.
	pub fn bind(&self, seed: u64) -> Option<Result<BoundCron, ScheduleError>> {
		let Schedule::Cron { expression, .. } = &self.schedule else {
			return None;
		};
		let zone = self.zone.as_ref().map_or("UTC", |z| z.name.as_str());
		Some(CronExpr::read(expression).and_then(|expr| expr.bind(seed, zone)))
	}

	/// Whether the backup is due now, given when the last success's snapshot
	/// was taken.
	// spec: BKO#when-a-backup-is-due
	pub fn is_due(&self, seed: u64, now: Timestamp, last_success: Option<Timestamp>) -> bool {
		match &self.schedule {
			Schedule::Manual => false,
			Schedule::Interval { seconds } => interval_due(*seconds, now, last_success),
			Schedule::Cron { .. } => self
				.bind(seed)
				.and_then(Result::ok)
				.and_then(|cron| cron.window_at(now, self.since))
				.is_some_and(|w| w.is_due(now, last_success)),
		}
	}

	/// What to show as the next scheduled backup.
	// spec: BKO#editing-schedules
	pub fn next_backup(
		&self,
		seed: u64,
		now: Timestamp,
		last_success: Option<Timestamp>,
	) -> NextBackup {
		match &self.schedule {
			Schedule::Manual => NextBackup::Manual,
			Schedule::Interval { seconds } => match last_success {
				Some(last) => match last.checked_add(SignedDuration::from_secs(*seconds)) {
					Ok(at) if at > now => NextBackup::At(at),
					_ => NextBackup::DueNow,
				},
				None => NextBackup::DueNow,
			},
			Schedule::Cron { .. } => {
				let Some(Ok(cron)) = self.bind(seed) else {
					return NextBackup::Unreadable;
				};
				if let Some(window) = cron.window_at(now, self.since)
					&& now < window.closes
					&& last_success.is_none_or(|s| s < window.firing)
				{
					return if now >= window.opens {
						NextBackup::DueUntil(window.closes)
					} else {
						NextBackup::At(window.opens)
					};
				}
				match cron
					.firings_after(now)
					.find(|f| self.since.is_none_or(|s| *f >= s))
				{
					Some(next) => NextBackup::At(next),
					None => NextBackup::Manual,
				}
			}
		}
	}

	/// Whether the two most recent firings whose windows have closed both went
	/// without a successful backup since the earlier one. `began` bounds which
	/// firings count, besides the moment a layer last changed the schedule. An
	/// expression that can no longer be read has missed them all.
	// spec: BKJ#detection
	pub fn missed_two_firings(
		&self,
		seed: u64,
		now: Timestamp,
		last_success: Option<Timestamp>,
		began: Option<Timestamp>,
	) -> bool {
		let cron = match self.bind(seed) {
			None => return false,
			Some(Err(_)) => return true,
			Some(Ok(cron)) => cron,
		};
		let not_before = match (self.layers_since, began) {
			(Some(a), Some(b)) => Some(a.max(b)),
			(a, b) => a.or(b),
		};
		let closed = cron.closed_firings(now, not_before, 2);
		match closed.as_slice() {
			[earlier, _] => last_success.is_none_or(|s| s < *earlier),
			_ => false,
		}
	}
}

fn interval_due(seconds: i64, now: Timestamp, last_success: Option<Timestamp>) -> bool {
	match last_success {
		None => true,
		Some(last) => now.duration_since(last) >= SignedDuration::from_secs(seconds),
	}
}

/// When a machine's next backup of a type is expected.
// spec: BKO#editing-schedules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", content = "at", rename_all = "snake_case")]
pub enum NextBackup {
	/// Backed up only on request.
	Manual,
	/// Due now, with no end to the window.
	DueNow,
	/// A window is open and the backup is due until it closes.
	DueUntil(Timestamp),
	/// The next firing, or when the interval next elapses.
	At(Timestamp),
	/// A stored cron expression that can no longer be read, so nothing runs.
	Unreadable,
}

/// The window a firing opens: the backup is due from `opens` until `closes`,
/// halfway to the next firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Window {
	pub firing: Timestamp,
	pub opens: Timestamp,
	pub closes: Timestamp,
}

impl Window {
	/// Due while open and while no backup has succeeded since the firing, so a
	/// failed run is retried within the window.
	// spec: BKO#when-a-backup-is-due
	pub fn is_due(&self, now: Timestamp, last_success: Option<Timestamp>) -> bool {
		self.opens <= now && now < self.closes && last_success.is_none_or(|s| s < self.firing)
	}
}

/// A stable seed for one `(machine, type)`, so `H` and the spread offset land
/// the same every time and in every process.
pub fn schedule_seed(machine_id: Uuid, backup_type: &str) -> u64 {
	let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
	for byte in machine_id.as_bytes().iter().chain(backup_type.as_bytes()) {
		hash ^= u64::from(*byte);
		hash = hash.wrapping_mul(0x1000_0000_01b3);
	}
	hash
}

/// SplitMix64 finaliser, to give each field its own value from one seed.
fn mix(seed: u64, salt: u64) -> u64 {
	let mut z = seed.wrapping_add(salt.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
	z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
	z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
	z ^ (z >> 31)
}

/// Each cron field's name and inclusive range, in expression order. Day of
/// week runs 0 to 7, both ends being Sunday.
const FIELDS: [(&str, u8, u8); 5] = [
	("minute", 0, 59),
	("hour", 0, 23),
	("day of month", 1, 31),
	("month", 1, 12),
	("day of week", 0, 7),
];

const DAY_FIELDS: [usize; 3] = [2, 3, 4];

/// A cron expression an operator wrote: five fields, with no timezone, which
/// is its own setting.
///
/// Parsing accepts what [`cronexpr`] does (Vixie day-field semantics, names,
/// `L`, `W`, `5L`, `5#3`) with the addition of `H`, which stands for a single
/// value Canopy derives per machine and type. `H` is only a whole field.
// spec: BKO#cron-expressions
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
	fields: [String; 5],
}

impl CronExpr {
	/// Parse and validate an expression: it must be well formed, ever fire,
	/// and never fire twice within an hour, for every value `H` could take.
	pub fn parse(input: &str) -> Result<Self, ScheduleError> {
		let expr = Self::read(input)?;
		expr.check_fires()?;
		expr.check_gaps()?;
		Ok(expr)
	}

	/// Split an expression into its fields and check where it uses `H`,
	/// without checking when it fires: for one already validated.
	pub fn read(input: &str) -> Result<Self, ScheduleError> {
		if input.len() > MAX_EXPRESSION_LEN {
			return Err(ScheduleError::TooLong);
		}
		let fields: Vec<&str> = input.split_whitespace().collect();
		match fields.len() {
			0 => return Err(ScheduleError::EmptyExpression),
			5 => {}
			6 => return Err(ScheduleError::ZoneField),
			n => return Err(ScheduleError::FieldCount(n)),
		}
		let expr = Self {
			fields: std::array::from_fn(|i| fields[i].to_owned()),
		};
		for (i, field) in expr.fields.iter().enumerate() {
			if matches!(hash_use(field), HashUse::Misused) {
				return Err(ScheduleError::HashMisused {
					field: FIELDS[i].0,
					value: field.clone(),
				});
			}
		}
		Ok(expr)
	}

	/// The expression as stored: its five fields, single-spaced.
	pub fn as_string(&self) -> String {
		self.fields.join(" ")
	}

	/// Whether any field is `H`.
	pub fn has_hash(&self) -> bool {
		self.fields.iter().any(|f| f == "H")
	}

	/// Substitute each `H` with the value `pick` gives for its field.
	fn resolve(&self, pick: impl Fn(usize, u8, u8) -> u8) -> String {
		self.fields
			.iter()
			.enumerate()
			.map(|(i, f)| {
				if f == "H" {
					pick(i, FIELDS[i].1, FIELDS[i].2).to_string()
				} else {
					f.clone()
				}
			})
			.collect::<Vec<_>>()
			.join(" ")
	}

	/// Fires at least once, whatever `H` is. Only the month and day fields
	/// decide that, so the minute and hour are pinned and every combination of
	/// the others is tried.
	fn check_fires(&self) -> Result<(), ScheduleError> {
		let hashed: Vec<usize> = DAY_FIELDS
			.into_iter()
			.filter(|i| self.fields[*i] == "H")
			.collect();
		let mut values: Vec<u8> = hashed.iter().map(|i| FIELDS[*i].1).collect();
		loop {
			let text = self.resolve(|i, lo, _| {
				hashed
					.iter()
					.position(|h| *h == i)
					.map_or(lo, |at| values[at])
			});
			let crontab = cronexpr::parse_crontab(&format!("{text} UTC"))
				.map_err(|e| ScheduleError::Syntax(e.to_string()))?;
			if crontab.find_next(VALIDATION_START).is_err() {
				return Err(ScheduleError::NeverFires);
			}
			// Step the odometer; done once every field has wrapped.
			let mut at = 0;
			loop {
				let Some(&field) = hashed.get(at) else {
					return Ok(());
				};
				if values[at] < FIELDS[field].2 {
					values[at] += 1;
					break;
				}
				values[at] = FIELDS[field].1;
				at += 1;
			}
		}
	}

	/// No two consecutive firings less than an hour apart.
	///
	/// That depends only on the minute and hour fields, and on a clock without
	/// daylight-saving changes. With one minute value there is at most one
	/// firing an hour, so hourly is the fastest; with two or more, any hour
	/// that fires holds two firings under an hour apart. Rather than reason it
	/// out in the open, read two days of firings with the day fields opened up,
	/// which can only add firings, never remove a gap.
	fn check_gaps(&self) -> Result<(), ScheduleError> {
		let text = self.resolve(|_, lo, _| lo);
		let parts: Vec<&str> = text.split(' ').collect();
		let crontab = cronexpr::parse_crontab(&format!("{} {} * * * UTC", parts[0], parts[1]))
			.map_err(|e| ScheduleError::Syntax(e.to_string()))?;
		let start: Timestamp = VALIDATION_START.parse().expect("constant timestamp");
		let end = start + SignedDuration::from_hours(48);
		let mut previous: Option<Timestamp> = None;
		for firing in crontab
			.iter_after(MakeTimestamp(start))
			.into_iter()
			.flatten()
			.map_while(Result::ok)
		{
			let at = firing.timestamp();
			if at > end {
				break;
			}
			if let Some(prev) = previous {
				let gap = at.duration_since(prev);
				if gap < MIN_GAP {
					return Err(ScheduleError::TooFrequent {
						minutes: gap.as_mins(),
					});
				}
			}
			previous = Some(at);
		}
		Ok(())
	}

	/// Make the expression runnable for one machine and type, reading it in
	/// `zone`, an IANA name. `H` takes the value `seed` derives for its field.
	pub fn bind(&self, seed: u64, zone: &str) -> Result<BoundCron, ScheduleError> {
		TimeZone::get(zone).map_err(|_| ScheduleError::UnknownZone(zone.to_owned()))?;
		let text = self.resolve(|i, lo, hi| {
			let len = u64::from(hi - lo) + 1;
			lo + (mix(seed, i as u64) % len) as u8
		});
		let crontab = cronexpr::parse_crontab(&format!("{text} {zone}"))
			.map_err(|e| ScheduleError::Syntax(e.to_string()))?;
		Ok(BoundCron {
			crontab,
			seed,
			spread: !self.has_hash(),
		})
	}
}

#[derive(Debug, PartialEq, Eq)]
enum HashUse {
	None,
	Lone,
	Misused,
}

/// Whether a field uses `H`, and whether it does so as the whole field. A
/// weekday or month name that merely contains an `H` doesn't count.
fn hash_use(field: &str) -> HashUse {
	if field == "H" {
		return HashUse::Lone;
	}
	let starts_with_h = field
		.split([',', '-', '/'])
		.any(|token| token.starts_with(['H', 'h']));
	if starts_with_h {
		HashUse::Misused
	} else {
		HashUse::None
	}
}

/// An expression made runnable for one machine's type, in one zone.
#[derive(Debug, Clone)]
pub struct BoundCron {
	crontab: Crontab,
	seed: u64,
	/// Whether each machine's window opens a stable moment after the firing,
	/// which an expression doing its own spreading with `H` does not.
	spread: bool,
}

impl BoundCron {
	/// The firings after `after`, in order, skipping any that fall less than an
	/// hour after the one kept before, or that repeat its wall-clock time as
	/// the clocks go back. A skipped firing opens no window and doesn't bound
	/// the window of the one before.
	// spec: BKO#cron-expressions
	pub fn firings_after(&self, after: Timestamp) -> Firings {
		// Start a little early so the firing before `after` is seen and the
		// filter has what it needs to judge the first one.
		let start = after
			.checked_sub(SignedDuration::from_hours(2))
			.unwrap_or(after);
		Firings {
			inner: self.crontab.iter_after(MakeTimestamp(start)).ok(),
			previous: None,
			after,
		}
	}

	/// The window of the latest firing at or before `now`, which is the only
	/// one that can be open. Firings before `not_before` open none.
	pub fn window_at(&self, now: Timestamp, not_before: Option<Timestamp>) -> Option<Window> {
		let (past, next) = self.surround(now, 1, not_before);
		Some(self.window(*past.last()?, next?))
	}

	/// The latest `count` firings whose windows have closed by `now`, oldest
	/// first.
	pub fn closed_firings(
		&self,
		now: Timestamp,
		not_before: Option<Timestamp>,
		count: usize,
	) -> Vec<Timestamp> {
		let (past, next) = self.surround(now, count + 1, not_before);
		let mut closed = Vec::new();
		for (i, firing) in past.iter().enumerate() {
			let Some(following) = past.get(i + 1).copied().or(next) else {
				break;
			};
			if self.window(*firing, following).closes <= now {
				closed.push(*firing);
			}
		}
		let skip = closed.len().saturating_sub(count);
		closed.split_off(skip)
	}

	/// The window a firing opens, given the firing after it. It closes halfway
	/// to that one. Without `H` it opens a stable distance after the firing,
	/// kept small against the window.
	// spec: BKO#when-a-backup-is-due
	pub fn window(&self, firing: Timestamp, next: Timestamp) -> Window {
		let half = next.duration_since(firing) / 2;
		let offset = if self.spread {
			let cap = SPREAD_CAP.min(half / 4).as_secs();
			if cap > 0 {
				SignedDuration::from_secs((mix(self.seed, 99) % (cap as u64 + 1)) as i64)
			} else {
				SignedDuration::ZERO
			}
		} else {
			SignedDuration::ZERO
		};
		Window {
			firing,
			opens: firing + offset,
			closes: firing + half,
		}
	}

	/// Up to `want` firings at or before `now`, newest last, none before
	/// `not_before`, and the first firing after `now`.
	fn surround(
		&self,
		now: Timestamp,
		want: usize,
		not_before: Option<Timestamp>,
	) -> (Vec<Timestamp>, Option<Timestamp>) {
		let floor = not_before.and_then(|nb| nb.checked_sub(SignedDuration::from_secs(1)).ok());
		let mut result = (Vec::new(), None);
		for days in LOOKBACK_DAYS {
			let back = now
				.checked_sub(SignedDuration::from_hours(24 * days))
				.unwrap_or(Timestamp::MIN);
			let (after, at_floor) = match floor {
				Some(f) if f >= back => (f, true),
				_ => (back, false),
			};
			let mut past: Vec<Timestamp> = Vec::new();
			let mut next = None;
			for firing in self.firings_after(after) {
				if firing > now {
					next = Some(firing);
					break;
				}
				past.push(firing);
			}
			let skip = past.len().saturating_sub(want);
			let past = past.split_off(skip);
			let enough = past.len() >= want;
			result = (past, next);
			if enough || at_floor {
				break;
			}
		}
		result
	}
}

/// The firings of a [`BoundCron`] after a moment.
pub struct Firings {
	inner: Option<CronTimesIter>,
	previous: Option<Zoned>,
	after: Timestamp,
}

impl Iterator for Firings {
	type Item = Timestamp;

	fn next(&mut self) -> Option<Timestamp> {
		loop {
			let firing = self.inner.as_mut()?.next()?.ok()?;
			if let Some(previous) = &self.previous
				&& (firing.timestamp().duration_since(previous.timestamp()) < MIN_GAP
					|| firing.datetime() == previous.datetime())
			{
				continue;
			}
			let at = firing.timestamp();
			self.previous = Some(firing);
			if at > self.after {
				return Some(at);
			}
		}
	}
}

impl fmt::Debug for Firings {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Firings")
			.field("after", &self.after)
			.finish_non_exhaustive()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ts(s: &str) -> Timestamp {
		s.parse().unwrap()
	}

	fn cron(expr: &str, zone: &str, seed: u64) -> BoundCron {
		CronExpr::parse(expr).unwrap().bind(seed, zone).unwrap()
	}

	fn local(at: Timestamp, zone: &str) -> String {
		at.in_tz(zone)
			.unwrap()
			.strftime("%Y-%m-%d %H:%M")
			.to_string()
	}

	fn firings(c: &BoundCron, after: &str, n: usize) -> Vec<Timestamp> {
		c.firings_after(ts(after)).take(n).collect()
	}

	fn refusal(expr: &str) -> ScheduleError {
		CronExpr::parse(expr).unwrap_err()
	}

	#[test]
	fn accepts_the_grammar_operators_write() {
		for expr in [
			"0 2 * * *",
			"30 1 * * mon-fri",
			"0 2 * * SUN",
			"0 2 * JAN,JUL 1",
			"0 2 L * *",
			"0 2 15W * *",
			"0 2 * * 5L",
			"0 2 * * 5#3",
			"0 */6 * * *",
			"15 2 * * 0",
			"15 2 * * 7",
			"  0   2  *  *  *  ",
		] {
			CronExpr::parse(expr).unwrap_or_else(|e| panic!("`{expr}` refused: {e}"));
		}
	}

	#[test]
	fn day_fields_follow_vixie_semantics() {
		let tuesdays = cron("0 12 * * 2", "UTC", 1);
		let daily = cron("0 12 1-31 * 2", "UTC", 1);
		let week = firings(&tuesdays, "2024-09-23T00:00:00Z", 3);
		assert_eq!(
			week.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
			[
				"2024-09-24T12:00:00Z",
				"2024-10-01T12:00:00Z",
				"2024-10-08T12:00:00Z"
			]
		);
		let days = firings(&daily, "2024-09-23T00:00:00Z", 3);
		assert_eq!(days[1], ts("2024-09-24T12:00:00Z"));
		assert_eq!(days[2], ts("2024-09-25T12:00:00Z"));
	}

	#[test]
	fn hash_is_a_whole_field_only() {
		for expr in [
			"H 2 * * *",
			"0 H * * *",
			"0 2 H * *",
			"0 2 * H *",
			"0 2 * * H",
			"H H H H H",
		] {
			CronExpr::parse(expr).unwrap_or_else(|e| panic!("`{expr}` refused: {e}"));
		}
		for expr in [
			"H/15 * * * *",
			"H(0-5) * * * *",
			"0 H,3 * * *",
			"0 2 * * 1,H",
			"0 1-H * * *",
		] {
			assert!(
				matches!(refusal(expr), ScheduleError::HashMisused { .. }),
				"`{expr}` should be refused for its use of H"
			);
		}
	}

	#[test]
	fn weekday_names_containing_an_h_are_not_hash() {
		CronExpr::parse("0 2 * * THU").unwrap();
		CronExpr::parse("0 2 * * THU-SAT").unwrap();
	}

	#[test]
	fn hash_is_stable_per_seed_and_differs_across_seeds() {
		let expr = CronExpr::parse("H H * * *").unwrap();
		let at = |seed| {
			expr.bind(seed, "UTC")
				.unwrap()
				.firings_after(ts("2024-09-23T00:00:00Z"))
				.next()
				.unwrap()
		};
		assert_eq!(at(7), at(7));
		let distinct: std::collections::HashSet<_> = (0..40).map(at).collect();
		assert!(distinct.len() > 10, "40 machines should spread out");
	}

	#[test]
	fn seeds_differ_by_machine_and_by_type() {
		let a = Uuid::from_u128(1);
		let b = Uuid::from_u128(2);
		assert_eq!(
			schedule_seed(a, "tamanu-postgres"),
			schedule_seed(a, "tamanu-postgres")
		);
		assert_ne!(
			schedule_seed(a, "tamanu-postgres"),
			schedule_seed(b, "tamanu-postgres")
		);
		assert_ne!(
			schedule_seed(a, "tamanu-postgres"),
			schedule_seed(a, "tamanu-files")
		);
	}

	#[test]
	fn an_expression_that_never_fires_is_refused() {
		assert_eq!(refusal("0 0 30 2 *"), ScheduleError::NeverFires);
		assert_eq!(refusal("0 0 31 4,6,9,11 *"), ScheduleError::NeverFires);
		// Some value of H lands on a day February doesn't have.
		assert_eq!(refusal("0 0 H 2 *"), ScheduleError::NeverFires);
		// A 29 February expression does fire, in leap years.
		CronExpr::parse("0 0 29 2 *").unwrap();
	}

	#[test]
	fn an_expression_firing_within_the_hour_is_refused() {
		assert!(matches!(
			refusal("*/30 * * * *"),
			ScheduleError::TooFrequent { minutes: 30 }
		));
		assert!(matches!(
			refusal("* * * * *"),
			ScheduleError::TooFrequent { .. }
		));
		assert!(matches!(
			refusal("0,5 2 * * *"),
			ScheduleError::TooFrequent { minutes: 5 }
		));
		assert!(matches!(
			refusal("*/15 H * * *"),
			ScheduleError::TooFrequent { .. }
		));
		// Hourly is the fastest there is.
		CronExpr::parse("0 * * * *").unwrap();
		CronExpr::parse("H * * * *").unwrap();
		CronExpr::parse("45 */2 * * *").unwrap();
	}

	#[test]
	fn an_overlong_expression_is_refused_before_it_is_checked() {
		let minutes = vec!["0"; 300].join(",");
		assert_eq!(
			refusal(&format!("{minutes} 2 H H H")),
			ScheduleError::TooLong
		);
		let every_minute_spelled_out = (0..60).map(|m| m.to_string()).collect::<Vec<_>>();
		assert!(CronExpr::read(&format!("{} 2 * * *", every_minute_spelled_out.join(","))).is_ok());
	}

	#[test]
	fn a_timezone_field_is_refused() {
		assert_eq!(
			refusal("0 2 * * * Pacific/Auckland"),
			ScheduleError::ZoneField
		);
		assert!(matches!(refusal("0 2 * *"), ScheduleError::FieldCount(4)));
		assert_eq!(refusal("   "), ScheduleError::EmptyExpression);
	}

	#[test]
	fn a_malformed_expression_says_so() {
		assert!(matches!(refusal("0 25 * * *"), ScheduleError::Syntax(_)));
		assert!(matches!(refusal("nope 2 * * *"), ScheduleError::Syntax(_)));
	}

	#[test]
	fn schedules_validate() {
		assert!(Schedule::Manual.validate().is_ok());
		assert!(Schedule::Interval { seconds: 3600 }.validate().is_ok());
		assert_eq!(
			Schedule::Interval { seconds: 3599 }.validate(),
			Err(ScheduleError::IntervalTooShort)
		);
		assert_eq!(
			Schedule::Cron {
				expression: "0 2 * * *".into(),
				zone: Some("Mars/Olympus".into())
			}
			.validate(),
			Err(ScheduleError::UnknownZone("Mars/Olympus".into()))
		);
		assert!(
			Schedule::Cron {
				expression: "0 2 * * *".into(),
				zone: Some("Pacific/Auckland".into())
			}
			.validate()
			.is_ok()
		);
	}

	#[test]
	fn columns_round_trip() {
		for schedule in [
			Schedule::Manual,
			Schedule::Interval { seconds: 86_400 },
			Schedule::Cron {
				expression: "0 2 * * *".into(),
				zone: Some("UTC".into()),
			},
			Schedule::Cron {
				expression: "0 2 * * *".into(),
				zone: None,
			},
		] {
			let (i, c, z) = schedule.to_columns();
			assert_eq!(Schedule::from_columns(i, c, z), schedule);
		}
	}

	#[test]
	fn the_zone_is_the_schedules_then_the_machines_then_utc() {
		let z = resolve_zone(Some("Pacific/Auckland"), Some("Australia/Sydney"));
		assert_eq!(
			(z.name.as_str(), z.source),
			("Pacific/Auckland", ZoneSource::Schedule)
		);

		let z = resolve_zone(None, Some("Australia/Sydney"));
		assert_eq!(
			(z.name.as_str(), z.source),
			("Australia/Sydney", ZoneSource::Machine)
		);

		let z = resolve_zone(None, None);
		assert_eq!(
			(z.name.as_str(), z.source),
			("UTC", ZoneSource::UnreportedUtc)
		);
		let z = resolve_zone(None, Some("  "));
		assert_eq!(z.source, ZoneSource::UnreportedUtc);

		let z = resolve_zone(None, Some("Not/AZone"));
		assert_eq!(
			(z.name.as_str(), z.source),
			("UTC", ZoneSource::UnrecognisedUtc)
		);
		assert!(z.source.is_fallback());
	}

	#[test]
	fn a_windows_zone_name_is_read_as_its_iana_equivalent() {
		let z = resolve_zone(None, Some("New Zealand Standard Time"));
		assert_eq!(
			(z.name.as_str(), z.source),
			("Pacific/Auckland", ZoneSource::Machine)
		);
		let z = resolve_zone(None, Some("new zealand standard time"));
		assert_eq!(z.name, "Pacific/Auckland");
		let z = resolve_zone(None, Some("Tokyo Standard Time"));
		assert_eq!(z.name, "Asia/Tokyo");
	}

	#[test]
	fn every_windows_name_maps_to_a_zone_jiff_knows() {
		for (windows, iana) in windows_zones::WINDOWS_TO_IANA {
			assert!(TimeZone::get(iana).is_ok(), "{windows} -> {iana}");
		}
	}

	#[test]
	fn a_daily_firing_is_read_on_the_zones_wall_clock() {
		let c = cron("0 2 * * *", "Pacific/Auckland", 1);
		let got = firings(&c, "2026-10-07T00:00:00Z", 3);
		assert_eq!(
			got.iter()
				.map(|t| local(*t, "Pacific/Auckland"))
				.collect::<Vec<_>>(),
			["2026-10-08 02:00", "2026-10-09 02:00", "2026-10-10 02:00"]
		);
	}

	#[test]
	fn spring_forward_skips_a_firing_in_the_skipped_hour() {
		// Auckland jumps from 02:00 to 03:00 on 2026-09-27.
		let c = cron("30 2 * * *", "Pacific/Auckland", 1);
		let got = firings(&c, "2026-09-25T00:00:00Z", 4);
		let days: Vec<String> = got.iter().map(|t| local(*t, "Pacific/Auckland")).collect();
		assert!(
			!days.iter().any(|d| d.starts_with("2026-09-27")),
			"no firing on the day 02:30 doesn't exist: {days:?}"
		);
	}

	#[test]
	fn fall_back_fires_a_daily_expression_once() {
		// Auckland repeats 02:00-03:00 on 2026-04-05.
		let c = cron("30 2 * * *", "Pacific/Auckland", 1);
		let got = firings(&c, "2026-04-03T00:00:00Z", 5);
		let days: Vec<String> = got.iter().map(|t| local(*t, "Pacific/Auckland")).collect();
		assert_eq!(
			days.iter().filter(|d| d.starts_with("2026-04-05")).count(),
			1,
			"{days:?}"
		);
	}

	#[test]
	fn fall_back_skips_the_repeated_hour_of_an_hourly_expression() {
		let c = cron("0 * * * *", "Pacific/Auckland", 1);
		// Midday before the change to midday after.
		let got: Vec<Timestamp> = c
			.firings_after(ts("2026-04-04T11:00:00Z"))
			.take_while(|t| *t < ts("2026-04-04T17:30:00Z"))
			.collect();
		let wall: Vec<String> = got.iter().map(|t| local(*t, "Pacific/Auckland")).collect();
		let twos = wall.iter().filter(|w| w.ends_with(" 02:00")).count();
		assert_eq!(twos, 1, "the repeated 02:00 fires once: {wall:?}");
		for pair in got.windows(2) {
			assert!(pair[1].duration_since(pair[0]) >= MIN_GAP);
		}
	}

	#[test]
	fn a_half_hour_shift_does_not_fire_inside_the_hour() {
		// Lord Howe shifts by thirty minutes.
		let c = cron("30 * * * *", "Australia/Lord_Howe", 1);
		let got: Vec<Timestamp> = c
			.firings_after(ts("2026-04-04T00:00:00Z"))
			.take_while(|t| *t < ts("2026-04-06T00:00:00Z"))
			.collect();
		for pair in got.windows(2) {
			assert!(pair[1].duration_since(pair[0]) >= MIN_GAP, "{pair:?}");
		}
	}

	#[test]
	fn a_window_runs_from_the_firing_to_halfway_to_the_next() {
		let c = cron("0 2 * * *", "UTC", 1);
		let w = c.window(ts("2026-10-07T02:00:00Z"), ts("2026-10-08T02:00:00Z"));
		assert_eq!(w.firing, ts("2026-10-07T02:00:00Z"));
		assert_eq!(w.closes, ts("2026-10-07T14:00:00Z"));
	}

	#[test]
	fn a_spread_offset_is_small_against_the_window_and_stable() {
		let c = cron("0 2 * * *", "UTC", 12345);
		let w = c.window(ts("2026-10-07T02:00:00Z"), ts("2026-10-08T02:00:00Z"));
		let offset = w.opens.duration_since(w.firing);
		assert!(
			offset >= SignedDuration::ZERO && offset <= SPREAD_CAP,
			"{offset}"
		);
		let again = c.window(ts("2026-10-08T02:00:00Z"), ts("2026-10-09T02:00:00Z"));
		assert_eq!(again.opens.duration_since(again.firing), offset);

		// A window of an hour allows a quarter of half of it, at most.
		let tight = c.window(ts("2026-10-07T02:00:00Z"), ts("2026-10-07T03:00:00Z"));
		assert!(tight.opens.duration_since(tight.firing) <= SignedDuration::from_secs(450));
	}

	#[test]
	fn a_hashed_expression_opens_its_window_on_the_firing() {
		let c = cron("H 2 * * *", "UTC", 99);
		let w = c.window(ts("2026-10-07T02:07:00Z"), ts("2026-10-08T02:07:00Z"));
		assert_eq!(w.opens, w.firing);
	}

	#[test]
	fn machines_sharing_a_schedule_open_at_different_moments() {
		let offsets: std::collections::HashSet<i64> = (0..30)
			.map(|seed| {
				let c = cron("0 2 * * *", "UTC", seed);
				let w = c.window(ts("2026-10-07T02:00:00Z"), ts("2026-10-08T02:00:00Z"));
				w.opens.duration_since(w.firing).as_secs()
			})
			.collect();
		assert!(offsets.len() > 10);
	}

	fn effective(expr: &str, zone: &str, since: Option<&str>) -> EffectiveSchedule {
		EffectiveSchedule {
			schedule: Schedule::Cron {
				expression: expr.into(),
				zone: Some(zone.into()),
			},
			layer: Some(ScheduleLayer::Fleet),
			zone: Some(resolve_zone(Some(zone), None)),
			since: since.map(ts),
			layers_since: since.map(ts),
		}
	}

	#[test]
	fn due_inside_a_window_until_a_success_or_the_window_closes() {
		let s = effective("H 2 * * *", "UTC", None);
		let seed = 3;
		let firing = s
			.bind(seed)
			.unwrap()
			.unwrap()
			.firings_after(ts("2026-10-07T00:00:00Z"))
			.next()
			.unwrap();
		assert_eq!(firing.to_string()[11..13].to_string(), "02");

		let inside = firing + SignedDuration::from_hours(1);
		assert!(s.is_due(seed, inside, None));
		// A failed run leaves it due; a success since the firing ends it.
		assert!(s.is_due(seed, inside, Some(firing - SignedDuration::from_hours(20))));
		assert!(!s.is_due(seed, inside, Some(firing + SignedDuration::from_mins(5))));
		// Before the firing, and after the window closes unmet, it isn't due.
		assert!(!s.is_due(
			seed,
			firing - SignedDuration::from_mins(1),
			Some(firing - SignedDuration::from_hours(24))
		));
		let after_close = firing + SignedDuration::from_hours(13);
		assert!(!s.is_due(seed, after_close, None));
	}

	#[test]
	fn a_missed_window_is_not_caught_up_later() {
		let s = effective("0 2 * * *", "UTC", None);
		let last = ts("2026-10-05T02:10:00Z");
		// 02:00 on the 7th, window closed at 14:00; at 20:00 nothing is due.
		assert!(!s.is_due(1, ts("2026-10-07T20:00:00Z"), Some(last)));
		// The next window opens.
		assert!(s.is_due(1, ts("2026-10-08T02:30:00Z"), Some(last)));
	}

	#[test]
	fn a_machine_arriving_mid_window_is_due_at_once() {
		let s = effective("0 2 * * *", "UTC", None);
		assert!(s.is_due(1, ts("2026-10-07T09:00:00Z"), None));
	}

	#[test]
	fn a_firing_before_the_schedule_took_effect_opens_no_window() {
		// Changed at 14:00 from nightly 2am to 6am: nothing is due until 06:00
		// tomorrow, though 06:00 today has passed.
		let s = effective("0 6 * * *", "UTC", Some("2026-10-07T14:00:00Z"));
		let last = Some(ts("2026-10-07T02:10:00Z"));
		assert!(!s.is_due(1, ts("2026-10-07T14:30:00Z"), last));
		assert!(!s.is_due(1, ts("2026-10-08T05:59:00Z"), last));
		assert!(s.is_due(1, ts("2026-10-08T06:30:00Z"), last));
	}

	#[test]
	fn interval_and_manual_are_as_they_were() {
		let now = ts("2026-10-07T12:00:00Z");
		let interval = EffectiveSchedule {
			schedule: Schedule::Interval { seconds: 86_400 },
			layer: Some(ScheduleLayer::Group),
			zone: None,
			since: Some(now),
			layers_since: Some(now),
		};
		assert!(interval.is_due(1, now, None));
		assert!(!interval.is_due(1, now, Some(now - SignedDuration::from_hours(23))));
		assert!(interval.is_due(1, now, Some(now - SignedDuration::from_hours(24))));
		assert!(!EffectiveSchedule::manual().is_due(1, now, None));
	}

	#[test]
	fn next_backup_names_what_to_expect() {
		let now = ts("2026-10-07T09:00:00Z");
		assert_eq!(
			EffectiveSchedule::manual().next_backup(1, now, None),
			NextBackup::Manual
		);

		let interval = EffectiveSchedule {
			schedule: Schedule::Interval { seconds: 86_400 },
			layer: Some(ScheduleLayer::Fleet),
			zone: None,
			since: None,
			layers_since: None,
		};
		assert_eq!(interval.next_backup(1, now, None), NextBackup::DueNow);
		let last = now - SignedDuration::from_hours(20);
		assert_eq!(
			interval.next_backup(1, now, Some(last)),
			NextBackup::At(last + SignedDuration::from_hours(24))
		);
		assert_eq!(
			interval.next_backup(1, now, Some(now - SignedDuration::from_hours(30))),
			NextBackup::DueNow
		);

		let s = effective("0 2 * * *", "UTC", None);
		// A window is open: due until halfway to the next.
		assert_eq!(
			s.next_backup(1, now, Some(ts("2026-10-06T02:00:00Z"))),
			NextBackup::DueUntil(ts("2026-10-07T14:00:00Z"))
		);
		// Backed up since: the next firing.
		assert_eq!(
			s.next_backup(1, now, Some(ts("2026-10-07T02:30:00Z"))),
			NextBackup::At(ts("2026-10-08T02:00:00Z"))
		);
		// The window closed unmet: the next firing.
		let late = ts("2026-10-07T20:00:00Z");
		assert_eq!(
			s.next_backup(1, late, Some(ts("2026-10-06T02:00:00Z"))),
			NextBackup::At(ts("2026-10-08T02:00:00Z"))
		);
	}

	#[test]
	fn two_closed_windows_without_a_success_are_a_miss() {
		let s = effective("0 2 * * *", "UTC", None);
		let last = Some(ts("2026-10-03T02:10:00Z"));
		// By the 6th at 03:00 the 4th and 5th windows have closed (14:00 each
		// day); the 6th's is open. So the 4th and 5th were both missed.
		assert!(s.missed_two_firings(1, ts("2026-10-06T03:00:00Z"), last, None));
		// One missed window isn't two.
		assert!(!s.missed_two_firings(1, ts("2026-10-05T03:00:00Z"), last, None));
		// A success after the earlier of the two clears it.
		assert!(!s.missed_two_firings(
			1,
			ts("2026-10-06T03:00:00Z"),
			Some(ts("2026-10-05T02:10:00Z")),
			None
		));
	}

	#[test]
	fn firings_before_the_schedule_took_effect_are_not_missed() {
		let s = effective("0 2 * * *", "UTC", Some("2026-10-06T00:00:00Z"));
		let last = Some(ts("2026-10-01T02:10:00Z"));
		// Only the 6th's firing has closed since the change.
		assert!(!s.missed_two_firings(1, ts("2026-10-06T20:00:00Z"), last, None));
		assert!(s.missed_two_firings(1, ts("2026-10-07T20:00:00Z"), last, None));
	}

	#[test]
	fn a_machine_that_never_backed_up_misses_from_when_expected() {
		let s = effective("0 2 * * *", "UTC", None);
		let began = Some(ts("2026-10-05T12:00:00Z"));
		// Firings on the 6th and 7th have closed by 20:00 on the 7th.
		assert!(s.missed_two_firings(1, ts("2026-10-07T20:00:00Z"), None, began));
		assert!(!s.missed_two_firings(1, ts("2026-10-06T20:00:00Z"), None, began));
	}

	#[test]
	fn an_unreadable_stored_expression_is_never_due_and_always_late() {
		let s = effective("0 2 * * * *", "UTC", None);
		let now = ts("2026-10-07T20:00:00Z");
		assert!(!s.is_due(1, now, None));
		assert_eq!(s.next_backup(1, now, None), NextBackup::Unreadable);
		assert!(s.missed_two_firings(1, now, Some(now), None));
	}

	#[test]
	fn sparse_expressions_are_found_however_far_back() {
		let s = effective("0 0 1 1 *", "UTC", None);
		let c = s.bind(1).unwrap().unwrap();
		let w = c.window_at(ts("2026-10-07T00:00:00Z"), None).unwrap();
		assert_eq!(w.firing, ts("2026-01-01T00:00:00Z"));
	}
}
