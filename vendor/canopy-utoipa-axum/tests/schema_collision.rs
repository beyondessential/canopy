//! CANOPY FORK tests: the collision guard added in `router.rs` must panic when
//! two different types register the same OpenAPI component schema name, and
//! must stay quiet when the definitions are identical.

use axum::Json;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use serde::Deserialize;
use utoipa::ToSchema;

// Two distinct request bodies both forced to the component name `Dup` via
// `#[schema(as = ...)]` — exactly the situation that arises naturally when two
// modules define same-named `ToSchema` structs.
#[derive(Deserialize, ToSchema)]
#[schema(as = Dup)]
struct AlphaArgs {
    #[allow(dead_code)]
    alpha: i32,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = Dup)]
struct BetaArgs {
    #[allow(dead_code)]
    beta: String,
}

#[utoipa::path(post, path = "/alpha", request_body = AlphaArgs)]
async fn alpha(_: Json<AlphaArgs>) {}

#[utoipa::path(post, path = "/beta", request_body = BetaArgs)]
async fn beta(_: Json<BetaArgs>) {}

// A second handler reusing the *same* type produces the same schema under the
// same name — the harmless shared-shape case that must not panic.
#[utoipa::path(post, path = "/alpha2", request_body = AlphaArgs)]
async fn alpha2(_: Json<AlphaArgs>) {}

#[test]
#[should_panic(expected = "schema name collision")]
fn routes_on_one_router_panics_on_conflict() {
    let _: OpenApiRouter = OpenApiRouter::new()
        .routes(routes!(read_only: alpha))
        .routes(routes!(read_only: beta));
}

#[test]
#[should_panic(expected = "schema name collision")]
fn nest_panics_on_conflict() {
    let _: OpenApiRouter = OpenApiRouter::new()
        .nest("/a", OpenApiRouter::new().routes(routes!(read_only: alpha)))
        .nest("/b", OpenApiRouter::new().routes(routes!(read_only: beta)));
}

#[test]
#[should_panic(expected = "schema name collision")]
fn merge_panics_on_conflict() {
    let _: OpenApiRouter = OpenApiRouter::new()
        .merge(OpenApiRouter::new().routes(routes!(read_only: alpha)))
        .merge(OpenApiRouter::new().routes(routes!(read_only: beta)));
}

#[test]
fn identical_schema_under_same_name_is_fine() {
    // Same type registered twice: same name, same definition — no panic.
    let _: OpenApiRouter = OpenApiRouter::new().routes(routes!(read_only: alpha)).nest(
        "/again",
        OpenApiRouter::new().routes(routes!(read_only: alpha2)),
    );
}
