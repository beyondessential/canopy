//! Dump the private-server's OpenAPI spec to stdout as pretty-printed JSON.
//!
//! Used by `just gen-openapi` to refresh `private-web/openapi.json`, which the
//! React frontend then turns into TypeScript types via `openapi-typescript`.
//! No database or network is required — the spec is fully derived from compile-
//! time annotations.

use canopy_utoipa_axum::router::OpenApiRouter;
use private_server::{fns, openapi::ApiDoc};
use utoipa::OpenApi;

fn main() {
	let (_router, openapi) = OpenApiRouter::with_openapi(ApiDoc::openapi())
		.merge(fns::routes())
		.split_for_parts();
	// Through a `Value`, whose maps are ordered, because utoipa holds an
	// operation's extensions in a `HashMap`: written directly, an operation with
	// more than one would come out in a different order on every run.
	let value = serde_json::to_value(&openapi).expect("serialize spec");
	let json = serde_json::to_string_pretty(&value).expect("serialize spec");
	println!("{json}");
}
