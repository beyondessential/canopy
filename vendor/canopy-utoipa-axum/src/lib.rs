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
//!      .routes(routes!(read_only, silent: get_user))
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

/// The OpenAPI operation-extension key carrying why a danger handler is danger.
/// Its value is an array of reason names, each one of [`DANGER_REASONS`].
pub const DANGER_REASONS_EXTENSION: &str = "x-canopy-danger-reasons";

/// Every reason a handler can be danger for, in the order the SAFE spec lists
/// them. The wire form of each is what a `routes!(danger(reason, ...), silent: handler)`
/// entry names.
pub const DANGER_REASONS: [&str; 5] = [
    "irreversible",
    "fleet",
    "unprotects",
    "issues",
    "invalidates",
];

/// Fail compilation with `error` unless `name` is one of [`DANGER_REASONS`].
/// Called by the [`routes`] macro, from a constant, so an unknown reason is a
/// build error rather than a runtime one; not meant to be called directly.
#[doc(hidden)]
pub const fn __assert_danger_reason(name: &str, error: &str) {
    let mut i = 0;
    while i < DANGER_REASONS.len() {
        if const_str_eq(DANGER_REASONS[i], name) {
            return;
        }
        i += 1;
    }
    panic!("{}", error);
}

const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Record why a handler is danger on its OpenAPI operation. Called by the
/// [`routes`] macro; not meant to be called directly.
#[doc(hidden)]
pub fn __set_danger_reasons(operation: &mut utoipa::openapi::path::Operation, reasons: &[&str]) {
    use utoipa::openapi::extensions::Extensions;
    let incoming = Extensions::from_iter([(DANGER_REASONS_EXTENSION, reasons.to_vec())]);
    operation
        .extensions
        .get_or_insert_with(Extensions::default)
        .merge(incoming);
}

// CANOPY FORK: audit declarations (see the AUD spec).
//
// Every handler declares, beside its grade, whether it is audited or silent:
// `routes!(write, audit(MaintenanceDeclared): declare)` or
// `routes!(read_only, silent: list)`. A declaration is recorded as an OpenAPI
// operation extension, the single place it is written: the server reads it back
// to hold an audited handler to the actions it declares. An audited handler
// must take the audit extractor, which is checked at compile time.

/// The OpenAPI operation-extension key carrying the actions a handler declares.
/// Its value is an array of action names, empty for a handler that is silent.
pub const AUDIT_EXTENSION: &str = "x-canopy-audit";

/// The OpenAPI operation-extension key marking a handler whose declared actions
/// are ones it may record, rather than ones it must: it records none on a
/// request that takes no action, as a status report does unless it creates a
/// record. Present and `true` for such a handler.
pub const AUDIT_OPTIONAL_EXTENSION: &str = "x-canopy-audit-optional";

/// Record a handler's audit declaration on its OpenAPI operation. Called by the
/// [`routes`] macro; not meant to be called directly.
#[doc(hidden)]
pub fn __set_audit(
    operation: &mut utoipa::openapi::path::Operation,
    actions: &[&str],
    optional: bool,
) {
    use utoipa::openapi::extensions::Extensions;
    let mut incoming = Extensions::from_iter([(AUDIT_EXTENSION, actions.to_vec())]);
    if optional {
        incoming.merge(Extensions::from_iter([(AUDIT_OPTIONAL_EXTENSION, true)]));
    }
    operation
        .extensions
        .get_or_insert_with(Extensions::default)
        .merge(incoming);
}

/// Implemented by the audit extractor, so a handler can be checked for taking
/// it without this crate knowing what it is.
pub trait AuditExtractor {}

/// Where in a handler's arguments the audit extractor sits, as the arguments
/// before it.
#[doc(hidden)]
pub struct Before<T>(core::marker::PhantomData<T>);

/// A handler that takes the audit extractor as one of its arguments. Implemented
/// across argument counts and positions the way axum implements `Handler`, with
/// the arguments before the extractor in the trait's parameter so that the
/// implementations do not overlap.
#[doc(hidden)]
pub trait TakesAudit<Marker> {}

macro_rules! takes_audit_at {
    ( [ $( $before:ident )* ] $at:ident [ $( $after:ident )* ] ) => {
        #[allow(non_snake_case, unused_parens)]
        impl<F, Fut, $( $before, )* $at, $( $after, )*>
            TakesAudit<(Before<( $( $before, )* )>, $( $before, )* $at, $( $after, )*)> for F
        where
            F: Fn($( $before, )* $at, $( $after, )*) -> Fut,
            $at: AuditExtractor,
        {
        }
    };
}

macro_rules! takes_audit {
    ( [ $( $before:ident )* ] [] ) => {};
    ( [ $( $before:ident )* ] [ $at:ident $( $after:ident )* ] ) => {
        takes_audit_at!( [ $( $before )* ] $at [ $( $after )* ] );
        takes_audit!( [ $( $before )* $at ] [ $( $after )* ] );
    };
}

takes_audit!([][A1]);
takes_audit!([] [A1 A2]);
takes_audit!([] [A1 A2 A3]);
takes_audit!([] [A1 A2 A3 A4]);
takes_audit!([] [A1 A2 A3 A4 A5]);
takes_audit!([] [A1 A2 A3 A4 A5 A6]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7 A8]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7 A8 A9]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7 A8 A9 A10]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11]);
takes_audit!([] [A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12]);

/// Fail compilation unless the handler takes the audit extractor. Called by the
/// [`routes`] macro for an audited handler; not meant to be called directly.
///
/// A handler that takes it, wherever among its arguments:
///
/// ```
/// use canopy_utoipa_axum::{AuditExtractor, __assert_audited};
/// struct Audit;
/// impl AuditExtractor for Audit {}
/// async fn first(_: Audit, _: String) {}
/// async fn middle(_: u8, _: Audit, _: String) {}
/// async fn only(_: Audit) {}
/// __assert_audited(&first);
/// __assert_audited(&middle);
/// __assert_audited(&only);
/// ```
///
/// One that does not does not build:
///
/// ```compile_fail
/// use canopy_utoipa_axum::__assert_audited;
/// async fn does_not(_: u8, _: String) {}
/// __assert_audited(&does_not);
/// ```
#[doc(hidden)]
pub fn __assert_audited<F, M>(_: &F)
where
    F: TakesAudit<M>,
{
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
///  let _: UtoipaMethodRouter = routes!(read_only, silent: get_user);
/// ```
/// Since the _`axum`_ does not support method filter for `CONNECT` requests, using this macro with
/// handler having request method type `CONNECT` `#[utoipa::path(connect, path = "")]` will panic at
/// runtime.
///
/// A route that declares no safety mode and no audit declaration does not build:
///
/// ```compile_fail
/// # use canopy_utoipa_axum::routes;
/// # use utoipa::path;
///  #[utoipa::path(get, path = "")]
///  async fn get_user() {}
///
///  let _ = routes!(get_user);
/// ```
///
/// Nor does one that declares a mode and no audit declaration:
///
/// ```compile_fail
/// # use canopy_utoipa_axum::routes;
/// # use utoipa::path;
///  #[utoipa::path(get, path = "")]
///  async fn get_user() {}
///
///  let _ = routes!(read_only: get_user);
/// ```
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
///      .routes(routes!(read_only, silent: get_user))
///      .routes(routes!(write, silent: post_user));
/// ```
#[macro_export]
macro_rules! routes {
    // CANOPY FORK: graded entries. The grade prefix names the safety mode the
    // handler requires (see the SAFE spec); it is recorded as an OpenAPI
    // operation extension and read back by the server to enforce it. The audit
    // clause beside it says whether the handler is audited or silent (see the
    // AUD spec), and which actions an audited one takes.
    ( read_only, $clause:ident $( ( $( $action:ident ),+ $(,)? ) )? : $handler:path $(,)? ) => {
        $crate::routes!( @graded "read-only" [] $clause $( [ $( $crate::routes!( @action $action ) ),+ ] )? : $handler )
    };
    ( write, $clause:ident $( ( $( $action:ident ),+ $(,)? ) )? : $handler:path $(,)? ) => {
        $crate::routes!( @graded "write" [] $clause $( [ $( $crate::routes!( @action $action ) ),+ ] )? : $handler )
    };
    // A danger handler also says why, as one or more reasons from
    // `DANGER_REASONS`: `routes!(danger(fleet, invalidates), silent: revoke)`. The
    // operator is told these when a blocked control asks to raise.
    ( danger( $( $reason:ident ),+ $(,)? ), $clause:ident $( ( $( $action:ident ),+ $(,)? ) )? : $handler:path $(,)? ) => {
        $crate::routes!(
            @graded "danger" [ $( $crate::routes!( @reason $reason ) ),+ ]
            $clause $( [ $( $crate::routes!( @action $action ) ),+ ] )? : $handler
        )
    };
    ( danger : $handler:path $(,)? ) => {
        compile_error!(
            "a danger handler declares why it is danger: \
             routes!(danger(reason, ...), audit(...): handler), with each reason one of \
             canopy_utoipa_axum::DANGER_REASONS. See the SAFE spec."
        )
    };
    ( @reason $reason:ident ) => {
        {
            const _: () = $crate::__assert_danger_reason(
                stringify!($reason),
                concat!(
                    "`", stringify!($reason), "` is not a danger reason: each reason is one of \
                     canopy_utoipa_axum::DANGER_REASONS. See the SAFE spec."
                ),
            );
            stringify!($reason)
        }
    };
    // An action is named by its variant in the audit catalogue, which is also
    // the constant carrying its stable name, so a misspelt action does not
    // build.
    ( @action $action:ident ) => { ::database::audit::names::$action };
    // The device-facing public API is a different surface: its callers are
    // machines presenting a certificate, not operators holding a session, so
    // safety modes do not govern it. Saying so is explicit and greppable rather
    // than being the shape an entry falls into by omission. It still declares
    // whether it is audited. One handler to an entry, as the graded surface: a
    // path serving two methods is two entries, which axum merges.
    ( public, $clause:ident $( ( $( $action:ident ),+ $(,)? ) )? : $handler:path $(,)? ) => {
        $crate::routes!( @public $clause $( [ $( $crate::routes!( @action $action ) ),+ ] )? : $handler )
    };
    // How a clause reads: `silent` takes no action, `audit(...)` must record at
    // least one of the actions it names, and `audit_optional(...)` may.
    ( @optional silent ) => { false };
    ( @optional audit ) => { false };
    ( @optional audit_optional ) => { true };
    ( @optional $other:ident ) => {
        compile_error!(
            "an audit declaration is `silent`, `audit(Action, ...)` or `audit_optional(Action, ...)`. \
             See the AUD spec."
        )
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
    ( @public $audit:ident $( [ $( $action:expr ),* ] )? : $handler:path ) => {
        {
            use $crate::PathItemExt;
            let mut paths = utoipa::openapi::path::Paths::new();
            let mut schemas = Vec::<(String, utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>)>::new();
            let (path, mut item, types) = $crate::routes!(@resolve_types $handler : schemas);
            let actions: &[&str] = &[ $( $( $action ),* )? ];
            $crate::__set_audit(&mut item, actions, $crate::routes!( @optional $audit ));
            $( let _ = [ $( $action ),* ]; $crate::__assert_audited(&$handler); )?
            let method_router = types.iter().by_ref().fold(axum::routing::MethodRouter::new(), |router, path_type| {
                router.on(path_type.to_method_filter(), $handler)
            });
            paths.add_path_operation(&path, types, item);
            (schemas, paths, method_router)
        }
    };
    ( @graded $mode:literal [ $( $reason:expr ),* ] $audit:ident $( [ $( $action:expr ),* ] )? : $handler:path ) => {
        {
            use $crate::PathItemExt;
            let mut paths = utoipa::openapi::path::Paths::new();
            let mut schemas = Vec::<(String, utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>)>::new();
            let (path, mut item, types) = $crate::routes!(@resolve_types $handler : schemas);
            $crate::__set_safety_mode(&mut item, $mode);
            let reasons: &[&str] = &[ $( $reason ),* ];
            if !reasons.is_empty() {
                $crate::__set_danger_reasons(&mut item, reasons);
            }
            let actions: &[&str] = &[ $( $( $action ),* )? ];
            $crate::__set_audit(&mut item, actions, $crate::routes!( @optional $audit ));
            $( let _ = [ $( $action ),* ]; $crate::__assert_audited(&$handler); )?
            let method_router = types.iter().by_ref().fold(axum::routing::MethodRouter::new(), |router, path_type| {
                router.on(path_type.to_method_filter(), $handler)
            });
            paths.add_path_operation(&path, types, item);
            (schemas, paths, method_router)
        }
    };
    // CANOPY FORK: an entry without a grade and an audit declaration is a
    // compile error, so no handler can reach the administrative surface
    // ungraded or unaudited. Upstream's bare and multi-handler forms are gone
    // with it: every canopy route table is one handler per entry, and one
    // declaration per handler is the point.
    ( $handler:path $(, $tail:path)* $(,)? ) => {
        compile_error!(
            "this handler declares no safety mode and no audit declaration. Every handler on \
             the administrative surface says which mode it requires and whether it is \
             audited: routes!(read_only, silent: handler), \
             routes!(write, audit(Action): handler), or \
             routes!(danger(reason, ...), audit(Action): handler). See the SAFE and AUD specs."
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
