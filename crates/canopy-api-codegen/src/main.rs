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

use std::{
	collections::{BTreeMap, BTreeSet},
	fs,
	path::PathBuf,
	process::ExitCode,
};

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

	// Captured before the map is consumed: an envelope named after an operation
	// must not collide with a type named after a schema.
	let schema_names: BTreeSet<String> = schemas.keys().cloned().collect();

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
	out.push_str(&methods(&spec, &schema_names)?);
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

/// Operations whose method was published before this generator could express
/// their request body, listed as `(verb, path)`.
///
/// A published method cannot gain an argument without breaking every call site,
/// so each of these widens its last path parameter to `impl Into<…Request>`
/// instead. The envelope converts from anything string-like, so a call written
/// against the published signature still compiles and still sends what it sent
/// before, while a caller that needs the body builds the envelope and passes
/// that.
///
/// Every other operation carrying a body or a query parameter takes its envelope
/// as a trailing argument, which is the shape to prefer; this list is the ledger
/// of what predates it, and a new operation never joins it. An entry matching no
/// operation fails generation, so renaming one of these paths surfaces here
/// rather than silently reshaping a published method.
const GRANDFATHERED: &[(&str, &str)] = &[
	(
		"post",
		"/artifacts/groups/{group}/{version}/{artifact_type}/{platform}",
	),
	("post", "/artifacts/{version}/{artifact_type}/{platform}"),
	("post", "/versions/{version}"),
];

/// What an operation takes as its request body.
#[derive(Debug)]
enum Body {
	/// A `$ref`-ed schema, sent as JSON.
	Json(String),
	/// Text canopy reads as one value, such as a download URL.
	Text,
	/// Bytes canopy holds, sent as they are.
	Octets,
}

impl Body {
	/// The Rust type of the envelope field carrying it.
	fn field_type(&self) -> String {
		match self {
			Self::Json(ty) => ty.clone(),
			Self::Text => "::std::string::String".to_owned(),
			Self::Octets => "::bytes::Bytes".to_owned(),
		}
	}

	/// The media type it is sent under.
	fn content_type(&self) -> &'static str {
		match self {
			Self::Json(_) => "application/json",
			Self::Text => "text/plain",
			Self::Octets => "application/octet-stream",
		}
	}
}

/// A query parameter, as the envelope field it becomes.
struct QueryParam {
	name: String,
	required: bool,
	ty: String,
}

/// The single request argument an operation's method takes.
///
/// Query parameters live here rather than as arguments of their own, so that
/// adding one later adds an optional field to a `#[non_exhaustive]` struct with
/// a builder — which is compatible — rather than changing a method's arity,
/// which is not. That is the rule the document itself uses for adding an
/// optional property, applied to parameters.
struct Envelope {
	name: String,
	/// The path parameter this envelope absorbs, for a grandfathered method.
	/// That method takes the parameter as `impl Into<…>`, so the envelope has to
	/// carry it in the parameter's place.
	absorbed: Option<String>,
	body: Option<Body>,
	query: Vec<QueryParam>,
}

/// One operation, as the client method it becomes.
struct Operation {
	name: String,
	verb: String,
	path: String,
	params: Vec<String>,
	/// The body an operation carries directly, for a method that predates the
	/// envelope and takes its JSON body as an argument of its own.
	body: Option<String>,
	envelope: Option<Envelope>,
	response: Option<String>,
	summary: Option<String>,
	description: Option<String>,
}

/// Emit one method per operation on `CanopyClient`, routed through its shared
/// call plumbing, preceded by the request envelopes those methods take. Names
/// come from the path; where a path is served by more than one verb the verb is
/// prefixed to tell them apart.
fn methods(spec: &Value, schema_names: &BTreeSet<String>) -> Result<String, String> {
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
			let params: Vec<String> = path
				.split('/')
				.filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
				.map(str::to_owned)
				.collect();
			let body = request_body(op, path, verb)?;
			let query = query_params(op, path, verb)?;
			// An operation with a plain JSON body and nothing else keeps that body
			// as its own argument, which is how every such method was published.
			// Anything else travels in an envelope.
			let (body, envelope) = if query.is_empty() && matches!(body, None | Some(Body::Json(_)))
			{
				let body = match body {
					Some(Body::Json(ty)) => Some(ty),
					_ => None,
				};
				(body, None)
			} else {
				let operation_id =
					op.get("operationId")
						.and_then(Value::as_str)
						.ok_or_else(|| {
							format!(
								"{} {path} declares no operationId, which is what its request envelope \
						 would be named after",
								verb.to_uppercase()
							)
						})?;
				let envelope = envelope(
					operation_id,
					path,
					verb,
					&params,
					body,
					query,
					GRANDFATHERED.contains(&(verb.as_str(), path.as_str())),
					schema_names,
				)?;
				(None, Some(envelope))
			};

			// The method names its request argument `body` or `request`; a path
			// parameter taking that name would be shadowed by it, and the path
			// would be built from the wrong value.
			let mut reserved = Vec::new();
			if body.is_some() {
				reserved.push("body");
			}
			if envelope.is_some() {
				reserved.push("request");
			}
			if let Some(clash) = params
				.iter()
				.find(|param| reserved.contains(&param.as_str()))
			{
				return Err(format!(
					"the path parameter {clash} of {} {path} has the name this method gives the \
					 request it carries, which would shadow it: rename the parameter",
					verb.to_uppercase()
				));
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
				params,
				body,
				envelope,
				response: response(op, path, verb)?,
				summary: op.get("summary").and_then(Value::as_str).map(str::to_owned),
				description: op
					.get("description")
					.and_then(Value::as_str)
					.map(str::to_owned),
			});
		}
	}

	// A ledger entry matching nothing means a grandfathered path moved, which
	// would quietly reshape the method that entry exists to hold still.
	for (verb, path) in GRANDFATHERED {
		if !operations
			.iter()
			.any(|op| op.verb == *verb && op.path == *path)
		{
			return Err(format!(
				"{} {path} is listed as a method published before its body could be expressed, \
                 but the document has no such operation: the path moved, and the method it names \
                 would change shape",
				verb.to_uppercase()
			));
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

	let mut out = String::new();
	for op in &operations {
		if let Some(envelope) = &op.envelope {
			out.push_str(&envelope_type(envelope, &op.verb, &op.path));
		}
	}

	out.push_str(
		"/// One method per operation in canopy's OpenAPI document.\n\
         impl<T: crate::CanopyTransport> crate::CanopyClient<T> {\n",
	);
	for op in &operations {
		let method = if collides.contains(&op.name) {
			format!("{}_{}", op.verb, op.name)
		} else {
			op.name.clone()
		};
		let absorbed = op
			.envelope
			.as_ref()
			.and_then(|envelope| envelope.absorbed.as_deref());

		let mut args = String::new();
		for param in &op.params {
			if Some(param.as_str()) == absorbed {
				let name = &op
					.envelope
					.as_ref()
					.expect("absorbed implies an envelope")
					.name;
				args.push_str(&format!(
					", {}: impl ::std::convert::Into<{name}>",
					escape_ident(param)
				));
			} else {
				args.push_str(&format!(", {}: &str", escape_ident(param)));
			}
		}
		if let Some(body) = &op.body {
			args.push_str(&format!(", body: &{body}"));
		}
		if let Some(envelope) = &op.envelope
			&& envelope.absorbed.is_none()
		{
			args.push_str(&format!(", request: {}", envelope.name));
		}

		let path = if op.params.is_empty() {
			format!("{:?}", op.path)
		} else {
			let mut template = op.path.clone();
			for param in &op.params {
				template = template.replace(&format!("{{{param}}}"), "{}");
			}
			let filled: Vec<String> = op
				.params
				.iter()
				.map(|param| {
					if Some(param.as_str()) == absorbed {
						format!("request.{}", escape_ident(param))
					} else {
						escape_ident(param)
					}
				})
				.collect();
			format!("&format!({template:?}, {})", filled.join(", "))
		};
		let path = match &op.envelope {
			Some(envelope) if !envelope.query.is_empty() => {
				format!("&crate::query({path}, &[{}])", query_pairs(&envelope.query))
			}
			_ => path,
		};

		let sent = match &op.envelope {
			None if op.body.is_some() => "Some(body)".to_owned(),
			None => "None::<&()>".to_owned(),
			Some(envelope) => match &envelope.body {
				None => "None::<&()>".to_owned(),
				Some(Body::Json(_)) => "Some(&request.body)".to_owned(),
				Some(body) => payload(body, envelope.absorbed.is_some()),
			},
		};
		let raw = matches!(
			&op.envelope,
			Some(Envelope {
				body: Some(Body::Text | Body::Octets),
				..
			})
		);
		let (call, ret) = match (raw, &op.response) {
			(true, Some(ty)) => ("call_payload_json", format!("crate::Result<{ty}>")),
			(true, None) => ("call_payload_empty", "crate::Result<()>".to_owned()),
			(false, Some(ty)) => ("call_json", format!("crate::Result<{ty}>")),
			(false, None) => ("call_empty", "crate::Result<()>".to_owned()),
		};
		let tail = match &op.envelope {
			Some(envelope) if raw => format!(
				"{sent}, {:?}",
				envelope
					.body
					.as_ref()
					.expect("a raw call has a body")
					.content_type()
			),
			_ => sent,
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
		out.push_str(&format!(
			"\tpub async fn {method}(&self{args}) -> {ret} {{\n"
		));
		if let Some(absorbed) = absorbed {
			let name = &op
				.envelope
				.as_ref()
				.expect("absorbed implies an envelope")
				.name;
			out.push_str(&format!(
				"\t\tlet request: {name} = {}.into();\n",
				escape_ident(absorbed)
			));
		}
		out.push_str(&format!(
			"\t\tself.{call}(::http::Method::{}, {path}, {tail}).await\n\t}}\n",
			op.verb.to_uppercase(),
		));
	}
	out.push_str("}\n");
	Ok(out)
}

/// The expression handing an envelope's non-JSON body to the call plumbing.
///
/// A grandfathered envelope's body is optional, because the signature it holds
/// still could not send one; absent, it sends the empty payload that signature
/// has always sent.
fn payload(body: &Body, optional: bool) -> String {
	match (body, optional) {
		(Body::Text, true) => "request.body.map(::bytes::Bytes::from).unwrap_or_default()".into(),
		(Body::Text, false) => "::bytes::Bytes::from(request.body)".into(),
		(Body::Octets, true) => "request.body.unwrap_or_default()".into(),
		(Body::Octets, false) => "request.body".into(),
		(Body::Json(_), _) => unreachable!("a JSON body is not sent as a payload"),
	}
}

/// The `(name, value)` pairs a generated method hands to `crate::query`.
fn query_pairs(query: &[QueryParam]) -> String {
	query
		.iter()
		.map(|param| {
			let ident = escape_ident(&param.name);
			if param.required {
				format!(
					"({:?}, Some(::std::string::ToString::to_string(&request.{ident})))",
					param.name
				)
			} else {
				format!(
					"({:?}, request.{ident}.as_ref().map(::std::string::ToString::to_string))",
					param.name
				)
			}
		})
		.collect::<Vec<_>>()
		.join(", ")
}

/// Build the request argument an operation's method takes.
///
/// Everything the operation carries travels in one struct, so that what it
/// carries can grow without the method's arity moving.
#[allow(clippy::too_many_arguments)]
fn envelope(
	operation_id: &str,
	path: &str,
	verb: &str,
	params: &[String],
	body: Option<Body>,
	query: Vec<QueryParam>,
	grandfathered: bool,
	schema_names: &BTreeSet<String>,
) -> Result<Envelope, String> {
	let name = format!("{}Request", pascal(operation_id));
	if schema_names.contains(&name) {
		return Err(format!(
			"the request envelope for {} {path} would be named {name}, which the document already \
             declares as a schema: rename one of the two",
			verb.to_uppercase()
		));
	}

	let absorbed = if grandfathered {
		Some(params.last().cloned().ok_or_else(|| {
			format!(
				"{} {path} is listed as a method published before its body could be expressed, \
                 but it has no path parameter to widen, so its published signature has nowhere to \
                 carry a request",
				verb.to_uppercase()
			)
		})?)
	} else {
		None
	};

	let mut fields: Vec<String> = Vec::new();
	fields.extend(absorbed.clone());
	if body.is_some() {
		fields.push("body".to_owned());
	}
	fields.extend(query.iter().map(|param| param.name.clone()));
	for (at, field) in fields.iter().enumerate() {
		if fields[..at].contains(field) {
			return Err(format!(
				"the request envelope for {} {path} would carry two fields named {field}",
				verb.to_uppercase()
			));
		}
	}

	Ok(Envelope {
		name,
		absorbed,
		body,
		query,
	})
}

/// Emit an envelope as the struct a consumer builds, and — where it stands in
/// for a path parameter a published method took — the conversion that keeps that
/// method's call sites compiling.
fn envelope_type(envelope: &Envelope, verb: &str, path: &str) -> String {
	let mut out = format!("/// Request for `{} {path}`.\n", verb.to_uppercase());
	if let Some(absorbed) = &envelope.absorbed {
		out.push_str("///\n");
		out.push_str(&format!(
			"/// Converts from anything string-like, which is what this method took in place of\n\
             /// `{absorbed}` before it carried a request, so a call written against that\n\
             /// signature compiles unchanged and sends what it always sent.\n"
		));
	}
	out.push_str("#[derive(Clone, Debug, ::bon::Builder)]\n#[non_exhaustive]\n");
	out.push_str(&format!("pub struct {} {{\n", envelope.name));

	let mut field = |name: &str, ty: &str, optional: bool| {
		if ty.ends_with("::String") || ty == "::bytes::Bytes" {
			out.push_str("\t#[builder(into)]\n");
		}
		let ty = if optional {
			format!("::std::option::Option<{ty}>")
		} else {
			ty.to_owned()
		};
		out.push_str(&format!("\tpub {}: {ty},\n", escape_ident(name)));
	};

	if let Some(absorbed) = &envelope.absorbed {
		field(absorbed, "::std::string::String", false);
	}
	if let Some(body) = &envelope.body {
		field("body", &body.field_type(), envelope.absorbed.is_some());
	}
	for param in &envelope.query {
		field(&param.name, &param.ty, !param.required);
	}
	out.push_str("}\n\n");

	if let Some(absorbed) = &envelope.absorbed {
		out.push_str(&format!(
			"impl<T: ::std::convert::AsRef<str> + ?Sized> ::std::convert::From<&T> for {} {{\n\
             \tfn from(value: &T) -> Self {{\n\t\tSelf {{\n",
			envelope.name
		));
		out.push_str(&format!(
			"\t\t\t{}: value.as_ref().to_owned(),\n",
			escape_ident(absorbed)
		));
		if envelope.body.is_some() {
			out.push_str("\t\t\tbody: ::std::option::Option::None,\n");
		}
		for param in &envelope.query {
			if param.required {
				unreachable!("a grandfathered method sent no query parameter, so none is required");
			}
			out.push_str(&format!(
				"\t\t\t{}: ::std::option::Option::None,\n",
				escape_ident(&param.name)
			));
		}
		out.push_str("\t\t}\n\t}\n}\n\n");
	}
	out
}

/// A parameter or property name as a Rust identifier: `type` and friends are
/// legal names in a document and reserved here.
fn escape_ident(name: &str) -> String {
	let ident = name.replace('-', "_");
	if syn::parse_str::<syn::Ident>(&ident).is_ok() {
		ident
	} else {
		format!("r#{ident}")
	}
}

/// An `operation_id` as a type name.
fn pascal(name: &str) -> String {
	name.split(['_', '-'])
		.filter(|part| !part.is_empty())
		.map(|part| {
			let mut chars = part.chars();
			match chars.next() {
				Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
				None => String::new(),
			}
		})
		.collect()
}

/// The request body an operation takes, or `None` when it declares none.
///
/// Every media type the client can send is named here, and one it cannot is an
/// error rather than a body quietly left behind: a method that drops the body
/// its operation requires cannot do the thing it is named for.
fn request_body(op: &Value, path: &str, verb: &str) -> Result<Option<Body>, String> {
	let Some(content) = op
		.pointer("/requestBody/content")
		.and_then(Value::as_object)
	else {
		return Ok(None);
	};
	let mut media = content.iter();
	let (media_type, holder) = match (media.next(), media.next()) {
		(Some(only), None) => only,
		// Which of several the client would pick is a decision the document
		// should be making, and a reader of the generated method cannot see.
		_ => {
			return Err(format!(
				"the request body of {} {path} declares more than one media type, and the client \
                 has no grounds to choose between them: declare the one canopy expects",
				verb.to_uppercase()
			));
		}
	};

	match media_type.as_str() {
		"application/json" => {
			let schema = holder
				.get("schema")
				.ok_or_else(|| untypeable("request body", path, verb, holder))?;
			schema
				.get("$ref")
				.and_then(Value::as_str)
				.map(|reference| Some(Body::Json(type_name(reference))))
				.ok_or_else(|| untypeable("request body", path, verb, schema))
		}
		"text/plain" => Ok(Some(Body::Text)),
		"application/octet-stream" => Ok(Some(Body::Octets)),
		other => Err(format!(
			"the request body of {} {path} is {other}, which this generator cannot send: teach it \
             the media type, or declare one it knows (application/json, text/plain, \
             application/octet-stream). An operation is not served by a method that leaves its \
             body behind.",
			verb.to_uppercase()
		)),
	}
}

/// The query parameters an operation takes, in the order the document lists
/// them.
fn query_params(op: &Value, path: &str, verb: &str) -> Result<Vec<QueryParam>, String> {
	let Some(params) = op.get("parameters").and_then(Value::as_array) else {
		return Ok(Vec::new());
	};

	let mut out = Vec::new();
	for param in params {
		if param.get("in").and_then(Value::as_str) != Some("query") {
			continue;
		}
		let name = param.get("name").and_then(Value::as_str).ok_or_else(|| {
			format!(
				"a query parameter of {} {path} has no name",
				verb.to_uppercase()
			)
		})?;
		let schema = param.get("schema").ok_or_else(|| {
			format!(
				"the query parameter {name} of {} {path} declares no schema",
				verb.to_uppercase()
			)
		})?;
		let ty = query_type(schema).ok_or_else(|| {
			format!(
				"the query parameter {name} of {} {path} cannot be typed, and a parameter is not \
                 served by being left off: declare it as a string, an integer, or a boolean, or \
                 teach this generator the shape.\nschema: {schema}",
				verb.to_uppercase()
			)
		})?;
		out.push(QueryParam {
			name: name.to_owned(),
			required: param
				.get("required")
				.and_then(Value::as_bool)
				.unwrap_or(false),
			ty,
		});
	}
	Ok(out)
}

/// The Rust type of a query parameter.
///
/// A parameter is a string on the wire, but it is typed here as the document
/// describes it, so a caller hands over the thing itself rather than its
/// spelling.
fn query_type(schema: &Value) -> Option<String> {
	let format = schema.get("format").and_then(Value::as_str);
	let ty = match schema.get("type") {
		Some(Value::String(ty)) => ty.clone(),
		// An optional parameter may be rendered as a union with null.
		Some(Value::Array(types)) => {
			let mut named = types
				.iter()
				.filter_map(Value::as_str)
				.filter(|ty| *ty != "null");
			let ty = named.next()?.to_owned();
			if named.next().is_some() {
				return None;
			}
			ty
		}
		_ => return None,
	};

	Some(match (ty.as_str(), format) {
		("string", Some("uuid")) => "::uuid::Uuid".to_owned(),
		("string", _) => "::std::string::String".to_owned(),
		("integer", _) => "i64".to_owned(),
		("boolean", _) => "bool".to_owned(),
		_ => return None,
	})
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
	use std::collections::BTreeSet;

	use serde_json::{Value, json};

	use super::{Body, escape_ident, methods, pascal, query_type, request_body, stamp};

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

	/// A document carrying the operations the ledger names, so its check passes,
	/// plus whatever the test is about.
	fn spec_with(extra: Value) -> Value {
		let mut paths = json!({
			"/artifacts/groups/{group}/{version}/{artifact_type}/{platform}": {
				"post": {
					"operationId": "register_group_artifact",
					"requestBody": {"content": {"application/octet-stream": {"schema": {}}}},
				},
			},
			"/artifacts/{version}/{artifact_type}/{platform}": {
				"post": {
					"operationId": "register_artifact",
					"requestBody": {"content": {"text/plain": {"schema": {}}}},
				},
			},
			"/versions/{version}": {
				"post": {
					"operationId": "create_version",
					"requestBody": {"content": {"text/plain": {"schema": {}}}},
				},
			},
		});
		let into = paths.as_object_mut().expect("the fixture is an object");
		for (path, item) in extra.as_object().expect("the extra is an object") {
			into.insert(path.clone(), item.clone());
		}
		json!({"paths": paths})
	}

	fn body_of(media: &str, schema: Value) -> Result<Option<Body>, String> {
		let op = json!({"requestBody": {"content": {media: {"schema": schema}}}});
		request_body(&op, "/a", "post")
	}

	#[test]
	fn a_body_the_client_can_send_is_typed_by_its_media_type() {
		assert!(matches!(
			body_of("application/octet-stream", json!({})),
			Ok(Some(Body::Octets))
		));
		assert!(matches!(
			body_of("text/plain", json!({"type": "string"})),
			Ok(Some(Body::Text))
		));
		assert!(matches!(
			body_of("application/json", json!({"$ref": "#/components/schemas/Args"})),
			Ok(Some(Body::Json(ty))) if ty == "Args"
		));
	}

	#[test]
	fn a_body_the_client_cannot_send_is_refused_rather_than_dropped() {
		let err = body_of("multipart/form-data", json!({})).unwrap_err();
		assert!(err.contains("leaves its body behind"), "{err}");
	}

	#[test]
	fn a_json_body_that_is_not_a_ref_is_refused() {
		let err = body_of("application/json", json!({"type": "object"})).unwrap_err();
		assert!(err.contains("cannot be typed"), "{err}");
	}

	#[test]
	fn a_body_declaring_several_media_types_is_refused() {
		let op = json!({
			"requestBody": {"content": {"text/plain": {"schema": {}}, "application/json": {"schema": {}}}},
		});
		let err = request_body(&op, "/a", "post").unwrap_err();
		assert!(err.contains("more than one media type"), "{err}");
	}

	#[test]
	fn an_operation_with_no_body_has_none() {
		assert!(matches!(request_body(&json!({}), "/a", "post"), Ok(None)));
	}

	#[test]
	fn a_query_parameter_is_typed_as_the_document_describes_it() {
		assert_eq!(
			query_type(&json!({"type": "string", "format": "uuid"})).as_deref(),
			Some("::uuid::Uuid")
		);
		assert_eq!(
			query_type(&json!({"type": "string"})).as_deref(),
			Some("::std::string::String")
		);
		assert_eq!(
			query_type(&json!({"type": "integer"})).as_deref(),
			Some("i64")
		);
		// An optional parameter may be rendered as a union with null.
		assert_eq!(
			query_type(&json!({"type": ["string", "null"], "format": "uuid"})).as_deref(),
			Some("::uuid::Uuid")
		);
		assert_eq!(query_type(&json!({"type": "object"})), None);
	}

	#[test]
	fn a_query_parameter_travels_in_the_envelope_rather_than_as_an_argument() {
		let spec = spec_with(json!({
			"/widgets": {
				"post": {
					"operationId": "make_widget",
					"parameters": [{"name": "shape", "in": "query", "schema": {"type": "string"}}],
					"requestBody": {"content": {"application/octet-stream": {"schema": {}}}},
				},
			},
		}));
		let out = methods(&spec, &BTreeSet::new()).expect("the fixture generates");

		assert!(out.contains("pub struct MakeWidgetRequest"), "{out}");
		assert!(
			out.contains("pub shape: ::std::option::Option<::std::string::String>"),
			"a parameter is a field, so adding another one later is a compatible change\n{out}"
		);
		assert!(
			out.contains("pub async fn widgets(&self, request: MakeWidgetRequest)"),
			"an operation with no published method takes its envelope as a trailing argument\n{out}"
		);
	}

	#[test]
	fn a_required_query_parameter_is_not_optional_and_is_always_sent() {
		let spec = spec_with(json!({
			"/widgets": {
				"post": {
					"operationId": "make_widget",
					"parameters": [{
						"name": "shape",
						"in": "query",
						"required": true,
						"schema": {"type": "string", "format": "uuid"},
					}],
				},
			},
		}));
		let out = methods(&spec, &BTreeSet::new()).expect("the fixture generates");

		assert!(
			out.contains("pub shape: ::uuid::Uuid"),
			"a required parameter is not an option\n{out}"
		);
		assert!(
			out.contains("(\"shape\", Some(::std::string::ToString::to_string(&request.shape)))"),
			"and is always placed in the query\n{out}"
		);
	}

	#[test]
	fn a_grandfathered_method_widens_its_last_path_parameter() {
		let out = methods(&spec_with(json!({})), &BTreeSet::new()).expect("the fixture generates");
		assert!(
			out.contains(
				"pub async fn artifacts(&self, version: &str, artifact_type: &str, platform: impl \
				 ::std::convert::Into<RegisterArtifactRequest>)"
			),
			"the published parameter widens in place, so its arity does not move\n{out}"
		);
		assert!(
			out.contains("impl<T: ::std::convert::AsRef<str> + ?Sized> ::std::convert::From<&T>"),
			"the conversion is what keeps a published call site compiling\n{out}"
		);
	}

	#[test]
	fn a_path_parameter_shadowing_the_request_argument_is_refused() {
		let spec = spec_with(json!({
			"/widgets/{request}": {
				"post": {
					"operationId": "make_widget",
					"parameters": [{"name": "shape", "in": "query", "schema": {"type": "string"}}],
				},
			},
		}));
		let err = methods(&spec, &BTreeSet::new()).unwrap_err();
		assert!(err.contains("would shadow it"), "{err}");
	}

	#[test]
	fn a_ledger_entry_matching_no_operation_is_refused() {
		let spec = json!({"paths": {"/unrelated": {"get": {"operationId": "unrelated"}}}});
		let err = methods(&spec, &BTreeSet::new()).unwrap_err();
		assert!(err.contains("the path moved"), "{err}");
	}

	#[test]
	fn an_envelope_colliding_with_a_schema_is_refused() {
		let names = BTreeSet::from(["RegisterArtifactRequest".to_owned()]);
		let err = methods(&spec_with(json!({})), &names).unwrap_err();
		assert!(err.contains("already declares as a schema"), "{err}");
	}

	#[test]
	fn a_name_that_is_a_keyword_is_escaped() {
		assert_eq!(escape_ident("type"), "r#type");
		assert_eq!(escape_ident("artifact-type"), "artifact_type");
		assert_eq!(escape_ident("platform"), "platform");
	}

	#[test]
	fn an_envelope_is_named_after_its_operation() {
		assert_eq!(pascal("register_group_artifact"), "RegisterGroupArtifact");
		assert_eq!(pascal("create_version"), "CreateVersion");
	}
}
