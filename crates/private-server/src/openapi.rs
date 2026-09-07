use utoipa::{
	Modify, OpenApi,
	openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
};

/// Base OpenAPI document. Concrete paths and schemas are pulled in by
/// `utoipa-axum`'s `OpenApiRouter` as routes are registered.
#[derive(OpenApi)]
#[openapi(
	info(
		title = "canopy private-server",
		version = "",
		description = "Admin/operator API for the canopy fleet manager. Requests are gated behind Tailscale auth; admin-only endpoints additionally check the caller is on the admin list.\n\nRequest bodies may be sent compressed, and this is recommended for large payloads: the server transparently decodes `Content-Encoding: gzip`, `br`, `deflate`, or `zstd`. The accepted encodings are also listed structurally in the `x-request-compression` extension.",
		contact(name = "BES Developers", email = "contact@bes.au"),
		license(name = "GPL-3.0-or-later"),
	),
	modifiers(&SecuritySchemes, &RequestCompression),
	tags(
		(name = "admins", description = "Admin email allow-list management."),
		(name = "backups", description = "Group backup-repo onboarding, scheduling, and stats."),
		(name = "bestool", description = "Bestool SQL snippet library."),
		(name = "certificates", description = "TLS certificates and public names: what each server holds, the pause and profile that govern them, and revocation."),
		(name = "commons", description = "Shared configuration and identity helpers."),
		(name = "devices", description = "Device registry, trust, and key management."),
		(name = "domains", description = "Managed DNS zones and the domains each group controls."),
		(name = "healthchecks", description = "Healthcheck catalog: severities, conditional rules, and sample data."),
		(name = "incidents", description = "Operational incidents (groups of issues against a server)."),
		(name = "issues", description = "Per-server issues raised from device events."),
		(name = "machines", description = "The hosts applications run on: their group, location, and monitoring."),
		(name = "maintenance", description = "Windows declaring that a server or a group is being worked on."),
		(name = "mcp_tokens", description = "Bearer tokens for the public MCP mount."),
		(name = "upgrade_plans", description = "Where each group is going: the version it intends to move to, and when."),
		(name = "migration_tests", description = "Where each server stands against the version it would take next."),
		(name = "reporting_schemas", description = "Which of a group's versions have a reporting schema built for them."),
		(name = "restore_replicas", description = "Managed restore replicas: capabilities, worklist, and health."),
		(name = "self_alerts", description = "Canopy's alerts about its own operation."),
		(name = "server_groups", description = "Application group management and group-level configuration."),
		(name = "applications", description = "Application inventory, hierarchy, and metadata."),
		(name = "silenced_refs", description = "Silencing of issues, incidents, and healthchecks."),
		(name = "sql", description = "Read-only SQL playground."),
		(name = "statuses", description = "Live server status and version-distance summaries."),
		(name = "versions", description = "Canopy release versions and downloadable artifacts."),
	),
)]
pub struct ApiDoc;

/// Advertises the request-body Content-Encodings the server accepts as the
/// top-level `x-request-compression` vendor extension. Sourced from
/// [`commons_servers::request_compression_extension`] so it tracks the actual
/// decompression layer.
struct RequestCompression;

impl Modify for RequestCompression {
	fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
		openapi.extensions = Some(
			utoipa::openapi::extensions::ExtensionsBuilder::new()
				.add(
					"x-request-compression",
					commons_servers::request_compression_extension(),
				)
				.build(),
		);
	}
}

struct SecuritySchemes;

impl Modify for SecuritySchemes {
	fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
		let components = openapi.components.get_or_insert_with(Default::default);
		components.add_security_scheme(
			"tailscale-user",
			SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
				"Tailscale-User-Login",
				"Identity header injected by the Tailscale sidecar. Required for any caller-identifying endpoint.",
			))),
		);
		components.add_security_scheme(
			"tailscale-admin",
			SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
				"Tailscale-User-Login",
				"Identity header injected by the Tailscale sidecar. The login must additionally be an administrator: either on the admins allow-list, or granted admin by the tailnet policy.",
			))),
		);
	}
}
