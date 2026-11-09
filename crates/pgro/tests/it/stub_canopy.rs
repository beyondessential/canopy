//! A stand-in for Canopy that the cluster tests point pgro at.
//!
//! pgro runs on the CI host and reaches Canopy at [`ADDR`] with no proxy, so
//! the stub lives in the test process. It answers the restore endpoints pgro
//! calls, hands out the MinIO credentials the fixtures use as if they were
//! Canopy's short-lived ones, serves whatever worklist a test sets, and records
//! every request so a test can read what pgro reported.
//!
//! The server runs on a thread and runtime of its own, because each
//! `#[tokio::test]` has a runtime that ends with the test and the stub has to
//! outlive all of a module's tests.

use std::{
	collections::{BTreeMap, BTreeSet},
	io::Read,
	sync::{Arc, Mutex, OnceLock},
};

use axum::{
	Router,
	body::to_bytes,
	extract::{Request, State},
	http::{Method, StatusCode, header},
	response::{IntoResponse, Response},
};
use bes_canopy_api::{
	Redacted,
	schema::{CredentialProcessOutput, RestoreCredentials, WorklistEntry},
};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use uuid::Uuid;

/// Where pgro is told Canopy is, on the host both run on.
pub const ADDR: &str = "127.0.0.1:8098";

/// The repository password every fixture repository is created with.
pub const REPO_PASSWORD: &str = "test-repo-password";

/// The root credentials of the MinIO the fixtures run.
const MINIO_ACCESS_KEY: &str = "minioadmin";
const MINIO_SECRET_KEY: &str = "minioadmin";

/// One request pgro made, with its path under Canopy's device API and its body:
/// JSON where the request carried JSON, the text otherwise.
#[derive(Clone, Debug)]
pub struct Recorded {
	pub path: String,
	pub body: Value,
}

#[derive(Default)]
struct Inner {
	worklist: BTreeMap<String, Value>,
	requests: Vec<Recorded>,
	denied_groups: BTreeSet<String>,
}

#[derive(Clone, Default)]
pub struct StubCanopy {
	inner: Arc<Mutex<Inner>>,
}

/// The stub, started on first use and shared by every test in the process.
pub fn stub() -> &'static StubCanopy {
	static STUB: OnceLock<StubCanopy> = OnceLock::new();
	STUB.get_or_init(|| {
		let stub = StubCanopy::default();
		let app = Router::new().fallback(handle).with_state(stub.clone());
		let (ready, wait) = std::sync::mpsc::channel();
		std::thread::spawn(move || {
			tokio::runtime::Builder::new_current_thread()
				.enable_all()
				.build()
				.expect("stub canopy runtime")
				.block_on(async move {
					let listener = tokio::net::TcpListener::bind(ADDR)
						.await
						.unwrap_or_else(|err| panic!("stub canopy cannot listen on {ADDR}: {err}"));
					ready.send(()).expect("stub canopy ready signal");
					axum::serve(listener, app)
						.await
						.expect("stub canopy server");
				});
		});
		wait.recv().expect("stub canopy did not start");
		stub
	})
}

impl StubCanopy {
	/// Offer `entry` on the worklist under `name`, replacing any entry of that name.
	/// An entry stays until it is removed, so a test's replica is not torn down by
	/// a pass that happens while another test is running.
	pub fn put_entry(&self, name: &str, entry: &WorklistEntry) {
		let entry = serde_json::to_value(entry).expect("worklist entry serialises");
		self.inner
			.lock()
			.unwrap()
			.worklist
			.insert(name.to_owned(), entry);
	}

	pub fn remove_entry(&self, name: &str) {
		self.inner.lock().unwrap().worklist.remove(name);
	}

	/// Refuse credentials for `group`, as Canopy does where no declaration covers it.
	pub fn deny_credentials(&self, group: Uuid) {
		self.inner
			.lock()
			.unwrap()
			.denied_groups
			.insert(group.to_string());
	}

	/// The requests pgro has made to `path` (under the device API, without the
	/// `/public` mount), oldest first.
	pub fn requests_to(&self, path: &str) -> Vec<Recorded> {
		self.inner
			.lock()
			.unwrap()
			.requests
			.iter()
			.filter(|request| request.path == path)
			.cloned()
			.collect()
	}
}

/// What Canopy hands a restore: the fixtures' MinIO root credentials, shaped as
/// the short-lived read-only ones it issues, and the repository password.
fn credentials() -> RestoreCredentials {
	RestoreCredentials::builder()
		.credentials(
			CredentialProcessOutput::builder()
				.version(1)
				.access_key_id(MINIO_ACCESS_KEY.to_owned())
				.secret_access_key(Redacted(MINIO_SECRET_KEY.to_owned()))
				.session_token(Redacted(String::new()))
				.expiration("2099-01-01T00:00:00Z".parse().unwrap())
				.build(),
		)
		.repo_password(Redacted(REPO_PASSWORD.to_owned()))
		.build()
}

async fn handle(State(stub): State<StubCanopy>, request: Request) -> Response {
	let (parts, body) = request.into_parts();
	let raw = to_bytes(body, usize::MAX).await.expect("request body");
	let path = parts
		.uri
		.path()
		.strip_prefix("/public")
		.unwrap_or(parts.uri.path())
		.to_owned();

	let gzipped = parts
		.headers
		.get(header::CONTENT_ENCODING)
		.is_some_and(|encoding| encoding == "gzip");
	let mut text = Vec::new();
	if gzipped {
		GzDecoder::new(&raw[..])
			.read_to_end(&mut text)
			.expect("gzipped request body");
	} else {
		text.extend_from_slice(&raw);
	}
	let text = String::from_utf8_lossy(&text).into_owned();
	let body = serde_json::from_str(&text).unwrap_or(Value::String(text));

	let denied = body
		.get("group")
		.and_then(Value::as_str)
		.is_some_and(|group| stub.inner.lock().unwrap().denied_groups.contains(group));
	stub.inner.lock().unwrap().requests.push(Recorded {
		path: path.clone(),
		body,
	});

	match (&parts.method, path.as_str()) {
		(&Method::GET, "/restore-worklist") => {
			let entries: Vec<Value> = stub
				.inner
				.lock()
				.unwrap()
				.worklist
				.values()
				.cloned()
				.collect();
			axum::Json(entries).into_response()
		}
		(&Method::POST, "/restore-credentials") if denied => StatusCode::FORBIDDEN.into_response(),
		(&Method::POST, "/restore-credentials") => axum::Json(credentials()).into_response(),
		(&Method::POST, path) if path.starts_with("/artifacts/") => {
			axum::Json(json!({ "id": Uuid::new_v4() })).into_response()
		}
		(&Method::POST, "/restore-capabilities" | "/restore-verification" | "/backup-progress") => {
			StatusCode::OK.into_response()
		}
		_ => StatusCode::NOT_FOUND.into_response(),
	}
}
