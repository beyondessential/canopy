//! Generates `crates/canopy-api/src/generated.rs` from the public server's
//! OpenAPI document.
//!
//! The document is read from this repository, so generation needs no running
//! canopy and no network. The output is committed, so a change to the client's
//! surface appears in the change that causes it.
//!
//! Two kinds of type mapping are applied by rewriting the schemas before typify
//! sees them, rather than by patching the text it emits:
//!
//! - a field the document describes as a timestamp becomes [`jiff::Timestamp`]
//! - a field naming a credential secret becomes `Redacted<String>`, so it stays
//!   out of `Debug` output
//!
//! Both work by pointing the property at a synthetic schema and replacing that
//! schema with the target type, which keeps the property's own description.

use std::{collections::BTreeMap, fs, path::PathBuf, process::ExitCode};

use schemars::schema::RootSchema;
use serde_json::{Map, Value, json};
use typify::{TypeSpace, TypeSpaceSettings};

/// Synthetic schema names the replacements key on.
const TIMESTAMP: &str = "CanopyTimestamp";
const SECRET: &str = "CanopySecret";

/// Properties holding a credential secret, as `schema.property`.
///
/// Declared here rather than detected, because a secret is an ordinary string on
/// the wire. Generation fails when one of these is missing from the document, so
/// renaming a field surfaces here instead of silently unwrapping it.
const SECRETS: &[(&str, &str)] = &[
	("BackupTarget", "repo_password"),
	("CredentialProcessOutput", "SecretAccessKey"),
	("CredentialProcessOutput", "SessionToken"),
	("RestoreCredentials", "repo_password"),
];

const HTTP_VERBS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

fn main() -> ExitCode {
	let mut args = std::env::args().skip(1);
	let input = PathBuf::from(
		args.next()
			.unwrap_or_else(|| "crates/public-server/openapi.json".into()),
	);
	let output = PathBuf::from(
		args.next()
			.unwrap_or_else(|| "crates/canopy-api/src/generated.rs".into()),
	);
	let manifest = PathBuf::from(
		args.next()
			.unwrap_or_else(|| "crates/canopy-api/Cargo.toml".into()),
	);

	match generate(&input) {
		Ok((source, version)) => {
			if let Err(err) = fs::write(&output, source) {
				eprintln!("writing {}: {err}", output.display());
				return ExitCode::FAILURE;
			}
			if let Err(err) = stamp_version(&manifest, &version) {
				eprintln!("stamping {}: {err}", manifest.display());
				return ExitCode::FAILURE;
			}
			ExitCode::SUCCESS
		}
		Err(err) => {
			eprintln!("generating from {}: {err}", input.display());
			ExitCode::FAILURE
		}
	}
}

/// Write `version` into the `[package]` section of the crate's manifest.
///
/// The document declares the version and the crate takes it, so the manifest is
/// an output of generation rather than a second place the number is kept.
fn stamp_version(manifest: &PathBuf, version: &str) -> Result<(), String> {
	let text = fs::read_to_string(manifest).map_err(|err| err.to_string())?;
	let stamped = stamp(&text, version)?;
	if stamped == text {
		return Ok(());
	}
	fs::write(manifest, stamped).map_err(|err| err.to_string())
}

/// Replace the `[package]` section's version line, and only that line.
///
/// Dependency versions live in their own sections and are none of generation's
/// business, so the search is bounded to the `[package]` table.
fn stamp(text: &str, version: &str) -> Result<String, String> {
	let package = text
		.find("[package]")
		.ok_or("manifest has no [package] section")?;
	// The section runs to the next table header, or to the end of the file.
	let end = text[package + 1..]
		.find("\n[")
		.map(|at| package + at + 2)
		.unwrap_or(text.len());

	let line = text[package..end]
		.find("\nversion = ")
		.map(|at| package + at + 1)
		.ok_or("[package] section declares no version")?;
	let line_end = text[line..]
		.find('\n')
		.map(|at| line + at)
		.unwrap_or(text.len());

	Ok(format!(
		"{}version = {version:?}{}",
		&text[..line],
		&text[line_end..]
	))
}

fn generate(input: &PathBuf) -> Result<(String, String), String> {
	let text = fs::read_to_string(input).map_err(|err| err.to_string())?;
	let spec: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;

	let version = spec
		.pointer("/info/version")
		.and_then(Value::as_str)
		.filter(|version| !version.is_empty())
		.ok_or(
			"document declares no info.version; it is the version the crate takes, so \
			 generation cannot settle one for it",
		)?
		.to_owned();
	// BLAKE3, and named for it, because `bestool-canopy` — the crate consumers are
	// migrating off — exposed the document's digest as `OPENAPI_BLAKE3`. Keeping the
	// algorithm and the name makes that migration a substitution rather than a change.
	let digest = blake3::hash(text.as_bytes()).to_hex().to_string();

	let mut schemas = spec
		.pointer("/components/schemas")
		.and_then(Value::as_object)
		.ok_or("document has no components.schemas")?
		.clone();

	let open = open_objects_to_additional_properties(&mut schemas);
	timestamps_to_ref(&mut schemas);
	secrets_to_ref(&mut schemas)?;
	schemas.insert(
		TIMESTAMP.into(),
		json!({"type": "string", "format": "date-time"}),
	);
	schemas.insert(SECRET.into(), json!({"type": "string"}));

	let root: RootSchema = serde_json::from_value(json!({
		"$schema": "https://json-schema.org/draft/2020-12/schema",
		"definitions": schemas,
	}))
	.map_err(|err| format!("building a JSON Schema root: {err}"))?;

	let mut settings = TypeSpaceSettings::default();
	settings.with_struct_builder(false);
	settings.with_replacement(TIMESTAMP, "::jiff::Timestamp", [].into_iter());
	settings.with_replacement(
		SECRET,
		"crate::Redacted<::std::string::String>",
		[].into_iter(),
	);
	let mut space = TypeSpace::new(&settings);
	space
		.add_root_schema(root)
		.map_err(|err| format!("emitting wire types: {err}"))?;

	let mut file: syn::File = syn::parse2(space.to_stream())
		.map_err(|err| format!("parsing the emitted types: {err}"))?;
	let carried = add_further_keys(&mut file.items, &open);
	if let Some(missed) = open.iter().find(|name| !carried.contains(name)) {
		return Err(format!(
			"{missed} accepts further keys but its generated type has nowhere to carry them, so \
			 a consumer could neither send nor read them"
		));
	}
	relax_construction(&mut file.items);

	let mut out = String::from(
		"// @generated by canopy-api-codegen from crates/public-server/openapi.json.\n\
		 // Run `just gen-api` to refresh; do not edit by hand.\n\n",
	);
	out.push_str(&format!(
		"/// Version of the OpenAPI document this source was generated from, which is also\n\
		 /// this crate's own version.\n\
		 pub const OPENAPI_VERSION: &str = {version:?};\n\n\
		 /// BLAKE3 digest of that document, so a document that changed without the\n\
		 /// version moving with it can be told from one that did not.\n\
		 pub const OPENAPI_BLAKE3: &str = {digest:?};\n\n"
	));
	out.push_str(&prettyplease::unparse(&file));
	out.push('\n');
	out.push_str(&methods(&spec)?);
	Ok((out, version))
}

/// Rewrite an `allOf` of a free-form object beside a typed object into the typed
/// object carrying arbitrary further keys.
///
/// utoipa renders a `#[serde(flatten)]` catch-all field this way. Left as-is the
/// free-form member cannot be typed, so the whole schema would have to degrade to
/// untyped JSON; folded into `additionalProperties` it emits the declared fields
/// plus a map holding the rest, which is what the Rust type it came from is.
fn open_objects_to_additional_properties(schemas: &mut Map<String, Value>) -> Vec<String> {
	let mut folded = Vec::new();
	for (name, schema) in schemas.iter_mut() {
		let Some(members) = schema.get("allOf").and_then(Value::as_array).cloned() else {
			continue;
		};

		let free_form = |m: &Value| {
			m.get("type").and_then(Value::as_str) == Some("object")
				&& m.get("properties").is_none()
				&& m.get("$ref").is_none()
		};
		let (open, typed): (Vec<_>, Vec<_>) = members.iter().partition(|m| free_form(m));
		if open.is_empty() || typed.len() != 1 {
			continue;
		}

		let mut object = typed[0].as_object().cloned().unwrap_or_default();
		object.insert("additionalProperties".into(), Value::Bool(true));
		// Keep whichever description carries the prose: the outer one if the
		// schema has its own, else the free-form member's.
		let description = schema
			.get("description")
			.or_else(|| open[0].get("description"))
			.cloned();
		if let Some(description) = description {
			object.insert("description".into(), description);
		}
		*schema = Value::Object(object);
		folded.push(name.clone());
	}
	folded
}

/// Give each folded schema's struct the map field holding its further keys.
///
/// typify emits a map for a schema that is only `additionalProperties`, but drops
/// `additionalProperties` from a schema that also has declared properties, so the
/// catch-all is added here. Without it a consumer could not send or read the
/// further keys, which for a status push is the whole per-check detail.
fn add_further_keys(items: &mut [syn::Item], folded: &[String]) -> Vec<String> {
	let mut done = Vec::new();
	for item in items {
		match item {
			syn::Item::Struct(item) => {
				let name = item.ident.to_string();
				if !folded.contains(&name) {
					continue;
				}
				if let syn::Fields::Named(fields) = &mut item.fields {
					if fields
						.named
						.iter()
						.any(|field| field.ident.as_ref().is_some_and(|ident| ident == "extra"))
					{
						continue;
					}
					fields.named.push(syn::parse_quote! {
						/// Any further keys the schema accepts alongside those above,
						/// carried verbatim.
						#[serde(flatten)]
						#[builder(default)]
						pub extra: ::serde_json::Map<::std::string::String, ::serde_json::Value>
					});
					done.push(name);
				}
			}
			syn::Item::Mod(item) => {
				if let Some((_, items)) = &mut item.content {
					done.extend(add_further_keys(items, folded));
				}
			}
			_ => {}
		}
	}
	done
}

/// Point every `date-time` property at the synthetic timestamp schema.
///
/// A nullable one becomes a `oneOf` of null and the reference, which is how the
/// document already expresses a nullable reference.
fn timestamps_to_ref(schemas: &mut Map<String, Value>) {
	for schema in schemas.values_mut() {
		let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
			continue;
		};
		for property in properties.values_mut() {
			if property.get("format").and_then(Value::as_str) != Some("date-time") {
				continue;
			}
			let nullable = property
				.get("type")
				.and_then(Value::as_array)
				.is_some_and(|types| types.iter().any(|t| t.as_str() == Some("null")));
			let description = property.get("description").cloned();
			let reference = json!({"$ref": format!("#/definitions/{TIMESTAMP}")});
			let mut rewritten = if nullable {
				json!({"oneOf": [{"type": "null"}, reference]})
			} else {
				reference
			};
			if let Some(description) = description {
				rewritten["description"] = description;
			}
			*property = rewritten;
		}
	}
}

/// Point each declared secret property at the synthetic secret schema.
fn secrets_to_ref(schemas: &mut Map<String, Value>) -> Result<(), String> {
	for (schema_name, property_name) in SECRETS {
		let property = schemas
			.get_mut(*schema_name)
			.and_then(|schema| schema.get_mut("properties"))
			.and_then(Value::as_object_mut)
			.and_then(|properties| properties.get_mut(*property_name))
			.ok_or_else(|| {
				format!(
					"the document has no {schema_name}.{property_name}, which is declared a \
					 credential secret; a renamed field must be renamed in SECRETS too, or its \
					 value would stop being redacted"
				)
			})?;

		let description = property.get("description").cloned();
		let mut rewritten = json!({"$ref": format!("#/definitions/{SECRET}")});
		if let Some(description) = description {
			rewritten["description"] = description;
		}
		*property = rewritten;
	}
	Ok(())
}

/// Make every generated struct constructible without naming each field, and stop
/// literal construction from other crates.
///
/// The document evolves independently of any consumer, so a struct built with a
/// literal would break the moment canopy adds a field. A builder lets
/// construction name only the fields it sets, and `#[non_exhaustive]` makes the
/// builder the only way in, which is also what makes adding an optional property
/// a compatible change.
///
/// Only named-field structs get this: a builder cannot be derived on an enum or a
/// tuple struct, and an empty struct has nothing to build.
fn relax_construction(items: &mut [syn::Item]) {
	for item in items {
		match item {
			syn::Item::Struct(item) => {
				if let syn::Fields::Named(fields) = &item.fields
					&& !fields.named.is_empty()
				{
					item.attrs
						.push(syn::parse_quote!(#[derive(::bon::Builder)]));
					item.attrs.push(syn::parse_quote!(#[non_exhaustive]));
				}
			}
			syn::Item::Mod(item) => {
				if let Some((_, items)) = &mut item.content {
					relax_construction(items);
				}
			}
			_ => {}
		}
	}
}

/// One operation, as the client method it becomes.
struct Operation {
	name: String,
	verb: String,
	path: String,
	params: Vec<String>,
	query: Vec<QueryParam>,
	body: Option<Body>,
	response: Option<String>,
	summary: Option<String>,
	description: Option<String>,
}

/// What an operation takes as its request body.
enum Body {
	/// A `$ref`-ed schema, sent as JSON.
	Json(String),
	/// Bytes canopy holds, sent as they are.
	Octets,
}

/// A query parameter, which is a string on the wire whatever it is in the
/// document: the generated client renders it, and the server parses it.
struct QueryParam {
	name: String,
	required: bool,
}

/// Emit one method per operation on `CanopyClient`, routed through its shared
/// call plumbing. Names come from the path; where a path is served by more than
/// one verb the verb is prefixed to tell them apart.
fn methods(spec: &Value) -> Result<String, String> {
	let paths = spec
		.pointer("/paths")
		.and_then(Value::as_object)
		.ok_or("document has no paths")?;

	let mut operations = Vec::new();
	for (path, item) in paths {
		let item = item.as_object().ok_or("a path item is not an object")?;
		for (verb, op) in item {
			if !HTTP_VERBS.contains(&verb.as_str()) {
				continue;
			}
			operations.push(Operation {
				name: path
					.split('/')
					.filter(|seg| !seg.is_empty() && !seg.starts_with('{'))
					.map(|seg| seg.replace('-', "_"))
					.collect::<Vec<_>>()
					.join("_"),
				verb: verb.clone(),
				path: path.clone(),
				params: path
					.split('/')
					.filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
					.map(str::to_owned)
					.collect(),
				query: query_params(op),
				body: request_body(op, path, verb)?,
				response: response(op, path, verb)?,
				summary: op.get("summary").and_then(Value::as_str).map(str::to_owned),
				description: op
					.get("description")
					.and_then(Value::as_str)
					.map(str::to_owned),
			});
		}
	}

	let mut counts = BTreeMap::<&str, usize>::new();
	for op in &operations {
		*counts.entry(op.name.as_str()).or_default() += 1;
	}
	let collides: Vec<String> = operations
		.iter()
		.filter(|op| counts[op.name.as_str()] > 1)
		.map(|op| op.name.clone())
		.collect();

	operations.sort_by(|a, b| (&a.path, &a.verb).cmp(&(&b.path, &b.verb)));

	let mut out = String::from(
		"/// One method per operation in canopy's OpenAPI document.\n\
		 impl<T: crate::CanopyTransport> crate::CanopyClient<T> {\n",
	);
	for op in &operations {
		let method = if collides.contains(&op.name) {
			format!("{}_{}", op.verb, op.name)
		} else {
			op.name.clone()
		};

		let mut args = String::new();
		for param in &op.params {
			args.push_str(&format!(", {param}: &str"));
		}
		match &op.body {
			Some(Body::Json(ty)) => args.push_str(&format!(", body: &{ty}")),
			Some(Body::Octets) => args.push_str(", body: ::bytes::Bytes"),
			None => {}
		}
		for param in &op.query {
			let name = escape_ident(&param.name);
			if param.required {
				args.push_str(&format!(", {name}: &str"));
			} else {
				args.push_str(&format!(", {name}: Option<&str>"));
			}
		}

		let path = if op.params.is_empty() {
			format!("{:?}", op.path)
		} else {
			let mut template = op.path.clone();
			for param in &op.params {
				template = template.replace(&format!("{{{param}}}"), "{}");
			}
			format!("&format!({template:?}, {})", op.params.join(", "))
		};
		let path = if op.query.is_empty() {
			path
		} else {
			format!("&crate::query(&{path}, &[{}])", query_pairs(&op.query))
		};

		let (call, ret) = match (&op.body, &op.response) {
			(Some(Body::Octets), Some(ty)) => ("call_octets", format!("crate::Result<{ty}>")),
			(_, Some(ty)) => ("call_json", format!("crate::Result<{ty}>")),
			(_, None) => ("call_empty", "crate::Result<()>".to_owned()),
		};

		for text in [&op.summary, &op.description].into_iter().flatten() {
			for line in text.lines() {
				if line.is_empty() {
					out.push_str("\t///\n");
				} else {
					out.push_str(&format!("\t/// {line}\n"));
				}
			}
			out.push_str("\t///\n");
		}
		out.push_str(&format!("\t/// `{} {}`\n", op.verb.to_uppercase(), op.path));
		let sent = match &op.body {
			Some(Body::Octets) => "body",
			Some(Body::Json(_)) => "Some(body)",
			None => "None::<&()>",
		};
		out.push_str(&format!(
			"\tpub async fn {method}(&self{args}) -> {ret} {{\n\
			 \t\tself.{call}(::http::Method::{}, {path}, {sent}).await\n\t}}\n",
			op.verb.to_uppercase(),
		));
	}
	out.push_str("}\n");
	Ok(out)
}

/// The Rust type of an operation's JSON request body, or `None` when it has none.
fn request_body(op: &Value, path: &str, verb: &str) -> Result<Option<Body>, String> {
	if op
		.pointer("/requestBody/content/application~1octet-stream")
		.is_some()
	{
		return Ok(Some(Body::Octets));
	}

	let Some(schema) = op.pointer("/requestBody/content/application~1json/schema") else {
		return Ok(None);
	};
	schema
		.get("$ref")
		.and_then(Value::as_str)
		.map(|reference| Some(Body::Json(type_name(reference))))
		.ok_or_else(|| untypeable("request body", path, verb, schema))
}

/// The operation's query parameters, in the order the document lists them so
/// the generated argument order does not move when an unrelated one is added.
fn query_params(op: &Value) -> Vec<QueryParam> {
	op.get("parameters")
		.and_then(Value::as_array)
		.map(|params| {
			params
				.iter()
				.filter(|param| param.get("in").and_then(Value::as_str) == Some("query"))
				.filter_map(|param| {
					Some(QueryParam {
						name: param.get("name").and_then(Value::as_str)?.to_owned(),
						required: param
							.get("required")
							.and_then(Value::as_bool)
							.unwrap_or(false),
					})
				})
				.collect()
		})
		.unwrap_or_default()
}

/// The `(name, value)` pairs a generated method hands to [`crate::query`].
fn query_pairs(query: &[QueryParam]) -> String {
	query
		.iter()
		.map(|param| {
			let ident = escape_ident(&param.name);
			if param.required {
				format!("({:?}, Some({ident}))", param.name)
			} else {
				format!("({:?}, {ident})", param.name)
			}
		})
		.collect::<Vec<_>>()
		.join(", ")
}

/// A parameter name as a Rust identifier: `type` and friends are legal names in
/// a document and reserved here.
fn escape_ident(name: &str) -> String {
	let ident = name.replace('-', "_");
	if syn::parse_str::<syn::Ident>(&ident).is_ok() {
		ident
	} else {
		format!("r#{ident}")
	}
}

/// The Rust type of an operation's success response, or `None` when it declares
/// no body to parse.
fn response(op: &Value, path: &str, verb: &str) -> Result<Option<String>, String> {
	let Some(schema) = op.pointer("/responses/200/content/application~1json/schema") else {
		return Ok(None);
	};
	if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
		return Ok(Some(type_name(reference)));
	}
	match schema.get("type").and_then(Value::as_str) {
		Some("array") => schema
			.pointer("/items/$ref")
			.and_then(Value::as_str)
			.map(|item| Some(format!("::std::vec::Vec<{}>", type_name(item))))
			.ok_or_else(|| untypeable("response", path, verb, schema)),
		// A map with a declared value type, e.g. check severities keyed by check
		// name. Its keys are dynamic, so they are not schema properties, but the
		// value type is declared and is carried through to the client.
		Some("object") => schema
			.pointer("/additionalProperties/$ref")
			.and_then(Value::as_str)
			.map(|value| {
				Some(format!(
					"::std::collections::HashMap<::std::string::String, {}>",
					type_name(value)
				))
			})
			.ok_or_else(|| untypeable("response", path, verb, schema)),
		_ => Err(untypeable("response", path, verb, schema)),
	}
}

/// Refuse to generate rather than fall back to untyped JSON.
///
/// Every operation is reached through its generated types, so a schema this
/// generator cannot express is a defect to fix in the document or here.
fn untypeable(what: &str, path: &str, verb: &str, schema: &Value) -> String {
	format!(
		"the {what} of {} {path} cannot be typed, and an operation is not served by an untyped \
		 JSON body: give the schema a $ref, an array of $ref, or additionalProperties with a \
		 $ref, or teach this generator the shape.\nschema: {schema}",
		verb.to_uppercase(),
	)
}

/// Last path segment of a `#/components/schemas/Foo` reference.
fn type_name(reference: &str) -> String {
	reference
		.rsplit('/')
		.next()
		.expect("a reference is non-empty")
		.to_owned()
}

#[cfg(test)]
mod tests {
	use super::stamp;

	const MANIFEST: &str = "\
[package]
publish = true
name = \"bes-canopy-api\"
version = \"0.0.0\"
edition = \"2024\"

[dependencies]
serde_json = \"1.0.150\"
version = \"not-a-real-key\"
";

	#[test]
	fn stamps_the_package_version() {
		let out = stamp(MANIFEST, "1.2.3").unwrap();
		assert!(out.contains("version = \"1.2.3\"\nedition"));
	}

	#[test]
	fn leaves_dependency_versions_alone() {
		let out = stamp(MANIFEST, "1.2.3").unwrap();
		assert!(out.contains("serde_json = \"1.0.150\""));
		// A `version` key in a later table is not the package's version, even
		// though it matches the same pattern.
		assert!(out.contains("version = \"not-a-real-key\""));
	}

	#[test]
	fn is_idempotent() {
		let once = stamp(MANIFEST, "1.2.3").unwrap();
		assert_eq!(stamp(&once, "1.2.3").unwrap(), once);
	}

	#[test]
	fn stamping_the_same_version_changes_nothing() {
		assert_eq!(stamp(MANIFEST, "0.0.0").unwrap(), MANIFEST);
	}

	#[test]
	fn refuses_a_manifest_with_no_package_version() {
		let err = stamp("[package]\nname = \"x\"\n", "1.0.0").unwrap_err();
		assert!(err.contains("declares no version"), "{err}");
	}

	#[test]
	fn refuses_a_manifest_with_no_package_section() {
		let err = stamp("[workspace]\nmembers = []\n", "1.0.0").unwrap_err();
		assert!(err.contains("no [package] section"), "{err}");
	}

	#[test]
	fn a_package_section_at_the_end_of_the_file_is_still_found() {
		let text = "[dependencies]\nserde = \"1\"\n\n[package]\nversion = \"0.0.0\"\n";
		let out = stamp(text, "2.0.0").unwrap();
		assert!(out.contains("[package]\nversion = \"2.0.0\""), "{out}");
		assert!(out.contains("serde = \"1\""), "{out}");
	}
}
