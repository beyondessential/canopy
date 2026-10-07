//! Dump the private-server's OpenAPI spec to stdout as pretty-printed JSON.
//!
//! Used by `just gen-openapi` to refresh `private-web/openapi.json`, which the
//! React frontend then turns into TypeScript types via `openapi-typescript`.
//! No database or network is required — the spec is fully derived from compile-
//! time annotations.

use canopy_utoipa_axum::router::OpenApiRouter;
use private_server::{fns, openapi::ApiDoc};
use serde_json::Value;
use utoipa::OpenApi;

fn main() {
	let (_router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
		.merge(fns::routes())
		.split_for_parts();
	// utoipa holds an operation's extensions in a `HashMap`, so written directly
	// an operation with more than one would come out in a different order on
	// every run. Sorted explicitly rather than left to `Value`'s maps, which keep
	// insertion order wherever serde_json's `preserve_order` is on.
	let value = sorted(serde_json::to_value(&openapi).expect("serialize spec"));
	let json = serde_json::to_string_pretty(&value).expect("serialize spec");
	println!("{json}");
}

fn sorted(value: Value) -> Value {
	match value {
		Value::Object(map) => {
			let mut entries: Vec<_> = map.into_iter().collect();
			entries.sort_by(|(a, _), (b, _)| a.cmp(b));
			Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
		}
		Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
		other => other,
	}
}
