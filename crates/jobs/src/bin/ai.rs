//! Canopy's AI worker: the one process that holds the model provider's
//! credentials. It looks at each new vulnerability the scan finds, deciding the
//! plain cases and suggesting an outcome for the rest (see
//! `jobs::vulnerabilities::ai`).
//!
//! It is fed through the database: it claims items with no decision and no
//! suggestion, so it needs nothing from the monitor beyond the rows the scan
//! writes. With no model configured it does nothing.
//!
//! Configured by `OPENROUTER_API_KEY` and `OPENROUTER_MODEL`, and optionally
//! `OPENROUTER_BASE_URL`.

use std::time::Duration;

use clap::Parser;
use jobs::vulnerabilities::ai::{Config, Model, pass};
use lloggs::{LoggingArgs, PreArgs};
use tokio::time::sleep;
use tracing::{error, info};

const TICK: Duration = Duration::from_secs(60);
const BATCH: i64 = 10;

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
	let mut _guard = PreArgs::parse().setup()?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(args.logging.setup(|v| match v {
			0 => "info",
			1 => "debug",
			_ => "trace",
		})?);
	}

	let Some(config) = Config::from_env() else {
		info!("no model configured (OPENROUTER_API_KEY and OPENROUTER_MODEL): nothing to do");
		futures::future::pending::<()>().await;
		return Ok(());
	};
	let model = Model::new(config);
	let pool = database::init();
	info!("looking at new vulnerabilities with {}", model.name());

	loop {
		match pool.get().await {
			Ok(mut conn) => loop {
				match pass(&mut conn, &model, BATCH).await {
					Ok(0) => break,
					Ok(n) => info!("looked at {n} vulnerabilities"),
					Err(e) => {
						error!("AI pass failed: {e}");
						break;
					}
				}
			},
			Err(e) => error!("failed to get database connection: {e}"),
		}
		sleep(TICK).await;
	}
}
