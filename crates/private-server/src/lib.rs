pub mod backup_probe;
pub mod fns;
pub mod mcp;
pub mod openapi;
pub mod run_pairing;
pub mod safety;
pub mod spa;
pub mod state;

pub fn routes(state: crate::state::AppState) -> commons_errors::Result<axum::routing::Router<()>> {
	use axum::middleware;
	use axum::routing::Router;
	use canopy_utoipa_axum::router::OpenApiRouter;
	use utoipa::OpenApi;
	use utoipa_swagger_ui::SwaggerUi;

	let (api_router, api_spec) = OpenApiRouter::with_openapi(openapi::ApiDoc::openapi())
		.merge(fns::routes())
		.split_for_parts();

	// Every graded handler is decided against the caller's session here (see the
	// SAFE spec). The grades come from the document the `routes!` entries just
	// produced, so there is one declaration per handler and no second copy to
	// drift. Applied to the API routes only: the SPA and Swagger are not the
	// administrative surface.
	let safety = crate::safety::SafetyState {
		app: state.clone(),
		grades: std::sync::Arc::new(crate::safety::GradeMap::from_openapi(&api_spec)),
	};
	tracing::debug!(graded = safety.grades.len(), "safety-mode grades loaded");
	let api_router = api_router.layer(middleware::from_fn_with_state(
		safety,
		crate::safety::enforce,
	));

	// `/public/...` accepts tagged-device callers via the dual-auth
	// device extractor. Everything else (admin API, Swagger, SPA) is
	// human-only — the tagged-device guard 403s those callers up front
	// rather than relying on downstream extractors' opportunistic checks.
	// Read-only MCP query interface for tailnet agents. Mounted inside the
	// non-public subtree so it inherits the tagged-device guard; gated on top
	// by "any tailnet user" (see `mcp::require_tailnet_user`).
	let mcp: Router<crate::state::AppState> = Router::new()
		.fallback_service(canopy_mcp::service(state.db_read.clone()))
		.layer(middleware::from_fn(mcp::require_tailnet_user));

	let non_public = Router::new()
		.merge(api_router)
		.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api_spec))
		.nest("/api/mcp", mcp)
		.fallback(spa::handler)
		.layer(middleware::from_fn(
			commons_servers::tailnet_guard::reject_tagged_devices,
		));

	Ok(Router::new()
		.nest(
			"/public",
			Router::from(public_server::routes().with_state(
				// Wire the backup-credential clients (STS + kube Secret store) so the
				// nested public API can issue backup credentials and serve the repo
				// target/password, not just the DB-only endpoints.
				public_server::state::AppState::for_nested_mount(
					state.db.clone(),
					state.tailnet_directory.clone(),
					state.sts.clone(),
					state.kube.clone(),
					state.artifacts.clone(),
				)?,
			)),
		)
		.merge(non_public)
		.with_state(state))
}
