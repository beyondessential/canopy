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
		touched: Default::default(),
	};
	tracing::debug!(graded = safety.grades.len(), "safety-mode grades loaded");
	// Inside it, every audited handler is held to the actions it declares (see
	// the AUD spec), read from the same document.
	let audit = commons_servers::audit::AuditState {
		db: state.db.clone(),
		map: std::sync::Arc::new(commons_servers::audit::AuditMap::from_openapi(&api_spec)),
		actor: commons_servers::audit::ActorSource::Operator,
		undeclared: commons_servers::audit::Undeclared::Refuse,
		mount: "",
	};
	tracing::debug!(
		declared = audit.map.len(),
		audited = audit.map.audited(),
		"audit declarations loaded"
	);
	let api_router = api_router
		.layer(middleware::from_fn_with_state(
			audit,
			commons_servers::audit::enforce,
		))
		.layer(middleware::from_fn_with_state(
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
		// Beside the API, not within it: a redirect reaches no handler and so
		// carries no safety mode, and the layer above refuses what it cannot
		// grade. They carry the `/api` prefix themselves.
		.merge(fns::moved_paths())
		.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api_spec))
		.nest("/api/mcp", mcp)
		.fallback(spa::handler)
		.layer(middleware::from_fn(
			commons_servers::tailnet_guard::reject_tagged_devices,
		));

	// Wire the backup-credential clients (STS + kube Secret store) so the nested
	// public API can issue backup credentials and serve the repo target/password,
	// not just the DB-only endpoints.
	let public_state = public_server::state::AppState::for_nested_mount(
		state.db.clone(),
		state.tailnet_directory.clone(),
		state.sts.clone(),
		state.kube.clone(),
		state.artifacts.clone(),
	)?;
	let (public_router, _) = public_server::audited_under(
		public_server::routes(),
		public_state.db.clone(),
		"/public",
		commons_servers::audit::Undeclared::Refuse,
	);

	Ok(Router::new()
		.nest("/public", public_router.with_state(public_state))
		.merge(non_public)
		.with_state(state))
}
