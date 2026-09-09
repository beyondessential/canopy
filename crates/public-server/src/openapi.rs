use utoipa::{Modify, OpenApi, openapi::security::SecurityScheme};

/// Base OpenAPI document. Path entries are pulled in by `utoipa-axum`'s
/// `OpenApiRouter` as routes are registered. HTML, redirect, and binary-stream
/// endpoints (the UI side, the `timesync` Timesimp protocol, the artifact
/// download proxy) are deliberately excluded from the spec.
#[derive(OpenApi)]
#[openapi(
	info(
		title = "canopy public-server",
		version = "1.0.1",
		description = "Internet-facing API for the canopy fleet. Device-authenticated endpoints require an mTLS client certificate.\n\nRequest bodies may be sent compressed, and this is recommended for large payloads: the server transparently decodes `Content-Encoding: gzip`, `br`, `deflate`, or `zstd`. The accepted encodings are also listed structurally in the `x-request-compression` extension.",
		contact(name = "BES Developers", email = "contact@bes.au"),
		license(name = "GPL-3.0-or-later"),
	),
	modifiers(&SecuritySchemes, &RequestCompression),
	tags(
		(name = "artifacts", description = "Per-version artifact registration by releaser devices."),
		(name = "backup", description = "Device backup credential minting, target config, capability registration, and run reporting."),
		(name = "bestool", description = "Bestool SQL snippet read API."),
		(name = "certificates", description = "TLS certificates for a server's own names: requesting one for a signing request, and collecting it once Canopy has obtained it."),
		(name = "names", description = "Public names a server may act on: what it is entitled to, and the addresses to publish for them."),
		(name = "machines", description = "The boxes applications run on: a machine asking which box it is enrolled as, and what Canopy holds running on it."),
		(name = "restore", description = "Managed restore replicas: consumer capability registration, worklist, and read-only restore credentials."),
		(name = "applications", description = "Application registry — listing for the public, self-registration for server devices."),
		(name = "statuses", description = "Heartbeat / status submissions from server devices."),
		(name = "tags", description = "Key/value tags describing a server."),
		(name = "versions", description = "Canopy release versions and their downloadable artifacts."),
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
		// One mTLS scheme per role so individual endpoints can name what they
		// require; the actual transport (mTLS terminated at the edge proxy,
		// cert keyed against the device registry) is an implementation
		// detail callers don't need.
		let role_scheme = |role: &str| -> SecurityScheme {
			SecurityScheme::MutualTls {
				description: Some(format!(
					"mTLS client certificate for a device with the `{role}` role (or `admin`).",
				)),
				extensions: None,
			}
		};
		components.add_security_scheme("server-device", role_scheme("server"));
		components.add_security_scheme("releaser-device", role_scheme("releaser"));
		components.add_security_scheme("backup-restore-device", role_scheme("backup-restore"));
		components.add_security_scheme(
			"admin-device",
			SecurityScheme::MutualTls {
				description: Some(
					"mTLS client certificate for a device with the `admin` role.".to_string(),
				),
				extensions: None,
			},
		);
	}
}
