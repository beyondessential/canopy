// @generated automatically by Diesel CLI.

diesel::table! {
	admins (email) {
		email -> Text,
		created_at -> Timestamptz,
	}
}

diesel::table! {
	application_certificates (id) {
		id -> Uuid,
		application_id -> Uuid,
		name -> Text,
		key_fingerprint -> Text,
		csr -> Bytea,
		state -> Text,
		chain -> Nullable<Text>,
		not_after -> Nullable<Timestamptz>,
		issued_at -> Nullable<Timestamptz>,
		renewing -> Bool,
		attempts -> Int4,
		next_attempt_at -> Timestamptz,
		last_error -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		profile -> Nullable<Text>,
		renew_after -> Nullable<Timestamptz>,
		revoked_at -> Nullable<Timestamptz>,
		revoked_by -> Nullable<Text>,
		revocation_reason -> Nullable<Text>,
	}
}

diesel::table! {
	application_names (id) {
		id -> Uuid,
		application_id -> Uuid,
		name -> Text,
		addresses -> Array<Nullable<Inet>>,
		published_addresses -> Array<Nullable<Inet>>,
		published_at -> Nullable<Timestamptz>,
		last_error -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
	}
}

diesel::table! {
	application_reported_detail (application_id, source) {
		application_id -> Uuid,
		source -> Text,
		extra -> Jsonb,
		version -> Nullable<Text>,
		reported_at -> Timestamptz,
	}
}

diesel::table! {
	applications (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		name -> Nullable<Text>,
		rank -> Nullable<Text>,
		host -> Nullable<Text>,
		cloud -> Nullable<Bool>,
		geolocation -> Nullable<Array<Nullable<Float8>>>,
		alert_when_down_for -> Interval,
		group_id -> Nullable<Uuid>,
		notes -> Text,
		tags -> Jsonb,
		public_name -> Nullable<Text>,
		is_monitored -> Bool,
		deleted_at -> Nullable<Timestamptz>,
		registered_at -> Nullable<Timestamptz>,
		may_manage_dns -> Bool,
		may_manage_tls -> Bool,
		certificate_profile -> Nullable<Text>,
		name_management_paused_at -> Nullable<Timestamptz>,
		name_management_paused_by -> Nullable<Text>,
		name_management_pause_reason -> Nullable<Text>,
		machine_id -> Uuid,
		#[sql_name = "type"]
		type_ -> Text,
		reported_key -> Nullable<Text>,
	}
}

diesel::table! {
	artifacts (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		version_id -> Nullable<Uuid>,
		artifact_type -> Text,
		platform -> Text,
		download_url -> Nullable<Text>,
		device_id -> Nullable<Uuid>,
		version_range_pattern -> Nullable<Text>,
		group_id -> Nullable<Uuid>,
		content -> Nullable<Bytea>,
		content_type -> Nullable<Text>,
		digest -> Nullable<Text>,
		run_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	backup_credential_issuances (id) {
		id -> Int8,
		device_id -> Uuid,
		group_id -> Uuid,
		#[sql_name = "type"]
		type_ -> Text,
		issued_at -> Timestamptz,
		expires_at -> Timestamptz,
		purpose -> Text,
		sts_assumed_role -> Text,
		sts_request_id -> Nullable<Text>,
		access_key_id -> Nullable<Text>,
		bucket -> Text,
		prefix -> Text,
		run_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	backup_maintenance_runs (id) {
		id -> Int8,
		group_id -> Uuid,
		kind -> Text,
		started_at -> Timestamptz,
		finished_at -> Nullable<Timestamptz>,
		outcome -> Nullable<Text>,
		error -> Nullable<Text>,
		bytes_reclaimed -> Nullable<Int8>,
	}
}

diesel::table! {
	backup_recovery_verifications (id) {
		id -> Int8,
		verified_at -> Timestamptz,
		recipients -> Jsonb,
	}
}

diesel::table! {
	backup_repo_observed_snapshots (group_id, snapshot_id) {
		group_id -> Uuid,
		snapshot_id -> Text,
		source -> Text,
		snapshot_at -> Nullable<Timestamptz>,
		observed_at -> Timestamptz,
	}
}

diesel::table! {
	backup_repo_snapshots (group_id, source) {
		group_id -> Uuid,
		source -> Text,
		machine_id -> Nullable<Uuid>,
		#[sql_name = "type"]
		type_ -> Nullable<Text>,
		latest_snapshot_at -> Nullable<Timestamptz>,
		observed_at -> Timestamptz,
	}
}

diesel::table! {
	backup_repo_stats (group_id) {
		group_id -> Uuid,
		snapshot_count -> Nullable<Int4>,
		source_count -> Nullable<Int4>,
		logical_bytes -> Nullable<Int8>,
		physical_bytes -> Nullable<Int8>,
		bucket_bytes -> Nullable<Int8>,
		observed_at -> Timestamptz,
		bucket_bytes_observed_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	backup_requests (machine_id, type_, purpose) {
		machine_id -> Uuid,
		#[sql_name = "type"]
		type_ -> Text,
		purpose -> Text,
		requested_at -> Timestamptz,
		requested_by -> Nullable<Text>,
	}
}

diesel::table! {
	backup_restore_checks (id) {
		id -> Int8,
		replica_id -> Nullable<Uuid>,
		consumer_device_id -> Uuid,
		group_id -> Uuid,
		machine_id -> Nullable<Uuid>,
		#[sql_name = "type"]
		type_ -> Text,
		intent -> Text,
		snapshot_id -> Nullable<Text>,
		outcome -> Text,
		error -> Nullable<Text>,
		replica_healthy -> Bool,
		postgres_version -> Nullable<Text>,
		observed_at -> Timestamptz,
		s3_sent_raw_bytes -> Nullable<Int8>,
		s3_sent_payload_bytes -> Nullable<Int8>,
		s3_received_raw_bytes -> Nullable<Int8>,
		s3_received_payload_bytes -> Nullable<Int8>,
		reported_at -> Timestamptz,
		health_details -> Nullable<Jsonb>,
		run_id -> Nullable<Uuid>,
		redaction_outcome -> Nullable<Text>,
		redaction_manifest_version -> Nullable<Text>,
		redaction_columns_masked -> Nullable<Int8>,
		redaction_columns_skipped -> Nullable<Int8>,
		redaction_error -> Nullable<Text>,
		replica_name -> Nullable<Text>,
	}
}

diesel::table! {
	backup_run_progress (id) {
		id -> Int8,
		run_id -> Uuid,
		device_id -> Uuid,
		group_id -> Uuid,
		machine_id -> Nullable<Uuid>,
		#[sql_name = "type"]
		type_ -> Text,
		purpose -> Text,
		observed_at -> Timestamptz,
		snapshot_taken_at -> Nullable<Timestamptz>,
		bytes_read -> Nullable<Int8>,
		bytes_hashed -> Nullable<Int8>,
		bytes_uploaded -> Nullable<Int8>,
		bytes_cached -> Nullable<Int8>,
		bytes_estimated -> Nullable<Int8>,
		files_done -> Nullable<Int8>,
		files_estimated -> Nullable<Int8>,
		errors -> Nullable<Int8>,
		ignored_errors -> Nullable<Int8>,
		current_path -> Nullable<Text>,
		s3_sent_raw_bytes -> Nullable<Int8>,
		s3_sent_payload_bytes -> Nullable<Int8>,
		s3_received_raw_bytes -> Nullable<Int8>,
		s3_received_payload_bytes -> Nullable<Int8>,
		extra -> Jsonb,
	}
}

diesel::table! {
	backup_runs (id) {
		id -> Uuid,
		device_id -> Uuid,
		group_id -> Uuid,
		machine_id -> Nullable<Uuid>,
		#[sql_name = "type"]
		type_ -> Text,
		purpose -> Text,
		outcome -> Text,
		error -> Nullable<Text>,
		bytes_uploaded -> Nullable<Int8>,
		snapshot_id -> Nullable<Text>,
		reported_at -> Timestamptz,
		s3_sent_raw_bytes -> Nullable<Int8>,
		s3_sent_payload_bytes -> Nullable<Int8>,
		s3_received_raw_bytes -> Nullable<Int8>,
		s3_received_payload_bytes -> Nullable<Int8>,
		snapshot_logical_bytes -> Nullable<Int8>,
		snapshot_taken_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	backup_type_defaults (type_) {
		#[sql_name = "type"]
		type_ -> Text,
		default_interval -> Nullable<Interval>,
		default_retention -> Jsonb,
		auto_enable -> Bool,
		allow_below_floor -> Bool,
	}
}

diesel::table! {
	bestool_snippets (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		deleted_at -> Nullable<Timestamptz>,
		supersedes_id -> Nullable<Uuid>,
		editor -> Text,
		name -> Text,
		description -> Nullable<Text>,
		sql -> Text,
	}
}

diesel::table! {
	check_policies (id) {
		source -> Text,
		check_name -> Text,
		ceiling -> Text,
		escalates -> Bool,
		rules -> Nullable<Jsonb>,
		notes -> Nullable<Text>,
		first_seen -> Timestamptz,
		reviewed_at -> Nullable<Timestamptz>,
		reviewed_by -> Nullable<Text>,
		updated_at -> Timestamptz,
		documentation -> Nullable<Text>,
		last_seen -> Nullable<Timestamptz>,
		decommissioned_at -> Nullable<Timestamptz>,
		decommissioned_by -> Nullable<Text>,
		subject -> Nullable<Text>,
		application_type -> Nullable<Text>,
		id -> Uuid,
	}
}

diesel::table! {
	check_stability (issue_id) {
		issue_id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		observations -> Int8,
		degraded_observations -> Int8,
		last_observed_at -> Nullable<Timestamptz>,
		last_observed_degraded -> Nullable<Bool>,
		transitions -> Jsonb,
		duty_cycle -> Jsonb,
	}
}

diesel::table! {
	check_stability_backfill (done_at) {
		done_at -> Timestamptz,
	}
}

diesel::table! {
	chrome_releases (version) {
		version -> Text,
		release_date -> Text,
		is_eol -> Bool,
		eol_from -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
	}
}

diesel::table! {
	compromised_keys (key_fingerprint) {
		key_fingerprint -> Text,
		certificate_id -> Nullable<Uuid>,
		noted_by -> Nullable<Text>,
		noted_at -> Timestamptz,
	}
}

diesel::table! {
	device_connections (id, created_at) {
		id -> Uuid,
		created_at -> Timestamptz,
		device_id -> Uuid,
		ip -> Inet,
		user_agent -> Nullable<Text>,
	}
}

diesel::table! {
	device_keys (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		device_id -> Uuid,
		key_data -> Bytea,
		name -> Nullable<Text>,
		is_active -> Bool,
	}
}

diesel::table! {
	devices (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		role -> Text,
		tailscale_node_id -> Nullable<Text>,
		tailscale_node_name -> Nullable<Text>,
		tailscale_tailnet -> Nullable<Text>,
	}
}

diesel::table! {
	incident_issues (incident_id, issue_id, joined_at) {
		incident_id -> Uuid,
		issue_id -> Uuid,
		joined_at -> Timestamptz,
		left_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	incident_notes (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		incident_id -> Uuid,
		author -> Text,
		body -> Text,
	}
}

diesel::table! {
	incident_reeval_queue (application_id) {
		application_id -> Uuid,
		enqueued_at -> Timestamptz,
	}
}

diesel::table! {
	incidents (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		opened_at -> Timestamptz,
		closed_at -> Nullable<Timestamptz>,
		resolved_at -> Nullable<Timestamptz>,
		resolved_by -> Nullable<Text>,
		resolved_reason -> Nullable<Text>,
		server_group_id -> Nullable<Uuid>,
		escalated_at -> Nullable<Timestamptz>,
		closing_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	issue_notes (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		issue_id -> Uuid,
		author -> Text,
		body -> Text,
	}
}

diesel::table! {
	issues (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		application_id -> Nullable<Uuid>,
		device_id -> Nullable<Uuid>,
		source -> Text,
		#[sql_name = "ref"]
		ref_ -> Text,
		description -> Nullable<Text>,
		message -> Text,
		active -> Bool,
		first_seen -> Timestamptz,
		last_seen -> Timestamptz,
		resolved_at -> Nullable<Timestamptz>,
		resolved_by -> Nullable<Text>,
		resolved_reason -> Nullable<Text>,
		snoozed_until -> Nullable<Timestamptz>,
		server_group_id -> Nullable<Uuid>,
		check_name -> Nullable<Text>,
		observed_result -> Nullable<Text>,
		effective_result -> Nullable<Text>,
		detail -> Nullable<Jsonb>,
		degraded_since -> Nullable<Timestamptz>,
		last_degraded_at -> Nullable<Timestamptz>,
		escalates -> Bool,
		machine_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	machine_backup_capabilities (machine_id, type_) {
		machine_id -> Uuid,
		#[sql_name = "type"]
		type_ -> Text,
		enabled -> Bool,
		registered_at -> Timestamptz,
	}
}

diesel::table! {
	machine_enrollment_challenges (id) {
		id -> Uuid,
		machine_id -> Uuid,
		token_hash -> Bytea,
		public_key -> Bytea,
		nonce -> Bytea,
		created_at -> Timestamptz,
		expires_at -> Timestamptz,
		used_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	machine_enrollment_tokens (id) {
		id -> Uuid,
		machine_id -> Uuid,
		token_hash -> Bytea,
		created_at -> Timestamptz,
		expires_at -> Timestamptz,
		consumed_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	machine_reported_detail (machine_id, source) {
		machine_id -> Uuid,
		source -> Text,
		extra -> Jsonb,
		reported_at -> Timestamptz,
	}
}

diesel::table! {
	machines (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		name -> Nullable<Text>,
		group_id -> Nullable<Uuid>,
		device_id -> Nullable<Uuid>,
		cloud -> Nullable<Bool>,
		geolocation -> Nullable<Array<Nullable<Float8>>>,
		alert_when_down_for -> Interval,
		is_monitored -> Bool,
		notes -> Text,
		tags -> Jsonb,
		deleted_at -> Nullable<Timestamptz>,
		registered_at -> Nullable<Timestamptz>,
		restore_allowed_until -> Nullable<Timestamptz>,
		restore_allowed_by -> Nullable<Text>,
	}
}

diesel::table! {
	maintenance_windows (id) {
		id -> Uuid,
		machine_id -> Nullable<Uuid>,
		server_group_id -> Nullable<Uuid>,
		expected_end -> Timestamptz,
		note -> Nullable<Text>,
		declared_by -> Nullable<Text>,
		declared_at -> Timestamptz,
		amended_by -> Nullable<Text>,
		amended_at -> Nullable<Timestamptz>,
		ended_at -> Nullable<Timestamptz>,
		ended_by -> Nullable<Text>,
		settled_at -> Nullable<Timestamptz>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
	}
}

diesel::table! {
	mcp_tokens (id) {
		id -> Uuid,
		name -> Text,
		token_hash -> Bytea,
		created_by -> Text,
		created_at -> Timestamptz,
		expires_at -> Timestamptz,
		revoked_at -> Nullable<Timestamptz>,
		last_used_at -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	migration_tests (check_id) {
		check_id -> Int8,
		target_version_id -> Uuid,
		total_elapsed -> Interval,
		failed_migration -> Nullable<Text>,
		data_bytes_before -> Int8,
		data_bytes_after -> Int8,
		application_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	migration_timings (check_id, ordinal) {
		check_id -> Int8,
		ordinal -> Int4,
		name -> Text,
		elapsed -> Interval,
	}
}

diesel::table! {
	recovery_vault_writes (id) {
		id -> Uuid,
		written_at -> Timestamptz,
		bytes -> Int8,
	}
}

diesel::table! {
	restore_consumer_capabilities (consumer_device_id, intent) {
		consumer_device_id -> Uuid,
		intent -> Text,
		registered_at -> Timestamptz,
		description -> Nullable<Text>,
		semantics -> Jsonb,
		params -> Jsonb,
	}
}

diesel::table! {
	restore_replicas (id) {
		id -> Uuid,
		consumer_device_id -> Uuid,
		group_id -> Uuid,
		machine_id -> Nullable<Uuid>,
		#[sql_name = "type"]
		type_ -> Text,
		intent -> Text,
		name -> Text,
		overdue_after -> Nullable<Interval>,
		enabled -> Bool,
		created_by -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		params -> Jsonb,
		redacts -> Bool,
	}
}

diesel::table! {
	scoped_check_policies (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		source -> Text,
		check_name -> Text,
		application_id -> Nullable<Uuid>,
		server_group_id -> Nullable<Uuid>,
		ceiling -> Nullable<Text>,
		rules -> Nullable<Jsonb>,
		created_by -> Nullable<Text>,
		machine_id -> Nullable<Uuid>,
		subject -> Nullable<Text>,
		application_type -> Nullable<Text>,
	}
}

diesel::table! {
	server_group_backup_config (group_id) {
		group_id -> Uuid,
		bucket -> Text,
		prefix -> Text,
		target_role_arn -> Text,
		region -> Nullable<Text>,
		repo_password_ref -> Text,
		status -> Text,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		mode -> Text,
		last_init_error -> Nullable<Text>,
		maintenance_role_arn -> Text,
		placement -> Text,
		force_full_maintenance_at -> Nullable<Timestamptz>,
		force_full_maintenance_by -> Nullable<Text>,
		repo_password_rotated_at -> Nullable<Timestamptz>,
		repo_password_rotating_since -> Nullable<Timestamptz>,
	}
}

diesel::table! {
	server_group_backup_schedule (group_id, type_) {
		group_id -> Uuid,
		#[sql_name = "type"]
		type_ -> Text,
		expected_interval -> Nullable<Interval>,
		retention -> Nullable<Jsonb>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		allow_below_floor -> Bool,
	}
}

diesel::table! {
	server_group_domains (id) {
		id -> Uuid,
		group_id -> Uuid,
		domain -> Text,
		created_by -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
	}
}

diesel::table! {
	server_groups (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		name -> Text,
		notes -> Text,
		tags -> Jsonb,
		slack_open_delay -> Interval,
		version_application_id -> Nullable<Uuid>,
		effective_version -> Nullable<Text>,
		deleted_at -> Nullable<Timestamptz>,
		slack_close_delay -> Interval,
	}
}

diesel::table! {
	slack_outbox (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		kind -> Text,
		incident_id -> Nullable<Uuid>,
		issue_id -> Nullable<Uuid>,
		note_id -> Nullable<Uuid>,
		payload -> Jsonb,
		delivered_at -> Nullable<Timestamptz>,
		attempts -> Int4,
		last_error -> Nullable<Text>,
		last_response -> Nullable<Text>,
		gave_up_at -> Nullable<Timestamptz>,
		deliver_after -> Timestamptz,
	}
}

diesel::table! {
	source_policies (source) {
		source -> Text,
		reachability -> Text,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		ingest -> Text,
	}
}

diesel::table! {
	sql_playground_history (id) {
		id -> Uuid,
		query -> Text,
		tailscale_user -> Text,
		created_at -> Timestamptz,
	}
}

diesel::table! {
	statuses (id, created_at) {
		id -> Uuid,
		created_at -> Timestamptz,
		server_id -> Nullable<Uuid>,
		version -> Nullable<Text>,
		extra -> Jsonb,
		device_id -> Nullable<Uuid>,
		healthy -> Bool,
		health -> Jsonb,
		source -> Text,
		machine_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	tailscale_users (login) {
		login -> Text,
		name -> Text,
		profile_pic -> Nullable<Text>,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
	}
}

diesel::table! {
	upgrade_plans (id) {
		id -> Uuid,
		group_id -> Uuid,
		target_version_id -> Uuid,
		planned_for -> Nullable<Date>,
		note -> Nullable<Text>,
		created_by -> Nullable<Text>,
		created_at -> Timestamptz,
		met_at -> Nullable<Timestamptz>,
		superseded_at -> Nullable<Timestamptz>,
		amended_by -> Nullable<Text>,
		amended_at -> Nullable<Timestamptz>,
		withdrawn_at -> Nullable<Timestamptz>,
		withdrawn_by -> Nullable<Text>,
		planned_time -> Nullable<Time>,
		planned_zone -> Nullable<Text>,
		planned_end_time -> Nullable<Time>,
	}
}

diesel::table! {
	version_known_issues (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		author -> Text,
		description -> Text,
		resolved_at -> Nullable<Timestamptz>,
		resolved_by -> Nullable<Text>,
		resolution_message -> Nullable<Text>,
		min_major -> Int4,
		min_minor -> Int4,
		min_patch -> Int4,
		max_major -> Nullable<Int4>,
		max_minor -> Nullable<Int4>,
		max_patch -> Nullable<Int4>,
		application_id -> Nullable<Uuid>,
	}
}

diesel::table! {
	versions (id) {
		id -> Uuid,
		created_at -> Timestamptz,
		updated_at -> Timestamptz,
		major -> Int4,
		minor -> Int4,
		patch -> Int4,
		changelog -> Text,
		status -> Text,
		device_id -> Nullable<Uuid>,
	}
}

diesel::joinable!(application_certificates -> applications (application_id));
diesel::joinable!(application_names -> applications (application_id));
diesel::joinable!(application_reported_detail -> applications (application_id));
diesel::joinable!(applications -> machines (machine_id));
diesel::joinable!(artifacts -> devices (device_id));
diesel::joinable!(artifacts -> server_groups (group_id));
diesel::joinable!(artifacts -> versions (version_id));
diesel::joinable!(backup_credential_issuances -> devices (device_id));
diesel::joinable!(backup_credential_issuances -> server_groups (group_id));
diesel::joinable!(backup_maintenance_runs -> server_groups (group_id));
diesel::joinable!(backup_repo_observed_snapshots -> server_groups (group_id));
diesel::joinable!(backup_repo_snapshots -> machines (machine_id));
diesel::joinable!(backup_repo_snapshots -> server_groups (group_id));
diesel::joinable!(backup_repo_stats -> server_groups (group_id));
diesel::joinable!(backup_requests -> machines (machine_id));
diesel::joinable!(backup_restore_checks -> devices (consumer_device_id));
diesel::joinable!(backup_restore_checks -> machines (machine_id));
diesel::joinable!(backup_restore_checks -> restore_replicas (replica_id));
diesel::joinable!(backup_restore_checks -> server_groups (group_id));
diesel::joinable!(backup_run_progress -> devices (device_id));
diesel::joinable!(backup_run_progress -> machines (machine_id));
diesel::joinable!(backup_run_progress -> server_groups (group_id));
diesel::joinable!(backup_runs -> devices (device_id));
diesel::joinable!(backup_runs -> machines (machine_id));
diesel::joinable!(backup_runs -> server_groups (group_id));
diesel::joinable!(check_stability -> issues (issue_id));
diesel::joinable!(compromised_keys -> application_certificates (certificate_id));
diesel::joinable!(device_connections -> devices (device_id));
diesel::joinable!(device_keys -> devices (device_id));
diesel::joinable!(incident_issues -> incidents (incident_id));
diesel::joinable!(incident_issues -> issues (issue_id));
diesel::joinable!(incident_notes -> incidents (incident_id));
diesel::joinable!(incident_reeval_queue -> applications (application_id));
diesel::joinable!(incidents -> server_groups (server_group_id));
diesel::joinable!(issue_notes -> issues (issue_id));
diesel::joinable!(issues -> applications (application_id));
diesel::joinable!(issues -> devices (device_id));
diesel::joinable!(issues -> machines (machine_id));
diesel::joinable!(issues -> server_groups (server_group_id));
diesel::joinable!(machine_backup_capabilities -> machines (machine_id));
diesel::joinable!(machine_enrollment_challenges -> machines (machine_id));
diesel::joinable!(machine_enrollment_tokens -> machines (machine_id));
diesel::joinable!(machine_reported_detail -> machines (machine_id));
diesel::joinable!(machines -> devices (device_id));
diesel::joinable!(machines -> server_groups (group_id));
diesel::joinable!(maintenance_windows -> machines (machine_id));
diesel::joinable!(maintenance_windows -> server_groups (server_group_id));
diesel::joinable!(migration_tests -> applications (application_id));
diesel::joinable!(migration_tests -> backup_restore_checks (check_id));
diesel::joinable!(migration_tests -> versions (target_version_id));
diesel::joinable!(migration_timings -> migration_tests (check_id));
diesel::joinable!(restore_consumer_capabilities -> devices (consumer_device_id));
diesel::joinable!(restore_replicas -> devices (consumer_device_id));
diesel::joinable!(restore_replicas -> machines (machine_id));
diesel::joinable!(restore_replicas -> server_groups (group_id));
diesel::joinable!(scoped_check_policies -> applications (application_id));
diesel::joinable!(scoped_check_policies -> machines (machine_id));
diesel::joinable!(scoped_check_policies -> server_groups (server_group_id));
diesel::joinable!(server_group_backup_config -> server_groups (group_id));
diesel::joinable!(server_group_backup_schedule -> server_groups (group_id));
diesel::joinable!(server_group_domains -> server_groups (group_id));
diesel::joinable!(slack_outbox -> incident_notes (note_id));
diesel::joinable!(slack_outbox -> incidents (incident_id));
diesel::joinable!(slack_outbox -> issues (issue_id));
diesel::joinable!(statuses -> applications (server_id));
diesel::joinable!(statuses -> devices (device_id));
diesel::joinable!(statuses -> machines (machine_id));
diesel::joinable!(upgrade_plans -> server_groups (group_id));
diesel::joinable!(upgrade_plans -> versions (target_version_id));
diesel::joinable!(version_known_issues -> applications (application_id));
diesel::joinable!(versions -> devices (device_id));

diesel::allow_tables_to_appear_in_same_query!(
	admins,
	application_certificates,
	application_names,
	application_reported_detail,
	applications,
	artifacts,
	backup_credential_issuances,
	backup_maintenance_runs,
	backup_recovery_verifications,
	backup_repo_observed_snapshots,
	backup_repo_snapshots,
	backup_repo_stats,
	backup_requests,
	backup_restore_checks,
	backup_run_progress,
	backup_runs,
	backup_type_defaults,
	bestool_snippets,
	check_policies,
	check_stability,
	check_stability_backfill,
	chrome_releases,
	compromised_keys,
	device_connections,
	device_keys,
	devices,
	incident_issues,
	incident_notes,
	incident_reeval_queue,
	incidents,
	issue_notes,
	issues,
	machine_backup_capabilities,
	machine_enrollment_challenges,
	machine_enrollment_tokens,
	machine_reported_detail,
	machines,
	maintenance_windows,
	mcp_tokens,
	migration_tests,
	migration_timings,
	recovery_vault_writes,
	restore_consumer_capabilities,
	restore_replicas,
	scoped_check_policies,
	server_group_backup_config,
	server_group_backup_schedule,
	server_group_domains,
	server_groups,
	slack_outbox,
	source_policies,
	sql_playground_history,
	statuses,
	tailscale_users,
	upgrade_plans,
	version_known_issues,
	versions,
);
