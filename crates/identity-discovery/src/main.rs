use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};

use clap::Parser;
use oidc::Issuer;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
struct Args {
	#[arg(long, short, default_value = "8080", env = "PORT")]
	port: u16,

	#[arg(long, env = "BIND_ADDRESS", conflicts_with = "port")]
	bind: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	tracing_subscriber::fmt()
		.with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
		.init();
	let args = Args::parse();
	let addr = args
		.bind
		.unwrap_or_else(|| SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, args.port, 0, 0)));

	let Some(issuer) = Issuer::from_env().await else {
		return Err(format!(
			"no issuer and keys configured: set {} and {}",
			oidc::ISSUER_ENV,
			oidc::KEYS_ENV
		)
		.into());
	};
	tracing::info!(issuer = %issuer.issuer, %addr, "serving identity provider discovery");

	let listener = tokio::net::TcpListener::bind(addr).await?;
	axum::serve(listener, identity_discovery::router(issuer).await).await?;
	Ok(())
}
