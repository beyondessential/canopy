use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};

use axum::Router;
use axum_client_ip::ClientIpSource;
use canopy_utoipa_axum::router::OpenApiRouter;
use clap::Parser;
use commons_servers::{router, serve};
use lloggs::{LoggingArgs, PreArgs};
use public_server::state::AppState;
use utoipa::OpenApi as _;
use utoipa_swagger_ui::SwaggerUi;

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	#[arg(long, short, default_value = "8080", env = "PORT")]
	port: u16,

	#[arg(long, env = "BIND_ADDRESS", conflicts_with = "port")]
	bind: Option<SocketAddr>,

	#[arg(long, env = "CLIENT_IP_SOURCE", default_value = "ConnectInfo")]
	client_ip_source: ClientIpSource,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
	let mut _guard = PreArgs::parse_with_env("META_LOG").setup()?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(args.logging.setup(|v| match v {
			0 => "info",
			1 => "debug",
			_ => "trace",
		})?);
	}

	let addr = args
		.bind
		.unwrap_or_else(|| SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, args.port, 0, 0)));

	let state = AppState::init().await?;
	let (api_router, api_spec) = public_server::audited(
		OpenApiRouter::with_openapi(public_server::openapi::ApiDoc::openapi())
			.merge(public_server::routes()),
		state.db.clone(),
	);
	let app: Router<()> = api_router
		.with_state(state.clone())
		.merge(public_server::calendar::routes(state.clone()))
		.merge(public_server::mcp::routes(state))
		.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api_spec));

	serve(router(app, args.client_ip_source), addr).await?;
	Ok(())
}
