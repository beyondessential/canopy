//! What the generated types carry.

use bes_canopy_api::schema::{
	CheckResult, CredentialProcessOutput, HealthCheck, HealthCheckInstance, StatusPayload,
};

#[test]
fn a_credential_secret_is_readable_but_not_printed() {
	let creds: CredentialProcessOutput = serde_json::from_value(serde_json::json!({
		"Version": 1,
		"AccessKeyId": "AKIAEXAMPLE",
		"SecretAccessKey": "the-secret",
		"SessionToken": "the-token",
		"Expiration": "2026-09-04T02:00:00Z",
	}))
	.expect("the credential_process shape parses");

	assert_eq!(*creds.secret_access_key, "the-secret");
	assert_eq!(*creds.session_token, "the-token");

	let debug = format!("{creds:?}");
	assert!(
		!debug.contains("the-secret") && !debug.contains("the-token"),
		"a secret must not reach logs through Debug: {debug}"
	);
	assert!(
		debug.contains("AKIAEXAMPLE"),
		"the access key id is not a secret and stays visible"
	);
}

#[test]
fn a_secret_is_transparent_on_the_wire() {
	let creds: CredentialProcessOutput = serde_json::from_value(serde_json::json!({
		"Version": 1,
		"AccessKeyId": "AKIAEXAMPLE",
		"SecretAccessKey": "the-secret",
		"SessionToken": "the-token",
		"Expiration": "2026-09-04T02:00:00Z",
	}))
	.expect("the credential_process shape parses");

	let round_tripped = serde_json::to_value(&creds).expect("serialising the credentials");
	assert_eq!(round_tripped["SecretAccessKey"], "the-secret");
}

#[test]
fn a_timestamp_field_is_a_timestamp_not_text() {
	let creds: CredentialProcessOutput = serde_json::from_value(serde_json::json!({
		"Version": 1,
		"AccessKeyId": "AKIAEXAMPLE",
		"SecretAccessKey": "s",
		"SessionToken": "t",
		"Expiration": "2026-09-04T02:00:00Z",
	}))
	.expect("the credential_process shape parses");

	let expected: jiff::Timestamp = "2026-09-04T02:00:00Z".parse().expect("a valid instant");
	assert_eq!(creds.expiration, expected);
}

#[test]
fn a_status_push_carries_further_keys_alongside_its_declared_fields() {
	let payload: StatusPayload = serde_json::from_value(serde_json::json!({
		"source": "alertd",
		"health": [],
		"tamanuVersion": "2.11.0",
		"uptime": 12345,
	}))
	.expect("a status push with further keys parses");

	assert_eq!(payload.source.as_deref(), Some("alertd"));
	assert_eq!(payload.extra["tamanuVersion"], "2.11.0");
	assert_eq!(payload.extra["uptime"], 12345);

	let round_tripped = serde_json::to_value(&payload).expect("serialising the push");
	assert_eq!(
		round_tripped["tamanuVersion"], "2.11.0",
		"further keys are sent, not dropped"
	);
	assert!(
		round_tripped.get("extra").is_none(),
		"the further keys are flattened, not nested under a field"
	);
}

#[test]
fn a_check_carries_its_own_further_keys() {
	let check: HealthCheck = serde_json::from_value(serde_json::json!({
		"check": "disk-space",
		"result": "warning",
		"free_percent": 8,
	}))
	.expect("a check with further keys parses");

	assert_eq!(check.check, "disk-space");
	assert_eq!(check.result, Some(CheckResult::Warning));
	assert_eq!(check.extra["free_percent"], 8);
}

#[test]
fn a_struct_is_built_without_naming_every_field() {
	// A schema gaining a field leaves this call site working, which is what makes
	// adding an optional property a compatible change.
	let payload = StatusPayload::builder().health(vec![]).build();

	assert!(payload.source.is_none());
	assert!(payload.extra.is_empty());
}

#[test]
fn a_check_is_built_without_its_optional_detail() {
	// `detail` is an optional object, which the generated type holds as a bare
	// map: the builder must not demand it, or its addition broke every call
	// site building a check.
	let check = HealthCheck::builder()
		.check("disk-space".into())
		.result(CheckResult::Passed)
		.build();

	let sent = serde_json::to_value(&check).expect("serialising the check");
	assert_eq!(
		sent,
		serde_json::json!({ "check": "disk-space", "result": "passed" }),
		"an unset detail is not sent, so a flat-form check stays valid",
	);
}

#[test]
fn a_check_carries_instances_with_their_own_detail() {
	let instance = HealthCheckInstance::builder()
		.result(CheckResult::Failed)
		.label("Northgate Clinic".into())
		.detail(serde_json::Map::from_iter([(
			"minutes_since_success".to_string(),
			serde_json::json!(2875.4),
		)]))
		.build();
	let check = HealthCheck::builder()
		.check("sync_facility_stale".into())
		.detail(serde_json::Map::from_iter([(
			"fail_minutes".to_string(),
			serde_json::json!(30),
		)]))
		.instances([("6f1c2a9e".to_string(), instance)].into_iter().collect())
		.build();

	assert_eq!(
		serde_json::to_value(&check).expect("serialising the check"),
		serde_json::json!({
			"check": "sync_facility_stale",
			"detail": { "fail_minutes": 30 },
			"instances": {
				"6f1c2a9e": {
					"result": "failed",
					"label": "Northgate Clinic",
					"detail": { "minutes_since_success": 2875.4 },
				},
			},
		}),
	);
}
