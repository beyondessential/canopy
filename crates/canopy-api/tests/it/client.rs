//! The call plumbing every generated method routes through.

use bes_canopy_api::{CanopyClient, Error};

use crate::support::Recorder;

#[tokio::test]
async fn get_sends_a_path_only_uri_and_no_body() {
	let recorder = Recorder::json(200, "{}");
	let client = CanopyClient::new(recorder);

	client
		.status_check_severities("a-server")
		.await
		.expect("a 200 with an empty map parses");

	let request = client.transport().last();
	assert_eq!(request.method(), http::Method::GET);
	assert_eq!(request.uri(), "/status/a-server/check-severities");
	assert!(
		request.uri().scheme().is_none() && request.uri().authority().is_none(),
		"resolving the path against a base URL is the transport's job"
	);
	assert!(request.body().is_empty());
}

#[tokio::test]
async fn a_path_parameter_is_substituted() {
	let recorder = Recorder::json(200, "[]");
	let client = CanopyClient::new(recorder);

	client
		.versions_artifacts("2.11.0")
		.await
		.expect("a 200 with an empty list parses");

	assert_eq!(
		client.transport().last().uri(),
		"/versions/2.11.0/artifacts"
	);
}

#[tokio::test]
async fn an_unsuccessful_status_carries_the_status_and_body() {
	let recorder = Recorder::json(412, "device is dormant");
	let client = CanopyClient::new(recorder);

	let err = client
		.status_check_severities("a-server")
		.await
		.expect_err("a 412 is not a success");

	let http = err.http().expect("a non-2xx surfaces as an HTTP error");
	assert_eq!(http.status, http::StatusCode::PRECONDITION_FAILED);
	assert_eq!(http.path, "/status/a-server/check-severities");
	assert_eq!(http.body_text(), "device is dormant");
	assert_eq!(err.status(), Some(http::StatusCode::PRECONDITION_FAILED));
}

#[tokio::test]
async fn a_body_that_is_not_the_declared_json_is_a_decode_error_not_an_http_error() {
	let recorder = Recorder::json(200, "not json at all");
	let client = CanopyClient::new(recorder);

	let err = client
		.status_check_severities("a-server")
		.await
		.expect_err("a 200 carrying junk does not parse");

	assert!(matches!(err, Error::Decode { .. }));
	assert!(err.http().is_none());
}

#[tokio::test]
async fn a_transport_failure_is_distinct_from_a_failing_response() {
	use bes_canopy_api::{CanopyRequest, CanopyResponse, CanopyTransport, Result, async_trait};

	struct Unreachable;

	#[async_trait]
	impl CanopyTransport for Unreachable {
		async fn call(&self, _: CanopyRequest) -> Result<CanopyResponse> {
			Err(Error::transport(std::io::Error::other(
				"connection refused",
			)))
		}
	}

	let err = CanopyClient::new(Unreachable)
		.status_check_severities("a-server")
		.await
		.expect_err("no response was obtained");

	assert!(matches!(err, Error::Transport(_)));
	assert!(
		err.status().is_none(),
		"a failure to reach canopy carries no status"
	);
}

#[tokio::test]
async fn a_small_request_body_is_sent_uncompressed() {
	let recorder = Recorder::json(200, r#"{"ok":true}"#);
	let client = CanopyClient::new(recorder);

	let payload = bes_canopy_api::schema::StatusPayload::builder()
		.health(vec![])
		.build();
	let _ = client.status("a-server", &payload).await;

	let request = client.transport().last();
	assert_eq!(
		request.headers().get(http::header::CONTENT_TYPE).unwrap(),
		"application/json"
	);
	assert!(
		request
			.headers()
			.get(http::header::CONTENT_ENCODING)
			.is_none(),
		"a body this small costs more to compress than it saves"
	);
	assert_eq!(
		serde_json::from_slice::<serde_json::Value>(request.body()).expect("plain JSON"),
		serde_json::to_value(&payload).expect("serialising the payload")
	);
}

#[tokio::test]
async fn a_large_request_body_is_gzipped_and_says_so() {
	use std::io::Read as _;

	let recorder = Recorder::json(200, r#"{"ok":true}"#);
	let client = CanopyClient::new(recorder);

	// Well past the threshold, so the branch is taken regardless of how JSON
	// serialisation rounds out.
	let mut extra = serde_json::Map::new();
	for i in 0..200 {
		extra.insert(format!("key_number_{i}"), serde_json::json!("some value"));
	}
	let payload = bes_canopy_api::schema::StatusPayload::builder()
		.health(vec![])
		.extra(extra)
		.build();
	let _ = client.status("a-server", &payload).await;

	let request = client.transport().last();
	assert_eq!(
		request
			.headers()
			.get(http::header::CONTENT_ENCODING)
			.unwrap(),
		"gzip"
	);

	let mut decoded = Vec::new();
	flate2::read::GzDecoder::new(request.body().as_ref())
		.read_to_end(&mut decoded)
		.expect("the body is gzip");
	assert_eq!(
		serde_json::from_slice::<serde_json::Value>(&decoded).expect("the JSON inside"),
		serde_json::to_value(&payload).expect("serialising the payload")
	);
}

/// What the artifact registration endpoints answer with.
fn an_artifact() -> String {
	serde_json::json!({
		"id": "00000000-0000-0000-0000-000000000001",
		"version_id": null,
		"artifact_type": "reporting-schema",
		"platform": "any",
		"download_url": "https://example.invalid/a",
		"device_id": null,
		"version_range_pattern": null,
		"digest": null,
	})
	.to_string()
}

#[tokio::test]
async fn a_call_written_against_the_published_signature_sends_what_it_always_sent() {
	let recorder = Recorder::json(200, &an_artifact());
	let client = CanopyClient::new(recorder);

	// Held in `String`s rather than written as literals, which is the shape a
	// real call site takes: the last one reaches the envelope by conversion
	// rather than by the deref coercion a `&str` parameter used to allow, and
	// that conversion is what keeps this call compiling.
	let group = String::from("a-group");
	let version = String::from("2.11.0");
	let artifact_type = String::from("reporting-schema");
	let platform = String::from("any");

	client
		.artifacts_groups(&group, &version, &artifact_type, &platform)
		.await
		.expect("a 200 carrying an artifact parses");

	let request = client.transport().last();
	assert_eq!(
		request.uri(),
		"/artifacts/groups/a-group/2.11.0/reporting-schema/any",
		"an envelope that names no parameter sends the bare path"
	);
	assert!(request.body().is_empty());
	assert!(
		request.headers().get(http::header::CONTENT_TYPE).is_none(),
		"a request with no body declares no content type, as this call did before \
		 the method carried an envelope"
	);
}

#[tokio::test]
async fn an_envelope_carries_the_bytes_and_the_query_parameter() {
	let recorder = Recorder::json(200, &an_artifact());
	let client = CanopyClient::new(recorder);

	let run: uuid::Uuid = "3f1e4d5c-0000-4000-8000-00000000002b"
		.parse()
		.expect("a uuid");
	let request = bes_canopy_api::schema::RegisterGroupArtifactRequest::builder()
		.platform("any")
		.body(bes_canopy_api::bytes::Bytes::from_static(b"schema bytes"))
		.run(run)
		.build();

	client
		.artifacts_groups("a-group", "2.11.0", "reporting-schema", request)
		.await
		.expect("a 200 carrying an artifact parses");

	let sent = client.transport().last();
	assert_eq!(
		sent.uri(),
		"/artifacts/groups/a-group/2.11.0/reporting-schema/any\
		 ?run=3f1e4d5c-0000-4000-8000-00000000002b"
	);
	assert_eq!(sent.body().as_ref(), b"schema bytes");
	assert_eq!(
		sent.headers().get(http::header::CONTENT_TYPE).unwrap(),
		"application/octet-stream"
	);
	assert!(
		sent.headers().get(http::header::CONTENT_ENCODING).is_none(),
		"canopy records the digest of these bytes, so they go up as they are"
	);
}

#[tokio::test]
async fn a_text_body_is_sent_as_text_and_an_unset_parameter_is_left_off() {
	let recorder = Recorder::json(200, &an_artifact());
	let client = CanopyClient::new(recorder);

	let request = bes_canopy_api::schema::RegisterArtifactRequest::builder()
		.platform("linux")
		.body("https://example.invalid/pkg?v=1&arch=x86")
		.digest("sha256-LCTbqp+/w==")
		.build();

	client
		.artifacts("2.11.0", "installer", request)
		.await
		.expect("a 200 carrying an artifact parses");

	let sent = client.transport().last();
	assert_eq!(
		sent.uri(),
		"/artifacts/2.11.0/installer/linux?digest=sha256-LCTbqp%2B%2Fw%3D%3D",
		"`group` was never set, so it is absent rather than empty, and a digest \
		 carrying reserved characters is encoded"
	);
	assert_eq!(
		sent.body().as_ref(),
		b"https://example.invalid/pkg?v=1&arch=x86",
		"the body is the caller's text, whatever it looks like"
	);
	assert_eq!(
		sent.headers().get(http::header::CONTENT_TYPE).unwrap(),
		"text/plain"
	);
}
