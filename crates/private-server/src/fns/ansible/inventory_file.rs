//! Reading an Ansible YAML inventory file: the variables each host would carry
//! and the comments an operator left in it.
//!
//! A file is a tree of groups under `all`, each with optional `vars`, `hosts`,
//! and `children`. A value is read as Ansible reads YAML (PyYAML's 1.1
//! resolution, so `yes` and `no` are booleans) and carried as JSON.
// spec: ANSIMP#reading-the-file

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Number, Value};
use yaml_rust2::{
	parser::{Event, MarkedEventReceiver, Parser, Tag},
	scanner::{Marker, TScalarStyle},
};

/// Why a file is not an inventory, naming where it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl Display for ParseError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for ParseError {}

fn error(what: impl Into<String>) -> ParseError {
	ParseError(what.into())
}

/// The host named by a file's key, with the variables it would carry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileHost {
	/// What the host sets itself.
	pub own: BTreeMap<String, Value>,
	/// What it takes from the groups below `all` it is in, or in a group beneath,
	/// the value Ansible would have used where several set one: a nearer group's
	/// over a farther one's, then the higher `ansible_group_priority`, then the
	/// name that sorts later. A host's own value wins over all of it, so a name
	/// the host sets is not here.
	pub inherited: BTreeMap<String, Value>,
	/// The comment lines about the host, without their `#`, in order: those
	/// within its entry, and the run directly above its key, from every place
	/// it is listed.
	pub comments: Vec<String>,
}

impl FileHost {
	/// What the host would be given: its own over its groups'.
	pub fn effective(&self) -> BTreeMap<String, Value> {
		let mut effective = self.inherited.clone();
		effective.extend(self.own.clone());
		effective
	}
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InventoryFile {
	/// The variables under `all`.
	pub environment: BTreeMap<String, Value>,
	/// Every host, by the key the file gives it.
	pub hosts: BTreeMap<String, FileHost>,
	/// The file's comment lines that are about no host, without their `#`, in
	/// order.
	pub comments: Vec<String>,
}

#[derive(Debug, Clone)]
enum Node {
	Scalar {
		text: String,
		style: TScalarStyle,
		tag: Option<Tag>,
	},
	Seq(Vec<Node>),
	Map(Vec<(Node, Node)>),
}

enum Frame {
	Seq {
		anchor: usize,
		items: Vec<Node>,
	},
	Map {
		anchor: usize,
		pairs: Vec<(Node, Node)>,
		key: Option<Node>,
	},
}

#[derive(Default)]
struct Builder {
	stack: Vec<Frame>,
	/// The height of what each open frame holds so far, beside it.
	heights: Vec<usize>,
	root: Option<Node>,
	/// Each anchored node, with its height.
	anchors: HashMap<usize, (Node, usize)>,
	/// Lines that hold the text of a block scalar, which a `#` on does not
	/// begin a comment.
	block_lines: BTreeSet<usize>,
	/// Where each host's key is written: the key, its line, and its column.
	host_keys: Vec<HostKey>,
	/// How many nodes aliases have copied in, which a document that nests them
	/// can make grow exponentially.
	aliased: usize,
	too_many_aliases: bool,
	too_deep: bool,
}

/// How deeply a file may nest, far deeper than any inventory a person wrote,
/// and what keeps the recursion over the tree off the stack's end.
const DEPTH_LIMIT: usize = 100;

/// What aliases may copy into one file before it is refused, far more than any
/// inventory a person wrote holds.
const ALIAS_BUDGET: usize = 100_000;

impl Node {
	fn weight(&self) -> usize {
		match self {
			Node::Scalar { .. } => 1,
			Node::Seq(items) => 1 + items.iter().map(Node::weight).sum::<usize>(),
			Node::Map(pairs) => {
				1 + pairs
					.iter()
					.map(|(key, value)| key.weight() + value.weight())
					.sum::<usize>()
			}
		}
	}
}

/// Where a host's key is written in the file.
struct HostKey {
	key: String,
	/// Its line, counting from one.
	line: usize,
	/// Its column, counting from zero.
	column: usize,
}

/// Whether a key at `path` names a host: the keys above it are a group,
/// any number of `children` and a group beneath, then `hosts`.
fn is_host_path(path: &[String]) -> bool {
	path.len() >= 2
		&& path.len().is_multiple_of(2)
		&& path.last().is_some_and(|key| key == "hosts")
		&& path[1..path.len() - 1]
			.iter()
			.step_by(2)
			.all(|key| key == "children")
}

impl Builder {
	/// The keys whose values hold what comes next, from the top, where every
	/// one of them is a scalar key of a mapping.
	fn path(&self) -> Option<Vec<String>> {
		let (_, above) = self.stack.split_last()?;
		above
			.iter()
			.map(|frame| match frame {
				Frame::Map { key: Some(key), .. } => key_text(key, &[]).ok(),
				_ => None,
			})
			.collect()
	}

	/// Place a node of `height` levels in what is open.
	fn push(&mut self, node: Node, height: usize) {
		if let Some(held) = self.heights.last_mut() {
			*held = (*held).max(height);
		}
		match self.stack.last_mut() {
			None => self.root = Some(node),
			Some(Frame::Seq { items, .. }) => items.push(node),
			Some(Frame::Map { pairs, key, .. }) => match key.take() {
				None => *key = Some(node),
				Some(key) => pairs.push((key, node)),
			},
		}
	}

	fn anchor(&mut self, id: usize, node: &Node, height: usize) {
		if id > 0 {
			self.anchors.insert(id, (node.clone(), height));
		}
	}

	fn refuse_as_too_deep(&mut self) {
		self.too_deep = true;
		self.stack.clear();
		self.heights.clear();
	}
}

impl MarkedEventReceiver for Builder {
	fn on_event(&mut self, event: Event, mark: Marker) {
		if self.too_deep {
			return;
		}
		if matches!(event, Event::SequenceStart(..) | Event::MappingStart(..))
			&& self.stack.len() >= DEPTH_LIMIT
		{
			self.refuse_as_too_deep();
			return;
		}
		match event {
			Event::Scalar(text, style, id, tag) => {
				if matches!(style, TScalarStyle::Literal | TScalarStyle::Folded) {
					// The mark is the first line of the text.
					self.block_lines
						.extend(mark.line()..mark.line() + text.lines().count().max(1));
				}
				let node = Node::Scalar { text, style, tag };
				if matches!(self.stack.last(), Some(Frame::Map { key: None, .. }))
					&& let Some(path) = self.path()
					&& is_host_path(&path)
					&& let Ok(key) = key_text(&node, &path)
				{
					self.host_keys.push(HostKey {
						key,
						line: mark.line(),
						column: mark.col(),
					});
				}
				self.anchor(id, &node, 1);
				self.push(node, 1);
			}
			Event::SequenceStart(anchor, _) => {
				self.stack.push(Frame::Seq {
					anchor,
					items: Vec::new(),
				});
				self.heights.push(0);
			}
			Event::MappingStart(anchor, _) => {
				self.stack.push(Frame::Map {
					anchor,
					pairs: Vec::new(),
					key: None,
				});
				self.heights.push(0);
			}
			Event::SequenceEnd => {
				if let Some(Frame::Seq { anchor, items }) = self.stack.pop() {
					let height = self.heights.pop().unwrap_or_default() + 1;
					let node = Node::Seq(items);
					self.anchor(anchor, &node, height);
					self.push(node, height);
				}
			}
			Event::MappingEnd => {
				if let Some(Frame::Map { anchor, pairs, .. }) = self.stack.pop() {
					let height = self.heights.pop().unwrap_or_default() + 1;
					let node = Node::Map(pairs);
					self.anchor(anchor, &node, height);
					self.push(node, height);
				}
			}
			Event::Alias(id) => {
				if let Some((node, height)) = self.anchors.get(&id).cloned() {
					// An alias places its anchor's whole tree where it stands, so
					// the tree is held to the depth a file may nest as though it
					// had been written out there.
					if self.stack.len() + height > DEPTH_LIMIT + 1 {
						self.refuse_as_too_deep();
						return;
					}
					self.aliased = self.aliased.saturating_add(node.weight());
					if self.aliased > ALIAS_BUDGET {
						self.too_many_aliases = true;
						return;
					}
					self.push(node, height);
				}
			}
			Event::Nothing
			| Event::StreamStart
			| Event::StreamEnd
			| Event::DocumentStart
			| Event::DocumentEnd => {}
		}
	}
}

static INT_DECIMAL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[-+]?(0|[1-9][0-9_]*)$").unwrap());
static INT_BINARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]?0b[01_]+$").unwrap());
static INT_OCTAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]?0[0-7_]+$").unwrap());
static INT_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]?0x[0-9a-fA-F_]+$").unwrap());
static INT_SEXAGESIMAL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[-+]?[1-9][0-9_]*(:[0-5]?[0-9])+$").unwrap());
static FLOAT: LazyLock<Regex> = LazyLock::new(|| {
	Regex::new(r"^[-+]?([0-9][0-9_]*)\.[0-9_]*([eE][-+][0-9]+)?$|^[-+]?\.[0-9_]+([eE][-+][0-9]+)?$")
		.unwrap()
});
static FLOAT_SEXAGESIMAL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[-+]?[0-9][0-9_]*(:[0-5]?[0-9])+\.[0-9_]*$").unwrap());
static FLOAT_SPECIAL: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^([-+]?\.(inf|Inf|INF)|\.(nan|NaN|NAN))$").unwrap());

/// What a plain (unquoted, untagged) scalar is read as, by PyYAML's rules.
fn resolve_plain(text: &str) -> Value {
	match text {
		"" | "~" | "null" | "Null" | "NULL" => return Value::Null,
		"yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => {
			return Value::Bool(true);
		}
		"no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => {
			return Value::Bool(false);
		}
		"=" => return Value::String(text.to_owned()),
		_ => {}
	}

	let sign = |n: i128, text: &str| if text.starts_with('-') { -n } else { n };
	let digits = |text: &str| text.trim_start_matches(['-', '+']).replace('_', "");
	// Beyond what every reader of JSON holds exactly, the browser's text editor
	// among them, carried as the text it was, as the editor carries it too.
	const SAFE: i128 = (1 << 53) - 1;
	let integer = |n: i128| {
		if (-SAFE..=SAFE).contains(&n) {
			Value::Number(Number::from(n as i64))
		} else {
			Value::String(text.to_owned())
		}
	};

	if INT_DECIMAL.is_match(text) {
		return digits(text)
			.parse::<i128>()
			.map(|n| integer(sign(n, text)))
			.unwrap_or_else(|_| Value::String(text.to_owned()));
	}
	if INT_BINARY.is_match(text) {
		return i128::from_str_radix(digits(text).trim_start_matches("0b"), 2)
			.map(|n| integer(sign(n, text)))
			.unwrap_or_else(|_| Value::String(text.to_owned()));
	}
	if INT_HEX.is_match(text) {
		return i128::from_str_radix(digits(text).trim_start_matches("0x"), 16)
			.map(|n| integer(sign(n, text)))
			.unwrap_or_else(|_| Value::String(text.to_owned()));
	}
	if INT_OCTAL.is_match(text) {
		return i128::from_str_radix(&digits(text), 8)
			.map(|n| integer(sign(n, text)))
			.unwrap_or_else(|_| Value::String(text.to_owned()));
	}
	if INT_SEXAGESIMAL.is_match(text) {
		let total = digits(text).split(':').try_fold(0i128, |total, part| {
			part.parse::<i128>().map(|part| total * 60 + part)
		});
		return total
			.map(|n| integer(sign(n, text)))
			.unwrap_or_else(|_| Value::String(text.to_owned()));
	}
	if FLOAT_SPECIAL.is_match(text) {
		// No JSON form, so it is the text it is.
		return Value::String(text.to_owned());
	}
	if FLOAT.is_match(text) {
		return digits(text)
			.parse::<f64>()
			.ok()
			.map(|n| if text.starts_with('-') { -n } else { n })
			.and_then(Number::from_f64)
			.map(Value::Number)
			.unwrap_or_else(|| Value::String(text.to_owned()));
	}
	if FLOAT_SEXAGESIMAL.is_match(text) {
		let mut parts = digits(text);
		let fraction = parts.split_off(parts.find('.').unwrap_or(parts.len()));
		let whole = parts.split(':').try_fold(0f64, |total, part| {
			part.parse::<f64>().map(|part| total * 60.0 + part)
		});
		return whole
			.ok()
			.and_then(|whole| format!("{whole}{fraction}").parse::<f64>().ok())
			.map(|n| if text.starts_with('-') { -n } else { n })
			.and_then(Number::from_f64)
			.map(Value::Number)
			.unwrap_or_else(|| Value::String(text.to_owned()));
	}
	Value::String(text.to_owned())
}

/// What a scalar is read as, given its quoting and tag.
fn resolve(
	text: &str,
	style: TScalarStyle,
	tag: Option<&Tag>,
	path: &[String],
) -> Result<Value, ParseError> {
	if let Some(tag) = tag {
		let name = tag.suffix.as_str();
		if tag.handle == "!" && name == "vault" {
			return Err(error(format!(
				"{} is vault-encrypted, which cannot be carried across",
				path.join(".")
			)));
		}
		return Ok(match name {
			"int" | "float" | "bool" | "null" if tag.handle.starts_with("tag:yaml.org") => {
				resolve_plain(text)
			}
			_ => Value::String(text.to_owned()),
		});
	}
	Ok(match style {
		TScalarStyle::Plain => resolve_plain(text),
		_ => Value::String(text.to_owned()),
	})
}

fn key_text(node: &Node, path: &[String]) -> Result<String, ParseError> {
	match node {
		Node::Scalar { text, style, tag } => match resolve(text, *style, tag.as_ref(), path)? {
			Value::String(text) => Ok(text),
			Value::Null => Ok("null".to_owned()),
			other => Ok(other.to_string()),
		},
		_ => Err(error(format!(
			"{} has a key that is not a scalar",
			path.join(".")
		))),
	}
}

fn to_json(node: &Node, path: &mut Vec<String>) -> Result<Value, ParseError> {
	match node {
		Node::Scalar { text, style, tag } => resolve(text, *style, tag.as_ref(), path),
		Node::Seq(items) => items
			.iter()
			.enumerate()
			.map(|(index, item)| {
				path.push(index.to_string());
				let value = to_json(item, path);
				path.pop();
				value
			})
			.collect::<Result<Vec<_>, _>>()
			.map(Value::Array),
		Node::Map(pairs) => {
			let mut map = Map::new();
			let mut merged: Vec<&Node> = Vec::new();
			for (key, value) in pairs {
				let is_merge = matches!(
					key,
					Node::Scalar { text, style: TScalarStyle::Plain, tag: None } if text == "<<"
				);
				if is_merge {
					merged.push(value);
					continue;
				}
				let name = key_text(key, path)?;
				path.push(name.clone());
				let value = to_json(value, path);
				path.pop();
				map.insert(name, value?);
			}
			// An explicit key wins over a merged one, and an earlier merge
			// over a later one.
			for source in merged {
				let sources = match source {
					Node::Seq(items) => items.iter().collect::<Vec<_>>(),
					other => vec![other],
				};
				for source in sources {
					let Value::Object(source) = to_json(source, path)? else {
						return Err(error(format!(
							"{} merges with `<<` something that is not a mapping",
							path.join(".")
						)));
					};
					for (name, value) in source {
						map.entry(name).or_insert(value);
					}
				}
			}
			Ok(Value::Object(map))
		}
	}
}

/// A group as the file lays it out.
#[derive(Debug, Default)]
struct Group {
	vars: Map<String, Value>,
	hosts: Vec<String>,
	children: Vec<String>,
}

fn object<'a>(value: &'a Value, what: &str) -> Result<Option<&'a Map<String, Value>>, ParseError> {
	match value {
		Value::Null => Ok(None),
		Value::Object(map) => Ok(Some(map)),
		_ => Err(error(format!("{what} is not a mapping"))),
	}
}

const PRIORITY: &str = "ansible_group_priority";

/// Read the file's text as an inventory.
pub fn parse(text: &str) -> Result<InventoryFile, ParseError> {
	let mut builder = Builder::default();
	Parser::new_from_str(text)
		.load(&mut builder, false)
		.map_err(|e| error(e.to_string()))?;
	if builder.too_deep {
		return Err(error(format!(
			"the file nests more than {DEPTH_LIMIT} levels deep, which no inventory needs"
		)));
	}
	if builder.too_many_aliases {
		return Err(error(
			"the file's aliases expand to far more than it holds, which no inventory needs",
		));
	}
	let Some(root) = builder.root.take() else {
		return Err(error("the file is empty"));
	};
	let document = to_json(&root, &mut Vec::new())?;
	let Value::Object(top) = document else {
		return Err(error("the file is not a mapping of groups"));
	};
	let Some(all) = top.get("all") else {
		return Err(error(
			"the file has no `all` group, so it is not an inventory",
		));
	};

	let mut groups: BTreeMap<String, Group> = BTreeMap::new();
	let mut host_vars: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
	let mut depths: BTreeMap<String, usize> = BTreeMap::new();

	// Groups at the top beside `all` are its children, as Ansible reads them.
	let mut queue: Vec<(String, &Value, usize)> = vec![("all".into(), all, 0)];
	for (name, group) in &top {
		if name != "all" {
			queue.push((name.clone(), group, 1));
		}
	}
	let beside_all: Vec<String> = top.keys().filter(|name| *name != "all").cloned().collect();

	while let Some((name, value, depth)) = queue.pop() {
		let here = format!("group {name:?}");
		let Some(map) = object(value, &here)? else {
			groups.entry(name.clone()).or_default();
			let known = depths.entry(name).or_insert(depth);
			*known = (*known).max(depth);
			continue;
		};
		let entry = groups.entry(name.clone()).or_default();
		let known = depths.entry(name.clone()).or_insert(depth);
		*known = (*known).max(depth);

		if let Some(vars) = map.get("vars") {
			let Some(vars) = object(vars, &format!("`vars` of {here}"))? else {
				continue;
			};
			for (key, value) in vars {
				entry.vars.insert(key.clone(), value.clone());
			}
		}
		if let Some(hosts) = map.get("hosts")
			&& let Some(hosts) = object(hosts, &format!("`hosts` of {here}"))?
		{
			for (key, value) in hosts {
				let vars = object(value, &format!("host {key:?} of {here}"))?;
				entry.hosts.push(key.clone());
				let own = host_vars.entry(key.clone()).or_default();
				for (name, value) in vars.into_iter().flatten() {
					own.insert(name.clone(), value.clone());
				}
			}
		}
		if let Some(children) = map.get("children")
			&& let Some(children) = object(children, &format!("`children` of {here}"))?
		{
			for (child, value) in children {
				entry.children.push(child.clone());
				queue.push((child.clone(), value, depth + 1));
			}
		}
	}
	if let Some(all) = groups.get_mut("all") {
		all.children.extend(beside_all);
	}

	// Every group each host is in, directly or through a group beneath: each
	// group is walked once for the hosts under it.
	let mut membership: BTreeMap<String, BTreeSet<String>> = host_vars
		.keys()
		.map(|host| (host.clone(), BTreeSet::new()))
		.collect();
	for name in groups.keys().filter(|name| *name != "all") {
		let mut seen: BTreeSet<&str> = BTreeSet::new();
		let mut pending = vec![name.as_str()];
		while let Some(group) = pending.pop() {
			if !seen.insert(group) {
				continue;
			}
			let Some(data) = groups.get(group) else {
				continue;
			};
			for host in &data.hosts {
				if let Some(member_of) = membership.get_mut(host) {
					member_of.insert(name.clone());
				}
			}
			pending.extend(data.children.iter().map(String::as_str));
		}
	}

	let priority = |group: &Group| {
		group
			.vars
			.get(PRIORITY)
			.and_then(Value::as_i64)
			.unwrap_or(1)
	};

	let (comments, mut host_comments) = comments(text, &builder.block_lines, &builder.host_keys);

	let mut hosts = BTreeMap::new();
	for (key, own) in host_vars {
		let mut layers: Vec<(&String, usize, i64)> = membership[&key]
			.iter()
			.map(|name| (name, depths[name], priority(&groups[name])))
			.collect();
		// Deeper wins, then priority, then the name that sorts later.
		layers.sort_by(|a, b| (a.1, a.2, a.0).cmp(&(b.1, b.2, b.0)));

		let mut inherited = BTreeMap::new();
		for (name, ..) in layers {
			for (var, value) in &groups[name].vars {
				if var != PRIORITY {
					inherited.insert(var.clone(), value.clone());
				}
			}
		}
		for name in own.keys() {
			inherited.remove(name);
		}
		let comments = host_comments.remove(&key).unwrap_or_default();
		hosts.insert(
			key,
			FileHost {
				own: own.into_iter().collect(),
				inherited,
				comments,
			},
		);
	}

	let environment = groups
		.get("all")
		.map(|all| {
			all.vars
				.iter()
				.filter(|(name, _)| *name != PRIORITY)
				.map(|(name, value)| (name.clone(), value.clone()))
				.collect()
		})
		.unwrap_or_default();

	Ok(InventoryFile {
		environment,
		hosts,
		comments,
	})
}

/// The comment lines, those whose first character is `#` outside block
/// scalars, without the `#` and the space after it: those about no host, and
/// those about each host by its key.
///
/// A comment is about a host when it sits within the host's entry, indented
/// beneath its key, or in the run of comment lines directly above its key
/// that no host's entry holds.
// spec: ANSIMP#reading-the-file
fn comments(
	text: &str,
	block_lines: &BTreeSet<usize>,
	host_keys: &[HostKey],
) -> (Vec<String>, BTreeMap<String, Vec<String>>) {
	let lines: Vec<&str> = text.lines().collect();
	let indent = |line: &str| line.chars().count() - line.trim_start().chars().count();
	// The comment on a line, by index, with the column its `#` is at.
	let comment = |index: usize| -> Option<(usize, &str)> {
		if block_lines.contains(&(index + 1)) {
			return None;
		}
		let line = lines[index];
		line.trim_start()
			.strip_prefix('#')
			.map(|comment| (indent(line), comment))
	};

	let mut owner: Vec<Option<&str>> = vec![None; lines.len()];
	for host in host_keys {
		// Its entry runs on until a line that is not indented beneath its key.
		for index in host.line.min(lines.len())..lines.len() {
			if let Some((column, _)) = comment(index) {
				if column > host.column && owner[index].is_none() {
					owner[index] = Some(&host.key);
				}
			} else if !lines[index].trim().is_empty() && indent(lines[index]) <= host.column {
				break;
			}
		}
	}
	for host in host_keys {
		for index in (0..host.line.saturating_sub(1).min(lines.len())).rev() {
			if comment(index).is_none() || owner[index].is_some() {
				break;
			}
			owner[index] = Some(&host.key);
		}
	}

	let mut unowned = Vec::new();
	let mut by_host: BTreeMap<String, Vec<String>> = BTreeMap::new();
	for (index, owner) in owner.iter().enumerate() {
		let Some((_, comment)) = comment(index) else {
			continue;
		};
		let comment = comment.trim_start_matches('#').trim();
		if comment.is_empty() {
			continue;
		}
		match owner {
			Some(host) => by_host
				.entry((*host).to_owned())
				.or_default()
				.push(comment.to_owned()),
			None => unowned.push(comment.to_owned()),
		}
	}
	(unowned, by_host)
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn host<'a>(file: &'a InventoryFile, key: &str) -> &'a FileHost {
		file.hosts
			.get(key)
			.unwrap_or_else(|| panic!("{key} is a host: {:?}", file.hosts.keys()))
	}

	#[test]
	fn reads_the_variables_under_all_and_on_each_host() {
		let file = parse(
			"all:
  vars:
    timezone: Pacific/Auckland
    elastic_agent_enabled: true
  hosts:
    example-central:
      public_hostname: central.example.org
      tailscale_name: example-central
",
		)
		.unwrap();

		assert_eq!(
			file.environment,
			BTreeMap::from([
				("elastic_agent_enabled".to_owned(), json!(true)),
				("timezone".to_owned(), json!("Pacific/Auckland")),
			])
		);
		assert_eq!(
			host(&file, "example-central").own["public_hostname"],
			json!("central.example.org")
		);
	}

	#[test]
	fn a_value_is_what_ansible_reads_it_as() {
		let file = parse(
			"all:
  vars:
    a: yes
    b: no
    c: 'yes'
    d: 0644
    e: 1_000
    f: 2.5
    g: 1e3
    h: 1:30
    i: null
    j: ~
    k: .inf
    l: 0x1F
    m: 12345678901234567890123
    n: 9007199254740991
    o: 9007199254740993
  hosts:
    h1:
      list: [1, two, {three: 3}]
",
		)
		.unwrap();
		let vars = &file.environment;
		assert_eq!(vars["a"], json!(true));
		assert_eq!(vars["b"], json!(false));
		assert_eq!(vars["c"], json!("yes"), "quoted is a string");
		assert_eq!(vars["d"], json!(420), "a leading zero is octal");
		assert_eq!(vars["e"], json!(1000));
		assert_eq!(vars["f"], json!(2.5));
		assert_eq!(vars["g"], json!("1e3"), "no dot, so it is a string");
		assert_eq!(vars["h"], json!(90), "sexagesimal");
		assert_eq!(vars["i"], Value::Null);
		assert_eq!(vars["j"], Value::Null);
		assert_eq!(vars["k"], json!(".inf"), "no JSON form, so its text");
		assert_eq!(vars["l"], json!(31));
		assert_eq!(vars["m"], json!("12345678901234567890123"));
		assert_eq!(vars["n"], json!(9007199254740991_i64));
		assert_eq!(
			vars["o"],
			json!("9007199254740993"),
			"past what the browser holds exactly, so its text"
		);
		assert_eq!(
			host(&file, "h1").own["list"],
			json!([1, "two", { "three": 3 }])
		);
	}

	#[test]
	fn a_host_takes_the_variables_of_the_groups_it_is_in_or_beneath() {
		let file = parse(
			"all:
  vars:
    level: all
  children:
    facility:
      vars:
        level: facility
        site_wide: yes
      children:
        site_a:
          vars:
            level: site-a
          hosts:
            a1:
              own_only: 1
        site_b:
          hosts:
            b1: {}
    other:
      hosts:
        b1:
",
		)
		.unwrap();

		assert_eq!(
			host(&file, "a1").inherited,
			BTreeMap::from([
				("level".to_owned(), json!("site-a")),
				("site_wide".to_owned(), json!(true)),
			]),
			"the nearer group wins and the farther one still contributes"
		);
		assert_eq!(
			host(&file, "b1").inherited,
			BTreeMap::from([
				("level".to_owned(), json!("facility")),
				("site_wide".to_owned(), json!(true)),
			])
		);
		assert_eq!(file.environment["level"], json!("all"));
	}

	#[test]
	fn a_hosts_own_value_wins_over_its_groups() {
		let file = parse(
			"all:
  children:
    g:
      vars: { port: 22, user: root }
      hosts:
        h: { port: 2222 }
",
		)
		.unwrap();
		let h = host(&file, "h");
		assert_eq!(h.effective()["port"], json!(2222));
		assert_eq!(h.effective()["user"], json!("root"));
		assert!(!h.inherited.contains_key("port"));
	}

	#[test]
	fn groups_at_one_depth_are_decided_by_priority_then_by_name() {
		let file = parse(
			"all:
  children:
    alpha:
      vars: { v: alpha }
      hosts: { h: }
    beta:
      vars: { v: beta }
      hosts: { h: }
    heavy:
      vars: { v: heavy, ansible_group_priority: 5 }
      hosts: { h: }
    heavier:
      vars: { ansible_group_priority: 9 }
      hosts: { g: }
    gamma:
      vars: { w: alpha }
      hosts: { g: }
    zeta:
      vars: { w: zeta }
      hosts: { g: }
",
		)
		.unwrap();
		assert_eq!(host(&file, "h").inherited["v"], json!("heavy"));
		assert_eq!(host(&file, "g").inherited["w"], json!("zeta"));
		assert!(
			!host(&file, "h")
				.inherited
				.contains_key("ansible_group_priority"),
			"the priority orders groups and is not itself a variable"
		);
	}

	#[test]
	fn a_host_is_in_every_group_above_it_and_a_cycle_ends() {
		let file = parse(
			"all:
  children:
    outer:
      vars: { o: outer }
      children:
        inner:
          vars: { i: inner }
          hosts: { h: }
          children:
            outer:
    other:
      vars: { w: other }
      hosts: { g: }
",
		)
		.unwrap();
		assert_eq!(host(&file, "h").inherited["i"], json!("inner"));
		assert_eq!(host(&file, "h").inherited["o"], json!("outer"));
		assert!(!host(&file, "h").inherited.contains_key("w"));
		assert!(!host(&file, "g").inherited.contains_key("o"));
	}

	#[test]
	fn a_host_may_sit_straight_under_all_and_be_keyed_by_address_and_port() {
		let file = parse(
			"all:
  hosts:
    10.20.30.40:223:
      ansible_user: siteadmin
",
		)
		.unwrap();
		assert_eq!(
			host(&file, "10.20.30.40:223").own["ansible_user"],
			json!("siteadmin")
		);
	}

	#[test]
	fn a_secret_field_inside_a_structured_value_is_carried_whole() {
		let file = parse(
			"all:
  vars:
    tamanu_caddy_tls_dns_iam:
      access_key_id: AKIAEXAMPLE
      secret_access_key: abc/def
",
		)
		.unwrap();
		assert_eq!(
			file.environment["tamanu_caddy_tls_dns_iam"],
			json!({ "access_key_id": "AKIAEXAMPLE", "secret_access_key": "abc/def" })
		);
	}

	#[test]
	fn anchors_and_merge_keys_are_followed() {
		let file = parse(
			"all:
  vars:
    base: &base
      region: nz
      tier: one
    derived:
      <<: *base
      tier: two
",
		)
		.unwrap();
		assert_eq!(
			file.environment["derived"],
			json!({ "region": "nz", "tier": "two" })
		);
	}

	#[test]
	fn refuses_nesting_that_would_run_out_the_stack() {
		let nested = |levels: usize| {
			format!(
				"all:\n  vars:\n    deep: {}x{}\n",
				"[".repeat(levels),
				"]".repeat(levels)
			)
		};
		let refused = parse(&nested(150)).unwrap_err();
		assert!(refused.0.contains("levels deep"), "{refused}");
		// Far deeper is refused too, by the parser's own limit before ours.
		assert!(parse(&nested(5_000)).is_err());
		assert!(parse(&nested(20)).is_ok());
	}

	#[test]
	fn refuses_aliases_that_stack_trees_deeper_than_a_file_may_nest() {
		// Each anchor is shallow, but each holds the one before it, so placed
		// together they nest far past the limit.
		let mut text = String::from("all:\n  vars:\n    a0: &a0 [[[[[[[[[[x]]]]]]]]]]\n");
		for level in 1..40 {
			let previous = level - 1;
			text.push_str(&format!(
				"    a{level}: &a{level} [[[[[[[[[[*a{previous}]]]]]]]]]]\n"
			));
		}
		let refused = parse(&text).unwrap_err();
		assert!(refused.0.contains("levels deep"), "{refused}");

		let shallow = "all:\n  vars:\n    a: &a [[x]]\n    b: [[*a]]\n";
		assert_eq!(parse(shallow).unwrap().environment["b"], json!([[[["x"]]]]));
	}

	#[test]
	fn refuses_aliases_that_expand_without_bound() {
		let mut text = String::from("all:\n  vars:\n    a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
		for level in 1..12 {
			let previous = level - 1;
			text.push_str(&format!(
				"    a{level}: &a{level} [*a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}, *a{previous}]\n"
			));
		}
		let refused = parse(&text).unwrap_err();
		assert!(refused.0.contains("aliases"), "{refused}");
	}

	#[test]
	fn collects_comment_lines_but_not_the_text_of_a_block_scalar() {
		let file = parse(
			"# Use: --key-file /path/to/ssh-key
all:
  vars:
    banner: |
      # not a comment
      text
    # SSH key is in the password manager
    after: 1
  hosts:
    h: {} # trailing is not collected
",
		)
		.unwrap();
		assert_eq!(
			file.comments,
			[
				"Use: --key-file /path/to/ssh-key",
				"SSH key is in the password manager"
			]
		);
		assert_eq!(file.environment["banner"], json!("# not a comment\ntext\n"));
	}

	#[test]
	fn a_comment_within_a_hosts_entry_or_directly_above_it_is_about_that_host() {
		let file = parse(
			"# About the environment
all:
  hosts:
    # Office box, behind the site VPN
    # Ask the site lead for access
    office:
      # Key is in the password manager
      ansible_user: ubuntu
      # Rotated yearly
    # Central is in the cloud
    central:
      public_hostname: central.example.org

    # Spare, not about any host
  children:
    facilities:
      hosts:
        office:
          # Listed again beneath a group
          tamanu_facility_id: office
      # About the facilities group
      vars:
        timezone: Pacific/Auckland
",
		)
		.unwrap();
		assert_eq!(
			host(&file, "office").comments,
			[
				"Office box, behind the site VPN",
				"Ask the site lead for access",
				"Key is in the password manager",
				"Rotated yearly",
				"Listed again beneath a group",
			]
		);
		assert_eq!(host(&file, "central").comments, ["Central is in the cloud"]);
		assert_eq!(
			file.comments,
			[
				"About the environment",
				"Spare, not about any host",
				"About the facilities group",
			]
		);
	}

	#[test]
	fn a_variable_named_hosts_does_not_make_its_keys_hosts() {
		let file = parse(
			"all:
  vars:
    hosts:
      # About a variable
      office: 1
  hosts:
    office: {}
",
		)
		.unwrap();
		assert!(host(&file, "office").comments.is_empty());
		assert_eq!(file.comments, ["About a variable"]);
	}

	#[test]
	fn refuses_what_is_not_an_inventory_naming_where() {
		let error = |text: &str| parse(text).unwrap_err().0;

		assert!(error("").contains("empty"));
		assert!(error("- a\n- b\n").contains("not a mapping of groups"));
		assert!(error("plugin: canopy\n").contains("no `all` group"));
		assert!(error("all:\n  vars: [1, 2]\n").contains("`vars` of group \"all\""));
		assert!(error("all:\n  hosts:\n    h: 3\n").contains("host \"h\""));
		assert!(error("all:\n  vars: {a: [b\n").contains("line"));
	}

	#[test]
	fn refuses_a_vault_encrypted_value_naming_the_variable() {
		let refused = parse(
			"all:
  vars:
    salt: !vault |
      $ANSIBLE_VAULT;1.1;AES256
      6162636465
",
		)
		.unwrap_err();
		assert!(refused.0.contains("all"), "{refused}");
		assert!(refused.0.contains("salt"), "{refused}");
		assert!(refused.0.contains("vault"), "{refused}");
	}
}
