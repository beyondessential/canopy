//! CANOPY FORK tests: the grade prefix on a `routes!` entry must land in the
//! generated OpenAPI document as an operation extension, since that extension is
//! the single place a handler's safety mode is written (see the SAFE spec).

use canopy_utoipa_axum::{router::OpenApiRouter, routes, SAFETY_MODE_EXTENSION};

#[utoipa::path(post, path = "/list")]
async fn list() {}

#[utoipa::path(post, path = "/add")]
async fn add() {}

#[utoipa::path(post, path = "/delete")]
async fn delete() {}

/// The mode recorded against an operation, read back out of the built document.
fn mode_of(api: &utoipa::openapi::OpenApi, path: &str) -> String {
    let item = api.paths.paths.get(path).expect("path is registered");
    let operation = item.post.as_ref().expect("path has a post operation");
    operation
        .extensions
        .as_ref()
        .expect("operation carries extensions")
        .get(SAFETY_MODE_EXTENSION)
        .expect("operation carries the safety-mode extension")
        .as_str()
        .expect("the safety mode is a string")
        .to_owned()
}

#[test]
fn each_grade_lands_on_its_operation() {
    let api = OpenApiRouter::<()>::new()
        .routes(routes!(read_only: list))
        .routes(routes!(write: add))
        .routes(routes!(danger: delete))
        .into_openapi();

    assert_eq!(mode_of(&api, "/list"), "read-only");
    assert_eq!(mode_of(&api, "/add"), "write");
    assert_eq!(mode_of(&api, "/delete"), "danger");
}

#[test]
fn the_extension_key_is_an_openapi_extension() {
    // utoipa prefixes a key with `x-` unless it already carries one. The server
    // and the client build both look the key up verbatim, so a silent rename
    // would leave every handler reading as ungraded.
    assert!(SAFETY_MODE_EXTENSION.starts_with("x-"));
}
