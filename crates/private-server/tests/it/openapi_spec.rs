//! Smoke test: the OpenAPI spec generates and exposes one path per migrated module.

use canopy_utoipa_axum::router::OpenApiRouter;
use private_server::openapi::ApiDoc;
use utoipa::OpenApi;

fn build_spec() -> serde_json::Value {
	let (_router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
		.merge(private_server::fns::routes())
		.split_for_parts();
	serde_json::to_value(&openapi).expect("openapi serializes")
}

#[test]
fn spec_has_security_schemes() {
	let spec = build_spec();
	let schemes = &spec["components"]["securitySchemes"];
	assert!(
		schemes["tailscale-admin"].is_object(),
		"tailscale-admin scheme present"
	);
	assert!(
		schemes["tailscale-user"].is_object(),
		"tailscale-user scheme present"
	);
}

#[test]
fn spec_has_problem_details_schema() {
	let spec = build_spec();
	let schema = &spec["components"]["schemas"]["ProblemDetailsSchema"];
	assert!(schema.is_object(), "ProblemDetailsSchema is registered");
}

/// Each module gets a representative path checked.
#[test]
fn spec_has_paths_for_every_module() {
	let spec = build_spec();
	let paths = &spec["paths"];
	for p in [
		"/api/admins/list",
		"/api/bestool/list_snippets",
		"/api/commons/public_url",
		"/api/devices/list_trusted",
		"/api/incidents/list_active",
		"/api/issues/list",
		"/api/fleet/groups/list",
		"/api/sql/is_sql_available",
		"/api/statuses/summary",
		"/api/versions/get_grouped_versions",
	] {
		assert!(paths[p].is_object(), "{p} present in spec");
	}
}

/// Drift-check: the committed `private-web/openapi.json` must match what the
/// current rust handlers produce. If this fails, run `just gen-openapi` and
/// commit the resulting diff.
#[test]
fn committed_spec_matches_generated() {
	let path = concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/../../private-web/openapi.json"
	);
	let raw = std::fs::read_to_string(path)
		.expect("read private-web/openapi.json — generate it with `just gen-openapi`");
	let committed: serde_json::Value =
		serde_json::from_str(&raw).expect("private-web/openapi.json is valid JSON");
	let generated = build_spec();
	assert!(
		committed == generated,
		"private-web/openapi.json is stale — run `just gen-openapi` to refresh and commit the diff",
	);
}

#[test]
fn spec_path_count() {
	let spec = build_spec();
	let paths = spec["paths"].as_object().expect("paths is an object");
	// Sanity bound: every handler in private-server is annotated. If this drops
	// far below ~70, someone removed annotations; if it climbs much higher,
	// new endpoints exist that should be reviewed.
	assert!(
		paths.len() >= 60,
		"expected >=60 paths in spec, got {}",
		paths.len()
	);
}

/// `operationId` must be unique across the whole document (OpenAPI requires it,
/// and codegen/tooling keys off it). utoipa defaults an operation's id to the
/// handler's function name, so two same-named handlers in different modules
/// silently collide — set an explicit `operation_id` in the `#[utoipa::path]`
/// to disambiguate.
#[test]
fn no_duplicate_operation_ids() {
	let spec = build_spec();
	let paths = spec["paths"].as_object().expect("paths object");

	let mut by_id: std::collections::BTreeMap<String, Vec<String>> = Default::default();
	for (path, item) in paths {
		let ops = item.as_object().expect("path item object");
		for (method, op) in ops {
			// Non-operation keys (parameters, summary, …) aren't objects with
			// an operationId, so they fall through this filter.
			if let Some(id) = op.get("operationId").and_then(|v| v.as_str()) {
				by_id
					.entry(id.to_string())
					.or_default()
					.push(format!("{} {path}", method.to_uppercase()));
			}
		}
	}

	let dupes: Vec<String> = by_id
		.iter()
		.filter(|(_, uses)| uses.len() > 1)
		.map(|(id, uses)| format!("{id}: {}", uses.join(", ")))
		.collect();
	assert!(
		dupes.is_empty(),
		"duplicate operationIds (invalid OpenAPI) — set an explicit `operation_id`:\n{}",
		dupes.join("\n"),
	);
}

/// Every operation must document what it does and why: `summary` for the
/// one-line label tooling shows in lists, `description` for the fuller
/// explanation. utoipa leaves both blank by default, so an endpoint added
/// without doc comments on its handler silently ships undocumented.
#[test]
fn every_operation_has_summary_and_description() {
	let spec = build_spec();
	let paths = spec["paths"].as_object().expect("paths object");

	let mut missing: Vec<String> = Vec::new();
	for (path, item) in paths {
		let ops = item.as_object().expect("path item object");
		for (method, op) in ops {
			let Some(id) = op.get("operationId").and_then(|v| v.as_str()) else {
				continue;
			};
			let has_summary = op
				.get("summary")
				.and_then(|v| v.as_str())
				.is_some_and(|s| !s.is_empty());
			let has_description = op
				.get("description")
				.and_then(|v| v.as_str())
				.is_some_and(|s| !s.is_empty());
			if !has_summary || !has_description {
				missing.push(format!(
					"{id} ({} {path}): {}",
					method.to_uppercase(),
					match (has_summary, has_description) {
						(false, false) => "missing summary and description",
						(false, true) => "missing summary",
						(true, false) => "missing description",
						(true, true) => unreachable!(),
					}
				));
			}
		}
	}

	assert!(
		missing.is_empty(),
		"operations missing doc comments — add `///` summary/description lines above the handler:\n{}",
		missing.join("\n"),
	);
}

/// utoipa's generic built-ins have no Rust type of their own to hang a doc
/// comment on, so they can never gain a `description`: a bare `BTreeMap<_>`
/// or `serde_json::Value` field is documented on the *containing* struct's
/// field instead. Everything else — every named request/response/component
/// schema — must describe what it represents.
const SCHEMAS_WITHOUT_DOC_HOOK: &[&str] = &["BTreeMap", "Value"];

#[test]
fn every_schema_has_description() {
	let spec = build_spec();
	let schemas = spec["components"]["schemas"]
		.as_object()
		.expect("schemas object");

	let undocumented: Vec<&str> = schemas
		.iter()
		.filter(|(name, schema)| {
			!SCHEMAS_WITHOUT_DOC_HOOK.contains(&name.as_str())
				&& schema
					.get("description")
					.and_then(|v| v.as_str())
					.is_none_or(|s| s.is_empty())
		})
		.map(|(name, _)| name.as_str())
		.collect();

	assert!(
		undocumented.is_empty(),
		"schemas missing a description — add a doc comment to the Rust type (or extend \
		 SCHEMAS_WITHOUT_DOC_HOOK if it's a generic built-in with nowhere to put one):\n{}",
		undocumented.join("\n"),
	);
}

/// Every property a schema exposes, including ones folded in through a
/// top-level `allOf` (utoipa emits some flattened structs as an `allOf` of
/// inline object schemas rather than a single flat `properties` map).
fn schema_properties(schema: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
	let mut props = Vec::new();
	if let Some(direct) = schema.get("properties").and_then(|v| v.as_object()) {
		props.extend(direct.iter().map(|(k, v)| (k.clone(), v.clone())));
	}
	if let Some(branches) = schema.get("allOf").and_then(|v| v.as_array()) {
		for branch in branches {
			if let Some(branch_props) = branch.get("properties").and_then(|v| v.as_object()) {
				props.extend(branch_props.iter().map(|(k, v)| (k.clone(), v.clone())));
			}
		}
	}
	props
}

/// A property is documented if it carries its own non-empty `description`
/// — which covers both plain fields and `$ref` fields with a doc comment,
/// since utoipa attaches the description directly on the ref object rather
/// than wrapping it (a bare `$ref` can't carry a sibling keyword in strict
/// OpenAPI 3.0, but utoipa emits it that way regardless and every consumer
/// here tolerates it) — or if it's a nullable reference, which utoipa
/// represents as `oneOf: [{"type": "null"}, {"$ref": ..., "description":
/// ...}]`, i.e. an `Option<T>` field where the description lives on the
/// ref branch rather than the wrapping property.
fn property_is_documented(prop: &serde_json::Value) -> bool {
	if prop
		.get("description")
		.and_then(|v| v.as_str())
		.is_some_and(|s| !s.is_empty())
	{
		return true;
	}
	prop.get("oneOf")
		.and_then(|v| v.as_array())
		.is_some_and(|branches| {
			branches.iter().any(|b| {
				b.get("$ref").is_some()
					&& b.get("description")
						.and_then(|v| v.as_str())
						.is_some_and(|s| !s.is_empty())
			})
		})
}

#[test]
fn every_schema_property_has_description() {
	let spec = build_spec();
	let schemas = spec["components"]["schemas"]
		.as_object()
		.expect("schemas object");

	let mut undocumented: Vec<String> = Vec::new();
	for (schema_name, schema) in schemas {
		for (prop_name, prop) in schema_properties(schema) {
			if !property_is_documented(&prop) {
				undocumented.push(format!("{schema_name}.{prop_name}"));
			}
		}
	}

	assert!(
		undocumented.is_empty(),
		"schema fields missing a description — add a doc comment above the Rust field:\n{}",
		undocumented.join("\n"),
	);
}

/// Every tag an operation uses must be registered on the document with a
/// non-empty description (the `#[openapi(tags(...))]` list on `ApiDoc`),
/// so the generated docs group endpoints under a labelled section instead
/// of an unexplained bare name.
#[test]
fn every_operation_tag_is_registered() {
	let spec = build_spec();
	let registered: std::collections::BTreeSet<&str> = spec["tags"]
		.as_array()
		.expect("tags array")
		.iter()
		.filter(|tag| {
			tag.get("description")
				.and_then(|v| v.as_str())
				.is_some_and(|s| !s.is_empty())
		})
		.filter_map(|tag| tag.get("name").and_then(|v| v.as_str()))
		.collect();

	let paths = spec["paths"].as_object().expect("paths object");
	let mut unregistered: std::collections::BTreeSet<String> = Default::default();
	for item in paths.values() {
		let ops = item.as_object().expect("path item object");
		for op in ops.values() {
			let Some(tags) = op.get("tags").and_then(|v| v.as_array()) else {
				continue;
			};
			for tag in tags.iter().filter_map(|t| t.as_str()) {
				if !registered.contains(tag) {
					unregistered.insert(tag.to_string());
				}
			}
		}
	}

	assert!(
		unregistered.is_empty(),
		"tags used by operations but not registered with a description in ApiDoc's tags(...):\n{}",
		unregistered.into_iter().collect::<Vec<_>>().join("\n"),
	);
}

/// Every operation on the administrative surface carries a safety-mode grade.
///
/// The macro already makes an ungraded entry a compile error, so this cannot
/// fail through the route tables. What it does catch is a grade that reaches the
/// document as something the server cannot read back — a mode spelled
/// differently, or an operation registered by some other route that never went
/// through the macro. A grade the enforcement layer cannot parse is an
/// unenforced handler.
///
/// spec: SAFE#grading-the-administrative-surface
#[test]
fn every_operation_declares_a_safety_mode() {
	let spec = build_spec();
	let paths = spec["paths"].as_object().expect("paths object");

	let modes = ["read-only", "write", "danger"];
	let mut ungraded: std::collections::BTreeSet<String> = Default::default();
	let mut graded = 0usize;

	for (path, item) in paths {
		for (method, op) in item.as_object().expect("path item object") {
			let Some(op) = op.as_object() else { continue };
			match op.get("x-canopy-safety-mode").and_then(|v| v.as_str()) {
				Some(mode) if modes.contains(&mode) => graded += 1,
				Some(mode) => {
					ungraded.insert(format!("{method} {path}: unknown mode {mode:?}"));
				}
				None => {
					ungraded.insert(format!("{method} {path}"));
				}
			}
		}
	}

	assert!(
		ungraded.is_empty(),
		"operations on the administrative surface carrying no readable safety mode:\n{}",
		ungraded.into_iter().collect::<Vec<_>>().join("\n"),
	);
	assert!(graded > 100, "expected the whole surface, found {graded}");
}
