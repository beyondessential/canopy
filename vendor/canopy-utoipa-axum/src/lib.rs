#![cfg_attr(doc_cfg, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]

//! Utoipa axum brings `utoipa` and `axum` closer together by the way of providing an ergonomic API that is extending on
//! the `axum` API. It gives a natural way to register handlers known to `axum` and also simultaneously generates OpenAPI
//! specification from the handlers.
//!
//! ## Crate features
//!
//! - **`debug`**: Implement debug traits for types.
//!
//! ## Install
//!
//! Add dependency declaration to `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! utoipa-axum = "0.2"
//! ```
//!
//! ## Examples
//!
//! _**Use [`OpenApiRouter`][router] to collect handlers with _`#[utoipa::path]`_ macro to compose service and form OpenAPI spec.**_
//!
//! ```rust
//! # use axum::Json;
//! # use utoipa::openapi::OpenApi;
//! # use canopy_utoipa_axum::{routes, PathItemExt, router::OpenApiRouter};
//!  #[derive(utoipa::ToSchema, serde::Serialize)]
//!  struct User {
//!      id: i32,
//!  }
//!
//!  #[utoipa::path(get, path = "/user", responses((status = OK, body = User)))]
//!  async fn get_user() -> Json<User> {
//!     Json(User { id: 1 })
//!  }
//!  
//!  let (router, api): (axum::Router, OpenApi) = OpenApiRouter::new()
//!      .routes(routes!(read_only: get_user))
//!      .split_for_parts();
//! ```
//!
//! [router]: router/struct.OpenApiRouter.html

pub mod router;

use axum::routing::MethodFilter;
use utoipa::openapi::HttpMethod;

/// Extends [`utoipa::openapi::path::PathItem`] by providing conversion methods to convert this
/// path item type to a [`axum::routing::MethodFilter`].
pub trait PathItemExt {
    /// Convert this path item type to a [`axum::routing::MethodFilter`].
    ///
    /// Method filter is used with handler registration on [`axum::routing::MethodRouter`].
    fn to_method_filter(&self) -> MethodFilter;
}

impl PathItemExt for HttpMethod {
    fn to_method_filter(&self) -> MethodFilter {
        match self {
            HttpMethod::Get => MethodFilter::GET,
            HttpMethod::Put => MethodFilter::PUT,
            HttpMethod::Post => MethodFilter::POST,
            HttpMethod::Head => MethodFilter::HEAD,
            HttpMethod::Patch => MethodFilter::PATCH,
            HttpMethod::Trace => MethodFilter::TRACE,
            HttpMethod::Delete => MethodFilter::DELETE,
            HttpMethod::Options => MethodFilter::OPTIONS,
        }
    }
}

// CANOPY FORK: safety-mode grading (see the SAFE spec).
//
// Every handler on the private server's administrative surface declares the
// safety mode it requires, as a prefix on its `routes!` entry:
// `routes!(danger: delete)`. The grade is recorded as an OpenAPI operation
// extension, which is the single place it is written: the server reads it back
// to enforce the grade per request, and the client build reads it to present a
// control according to the mode it needs.

/// The OpenAPI operation-extension key carrying a handler's safety-mode grade.
/// Its value is the mode's wire form: `read-only`, `write`, or `danger`.
pub const SAFETY_MODE_EXTENSION: &str = "x-canopy-safety-mode";

/// Record a handler's safety-mode grade on its OpenAPI operation. Called by the
/// [`routes`] macro; not meant to be called directly.
#[doc(hidden)]
pub fn __set_safety_mode(operation: &mut utoipa::openapi::path::Operation, mode: &str) {
    use utoipa::openapi::extensions::Extensions;
    let incoming = Extensions::from_iter([(SAFETY_MODE_EXTENSION, mode)]);
    operation
        .extensions
        .get_or_insert_with(Extensions::default)
        .merge(incoming);
}

/// re-export paste so users do not need to add the dependency.
#[doc(hidden)]
pub use paste::paste;

/// Collect axum handlers annotated with [`utoipa::path`] to [`router::UtoipaMethodRouter`].
///
/// [`routes`] macro will return [`router::UtoipaMethodRouter`] which contains an
/// [`axum::routing::MethodRouter`] and currently registered paths. The output of this macro is
/// meant to be used together with [`router::OpenApiRouter`] which combines the paths and axum
/// routers to a single entity.
///
/// Only handlers collected with [`routes`] macro will get registered to the OpenApi.
///
/// # Panics
///
/// Routes registered via [`routes`] macro or via `axum::routing::*` operations are bound to same
/// rules where only one one HTTP method can can be registered once per call. This means that the
/// following will produce runtime panic from axum code.
///
/// ```rust,no_run
/// # use canopy_utoipa_axum::{routes, router::UtoipaMethodRouter};
/// # use utoipa::path;
///  #[utoipa::path(get, path = "/search")]
///  async fn search_user() {}
///
///  #[utoipa::path(get, path = "")]
///  async fn get_user() {}
///
///  let _: UtoipaMethodRouter = routes!(read_only: get_user);
/// ```
/// Since the _`axum`_ does not support method filter for `CONNECT` requests, using this macro with
/// handler having request method type `CONNECT` `#[utoipa::path(connect, path = "")]` will panic at
/// runtime.
///
/// # Examples
///
/// _**Create new `OpenApiRouter` with `get_user` and `post_user` paths.**_
/// ```rust
/// # use canopy_utoipa_axum::{routes, router::{OpenApiRouter, UtoipaMethodRouter}};
/// # use utoipa::path;
///  #[utoipa::path(get, path = "")]
///  async fn get_user() {}
///
///  #[utoipa::path(post, path = "")]
///  async fn post_user() {}
///
///  let _: OpenApiRouter = OpenApiRouter::new()
///      .routes(routes!(read_only: get_user))
///      .routes(routes!(write: post_user));
/// ```
#[macro_export]
macro_rules! routes {
    // CANOPY FORK: graded entries. The grade prefix names the safety mode the
    // handler requires (see the SAFE spec); it is recorded as an OpenAPI
    // operation extension and read back by the server to enforce it.
    ( read_only: $handler:path $(,)? ) => { $crate::routes!( @graded "read-only" : $handler ) };
    ( write: $handler:path $(,)? ) => { $crate::routes!( @graded "write" : $handler ) };
    ( danger: $handler:path $(,)? ) => { $crate::routes!( @graded "danger" : $handler ) };
    // The device-facing public API is a different surface: its callers are
    // machines presenting a certificate, not operators holding a session, so
    // safety modes do not govern it. Saying so is explicit and greppable rather
    // than being the shape an entry falls into by omission.
    // Upstream's multi-handler form survives here, because one path serving two
    // methods is registered in a single call. The graded arms stay one handler
    // each: a grade belongs to an operation, not to a path.
    ( public: $handler:path $(, $tail:path)* $(,)? ) => {
        {
            use $crate::PathItemExt;
            let mut paths = utoipa::openapi::path::Paths::new();
            let mut schemas = Vec::<(String, utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>)>::new();
            let (path, item, types) = $crate::routes!(@resolve_types $handler : schemas);
            #[allow(unused_mut)]
            let mut method_router = types.iter().by_ref().fold(axum::routing::MethodRouter::new(), |router, path_type| {
                router.on(path_type.to_method_filter(), $handler)
            });
            paths.add_path_operation(&path, types, item);
            $( method_router = $crate::routes!( schemas: method_router: paths: $tail ); )*
            (schemas, paths, method_router)
        }
    };
    ( $schemas:tt: $router:ident: $paths:ident: $handler:path $(, $tail:tt)* ) => {
        {
            let (path, item, types) = $crate::routes!(@resolve_types $handler : $schemas);
            let router = types.iter().by_ref().fold($router, |router, path_type| {
                router.on(path_type.to_method_filter(), $handler)
            });
            $paths.add_path_operation(&path, types, item);
            router
        }
    };
    ( @graded $mode:literal : $handler:path ) => {
        {
            use $crate::PathItemExt;
            let mut paths = utoipa::openapi::path::Paths::new();
            let mut schemas = Vec::<(String, utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>)>::new();
            let (path, mut item, types) = $crate::routes!(@resolve_types $handler : schemas);
            $crate::__set_safety_mode(&mut item, $mode);
            let method_router = types.iter().by_ref().fold(axum::routing::MethodRouter::new(), |router, path_type| {
                router.on(path_type.to_method_filter(), $handler)
            });
            paths.add_path_operation(&path, types, item);
            (schemas, paths, method_router)
        }
    };
    // CANOPY FORK: an entry without a grade is a compile error, so no handler
    // can reach the administrative surface ungraded. Upstream's bare and
    // multi-handler forms are gone with it: every canopy route table is one
    // handler per entry, and one grade per handler is the point.
    ( $handler:path $(, $tail:path)* $(,)? ) => {
        compile_error!(
            "this handler declares no safety mode. Every handler on the administrative \
             surface says which mode it requires: routes!(read_only: handler), \
             routes!(write: handler), or routes!(danger: handler). See the SAFE spec for \
             which one a handler takes."
        )
    };
    ( @resolve_types $handler:path : $schemas:tt ) => {
        {
            $crate::paste! {
                let path = $crate::routes!( @path [path()] of $handler );
                let mut operation = $crate::routes!( @path [operation()] of $handler );
                let types = $crate::routes!( @path [methods()] of $handler );
                let tags = $crate::routes!( @path [tags()] of $handler );
                $crate::routes!( @path [schemas(&mut $schemas)] of $handler );
                if !tags.is_empty() {
                    let operation_tags = operation.tags.get_or_insert(Vec::new());
                    operation_tags.extend(tags.iter().map(ToString::to_string));
                }
                (path, operation, types)
            }
        }
    };
    ( @path $op:tt of $part:ident $( :: $tt:tt )* ) => {
        $crate::routes!( $op : [ $part $( $tt )*] )
    };
    ( $op:tt : [ $first:tt $( $rest:tt )* ] $( $rev:tt )* ) => {
        $crate::routes!( $op : [ $( $rest )* ] $first $( $rev)* )
    };
    ( $op:tt : [] $first:tt $( $rest:tt )* ) => {
        $crate::routes!( @inverse $op : $first $( $rest )* )
    };
    ( @inverse $op:tt : $tt:tt $( $rest:tt )* ) => {
        $crate::routes!( @rev $op : $tt [$($rest)*] )
    };
    ( @rev $op:tt : $tt:tt [ $first:tt $( $rest:tt)* ] $( $reversed:tt )* ) => {
        $crate::routes!( @rev $op : $tt [ $( $rest )* ] $first $( $reversed )* )
    };
    ( @rev [$op:ident $( $args:tt )* ] : $handler:tt [] $($tt:tt)* ) => {
        {
            #[allow(unused_imports)]
            use utoipa::{Path, __dev::{Tags, SchemaReferences}};
            $crate::paste! {
                $( $tt :: )* [<__path_ $handler>]::$op $( $args )*
            }
        }
    };
    ( ) => {};
}

// CANOPY FORK: upstream's inline `#[cfg(test)] mod tests` (insta snapshot
// tests of the routes! macro) was removed to avoid vendoring its heavy
// dev-dependencies. The fork's own behaviour is covered by
// `tests/schema_collision.rs`.
