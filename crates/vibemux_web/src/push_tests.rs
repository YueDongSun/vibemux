const INSTANCE_TOKEN: &str = "test_token_abcdefghijklmnopqrstuvwxyz_0123456789";
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn private_temporary_state() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("temporary state directory");
    // This test is the producer/owner of the directory, just as InstanceLease is in production.
    #[cfg(windows)]
    vibemux_platform::secure_user_directory(temp.path()).expect("restrict fixture ACL");
    temp
}

fn subscription(endpoint: &str) -> PushSubscription {
    let keys = generated_browser_keys();
    PushSubscription {
        endpoint: endpoint.to_owned(),
        expiration_time: None,
        keys,
    }
}

fn generated_browser_keys() -> PushKeys {
    use openssl::{
        bn::BigNumContext,
        ec::{EcGroup, EcKey, PointConversionForm},
        nid::Nid,
    };

    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).expect("P-256 group");
    let key = EcKey::generate(&group).expect("browser keypair");
    let mut context = BigNumContext::new().expect("P-256 context");
    let p256dh = key
        .public_key()
        .to_bytes(&group, PointConversionForm::UNCOMPRESSED, &mut context)
        .expect("uncompressed browser public key");
    let mut auth = [0_u8; 16];
    getrandom::fill(&mut auth).expect("browser auth secret");
    PushKeys {
        p256dh: URL_SAFE_NO_PAD.encode(p256dh),
        auth: URL_SAFE_NO_PAD.encode(auth),
    }
}

#[derive(Default)]
struct CountingDelivery(AtomicUsize);

impl EventDelivery for CountingDelivery {
    fn deliver<'a>(
        &'a self,
        _event: &'a PushEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), PushError>> + Send + 'a>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn snapshot_delivery_only_sends_state_changes_and_survives_restart() {
    let temp = private_temporary_state();
    let make_service = || {
        PushService::startup(
            temp.path(),
            vec!["push.example.test".into()],
            "mailto:operator@example.test".into(),
            INSTANCE_TOKEN,
        )
        .expect("startup")
    };
    let service = make_service();
    let delivery = CountingDelivery::default();
    let task_id = "task_1";
    let mut snapshot = crate::control_bridge::BridgeSnapshot {
        generation: Some("0123456789abcdef0123456789abcdef".into()),
        stale: false,
        tasks: vec![crate::control_bridge::BridgeTask {
            task_id: task_id.into(),
            title: Some("private title never sent".into()),
            kind: "task".into(),
            state: "in_progress".into(),
            latest_sequence: 9,
            runs: Vec::new(),
            dispatch: None,
            workflow: None,
        }],
        ..Default::default()
    };

    service
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("initial baseline");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 0);

    snapshot.tasks[0].latest_sequence = 10;
    service
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("same-state sequence advance");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 0);

    snapshot.tasks[0].state = "done".into();
    snapshot.tasks[0].latest_sequence = 11;
    service
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("state transition");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 1);
    service
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("repeated unchanged snapshot");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 1);
    drop(service);

    let recovered = make_service();
    recovered
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("restart with unchanged snapshot");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 1);
    snapshot.generation = Some("abcdef0123456789abcdef0123456789".into());
    snapshot.tasks[0].state = "in_progress".into();
    snapshot.tasks[0].latest_sequence = 1;
    recovered
        .observe_snapshot_with(&snapshot, &delivery)
        .await
        .expect("changed state after daemon sequence reset");
    assert_eq!(delivery.0.load(Ordering::SeqCst), 2);
}

#[test]
fn subscriptions_require_explicit_https_host_and_valid_browser_keys() {
    let allowed = BTreeSet::from(["push.example.test".to_owned()]);
    assert!(
        subscription("https://push.example.test/send/opaque")
            .validate(&allowed)
            .is_ok()
    );
    for endpoint in [
        "http://push.example.test/send/opaque",
        "https://other.example.test/send/opaque",
        "https://user@push.example.test/send/opaque",
        "https://push.example.test:444/send/opaque",
        "https://push.example.test/send/opaque?token=secret",
        "https://push.example.test/send/opaque#fragment",
        "https://127.0.0.1/send/opaque",
        "https://[::1]/send/opaque",
    ] {
        assert!(
            subscription(endpoint).validate(&allowed).is_err(),
            "accepted {endpoint}"
        );
    }
    let mut malformed = subscription("https://push.example.test/send/opaque");
    malformed.keys.auth = "not-a-key".into();
    assert!(malformed.validate(&allowed).is_err());
}

#[test]
fn vapid_subject_and_allowed_host_require_valid_identifiers() {
    assert!(valid_vapid_subject("mailto:operator@example.test"));
    assert!(valid_vapid_subject("https://operator.example.test/contact"));
    assert!(!valid_vapid_subject("mailto:"));
    assert!(!valid_vapid_subject("https://user@operator.example.test/"));
    assert!(
        PushService::startup(
            tempfile::tempdir().expect("temporary state").path(),
            vec!["push.example.test/path".into()],
            "mailto:operator@example.test".into(),
            INSTANCE_TOKEN,
        )
        .is_err()
    );
}

#[test]
fn address_filter_rejects_private_and_nonroutable_results() {
    for address in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.2.3",
        "100.64.0.1",
        "192.0.0.8",
        "192.0.2.5",
        "192.88.99.1",
        "198.18.0.1",
        "198.51.100.2",
        "203.0.113.3",
        "2002:7f00:1::",
        "0.0.0.0",
        "224.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
    ] {
        assert!(
            !is_public_address(address.parse().expect("valid IP literal")),
            "accepted {address}"
        );
    }
    assert!(is_public_address(
        "8.8.8.8".parse().expect("valid public IPv4")
    ));
    assert!(is_public_address(
        "2606:4700:4700::1111".parse().expect("valid public IPv6")
    ));
    let mixed = [
        "8.8.8.8:443".parse().expect("public socket"),
        "10.0.0.8:443".parse().expect("private socket"),
    ];
    assert!(!are_public_addresses(&mixed));
    assert!(!are_public_addresses(&[]));
    assert!(are_public_addresses(&["8.8.8.8:443"
        .parse()
        .expect("public socket")]));
}

#[test]
fn state_persists_vapid_key_subscription_and_dedupe_across_restarts() {
    let temp = private_temporary_state();
    let service = PushService::startup(
        temp.path(),
        vec!["push.example.test".into()],
        "mailto:operator@example.test".into(),
        INSTANCE_TOKEN,
    )
    .expect("startup");
    let public_key = service.public_key();
    assert_eq!(
        URL_SAFE_NO_PAD
            .decode(&public_key)
            .expect("base64 public key")
            .len(),
        65
    );
    service
        .subscribe(subscription("https://push.example.test/send/opaque"))
        .expect("subscribe");
    let event = make_event(
        "0123456789abcdef0123456789abcdef",
        "task",
        "task_1",
        "in_progress",
        9,
    )
    .expect("valid event");
    let state_lock = Arc::clone(&service.state);
    assert_eq!(
        claim_event(&state_lock, &service.state_path, &event).expect("claim first event"),
        EventDecision::Baseline
    );
    assert_eq!(
        claim_event(&state_lock, &service.state_path, &event).expect("dedupe replay"),
        EventDecision::Unchanged
    );

    drop(service);
    let recovered = PushService::startup(
        temp.path(),
        vec!["push.example.test".into()],
        "mailto:operator@example.test".into(),
        INSTANCE_TOKEN,
    )
    .expect("restart");
    assert_eq!(recovered.public_key(), public_key);
    assert_eq!(
        recovered
            .state
            .lock()
            .expect("state lock")
            .subscriptions
            .len(),
        1
    );
    let state_lock = Arc::clone(&recovered.state);
    assert_eq!(
        claim_event(&state_lock, &recovered.state_path, &event).expect("dedupe recovered event"),
        EventDecision::Unchanged
    );
    let next = make_event(
        "0123456789abcdef0123456789abcdef",
        "task",
        "task_1",
        "done",
        10,
    )
    .expect("next event");
    assert_eq!(
        claim_event(&state_lock, &recovered.state_path, &next).expect("new state event"),
        EventDecision::Send
    );
    let new_generation = make_event(
        "fedcba9876543210fedcba9876543210",
        "task",
        "task_1",
        "done",
        1,
    )
    .expect("new generation event");
    assert_eq!(
        claim_event(&state_lock, &recovered.state_path, &new_generation)
            .expect("generation restart event"),
        EventDecision::Baseline
    );
    let state_reappears = make_event(
        "fedcba9876543210fedcba9876543210",
        "task",
        "task_1",
        "in_progress",
        2,
    )
    .expect("later state transition");
    assert_eq!(
        claim_event(&state_lock, &recovered.state_path, &state_reappears)
            .expect("later event with previously seen state"),
        EventDecision::Send
    );
}

#[test]
fn failed_persistence_does_not_accept_a_subscription_or_advance_dedupe() {
    let temp = private_temporary_state();
    let mut service = PushService::startup(
        temp.path(),
        vec!["push.example.test".into()],
        "mailto:operator@example.test".into(),
        INSTANCE_TOKEN,
    )
    .expect("startup");
    let persisted_path = service.state_path.clone();
    service.state_path = temp.path().to_path_buf();
    assert!(
        service
            .subscribe(subscription("https://push.example.test/send/opaque"))
            .is_err()
    );
    assert!(
        service
            .state
            .lock()
            .expect("state lock")
            .subscriptions
            .is_empty()
    );

    let event = make_event(
        "0123456789abcdef0123456789abcdef",
        "task",
        "task_1",
        "open",
        1,
    )
    .expect("valid event");
    let state = Arc::clone(&service.state);
    assert!(claim_event(&state, &service.state_path, &event).is_err());
    assert!(
        service
            .state
            .lock()
            .expect("state lock")
            .recent_events
            .is_empty()
    );

    service.state_path = persisted_path;
    assert_eq!(
        claim_event(&state, &service.state_path, &event).expect("claim after storage recovers"),
        EventDecision::Baseline
    );
}

#[test]
fn response_status_and_subscription_retention_are_bounded() {
    assert!(classify_response(201).is_ok());
    assert!(matches!(
        classify_response(404),
        Err(DeliveryFailure::Expired)
    ));
    assert!(matches!(
        classify_response(410),
        Err(DeliveryFailure::Expired)
    ));
    assert!(matches!(
        classify_response(401),
        Err(DeliveryFailure::Permanent)
    ));
    assert!(matches!(
        classify_response(302),
        Err(DeliveryFailure::Permanent)
    ));
    assert!(response_length_allowed(None));
    assert!(response_length_allowed(Some(MAX_RESPONSE_BYTES as u64)));
    assert!(!response_length_allowed(Some(
        MAX_RESPONSE_BYTES as u64 + 1
    )));
    assert_eq!(
        bounded_response_size(MAX_RESPONSE_BYTES - 1, 1),
        Some(MAX_RESPONSE_BYTES)
    );
    assert_eq!(bounded_response_size(MAX_RESPONSE_BYTES, 1), None);

    let now = now_seconds();
    let mut state = PushState {
        schema_version: PUSH_SCHEMA_VERSION,
        vapid_private_key: String::new(),
        instance_token_epoch: None,
        subscriptions: vec![
            StoredSubscription {
                subscription: subscription("https://push.example.test/old"),
                created_at: now.saturating_sub(PRUNE_AGE.as_secs() + 1),
                consecutive_failures: 0,
                quarantined: false,
            },
            StoredSubscription {
                subscription: PushSubscription {
                    expiration_time: Some(now_millis().saturating_sub(1)),
                    ..subscription("https://push.example.test/expired")
                },
                created_at: now,
                consecutive_failures: 0,
                quarantined: false,
            },
            StoredSubscription {
                subscription: subscription("https://push.example.test/current"),
                created_at: now,
                consecutive_failures: 0,
                quarantined: false,
            },
        ],
        recent_events: BTreeMap::new(),
    };
    prune_subscriptions(&mut state);
    assert_eq!(state.subscriptions.len(), 1);
    assert_eq!(
        state.subscriptions[0].subscription.endpoint,
        "https://push.example.test/current"
    );
}

#[tokio::test]
async fn expired_subscriptions_are_removed_and_permanent_failures_quarantine() {
    let temp = private_temporary_state();
    let service = PushService::startup(
        temp.path(),
        vec!["push.example.test".into()],
        "mailto:operator@example.test".into(),
        INSTANCE_TOKEN,
    )
    .expect("startup");
    let endpoint = "https://push.example.test/send/opaque";
    service
        .subscribe(subscription(endpoint))
        .expect("subscribe");
    for _ in 0..MAX_CONSECUTIVE_FAILURES {
        service
            .record_delivery_failure(endpoint.to_owned(), DeliveryFailure::Permanent)
            .await
            .expect("record failure");
    }
    assert!(service.state.lock().expect("state lock").subscriptions[0].quarantined);
    service
        .record_delivery_failure(endpoint.to_owned(), DeliveryFailure::Expired)
        .await
        .expect("drop expired endpoint");
    assert!(
        service
            .state
            .lock()
            .expect("state lock")
            .subscriptions
            .is_empty()
    );
}

#[test]
fn event_payload_is_a_strict_identifier_and_state_whitelist() {
    let event = make_event(
        "0123456789abcdef0123456789abcdef",
        "run",
        "run-1",
        "succeeded",
        3,
    )
    .expect("valid event");
    let payload = PushPayload::from(&event);
    let json = serde_json::to_value(&payload).expect("serialize payload");
    assert_eq!(json.as_object().expect("object").len(), 3);
    assert_eq!(
        serde_json::to_string(&payload).expect("payload wire JSON"),
        r#"{"kind":"run","id":"run-1","state":"succeeded"}"#
    );
    assert!(json.get("title").is_none());
    assert!(json.get("branch").is_none());
    assert!(json.get("path").is_none());
    assert!(json.get("generation").is_none());
    assert!(json.get("control_sequence").is_none());
    assert!(
        make_event(
            "0123456789abcdef0123456789abcdef",
            "task",
            "task",
            "freeform_title",
            1
        )
        .is_none()
    );
    assert!(
        make_event(
            "0123456789abcdef0123456789abcdef",
            "task",
            "task",
            "in_progress",
            1
        )
        .is_some()
    );
}

#[test]
fn encrypted_push_request_is_constructible_without_network_io() {
    let mut private = [0u8; 32];
    getrandom::fill(&mut private).expect("random key");
    let private = URL_SAFE_NO_PAD.encode(private);
    let keys = generated_browser_keys();
    let subscription = SubscriptionInfo::new(
        "https://push.example.test/send/opaque".to_owned(),
        keys.p256dh,
        keys.auth,
    );
    let mut signature = VapidSignatureBuilder::from_base64_no_sub(&private)
        .expect("VAPID key")
        .add_sub_info(&subscription);
    signature.add_claim("sub", "mailto:operator@example.test");
    let mut message = WebPushMessageBuilder::new(&subscription);
    message.set_ttl(300);
    message.set_payload(
        ContentEncoding::Aes128Gcm,
        br#"{"kind":"task","id":"task_1","state":"in_progress"}"#,
    );
    message.set_vapid_signature(signature.build().expect("VAPID signature"));
    let request = build_request::<Vec<u8>>(message.build().expect("encrypted message"));
    assert_eq!(request.method(), "POST");
    assert!(request.headers().contains_key("authorization"));
    assert_eq!(
        request
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("aes128gcm")
    );
    assert!(request.body().len() > 80);
}

#[test]
fn token_rotation_revokes_all_subscriptions_without_rotating_vapid_keys() {
    let temp = private_temporary_state();
    let start = |token: &str| {
        PushService::startup(
            temp.path(),
            vec!["push.example.test".into()],
            "mailto:operator@example.test".into(),
            token,
        )
        .expect("startup")
    };
    let service = start(INSTANCE_TOKEN);
    let public_key = service.public_key();
    service
        .subscribe(subscription("https://push.example.test/send/opaque"))
        .expect("subscribe");
    drop(service);
    let unchanged = start(INSTANCE_TOKEN);
    assert_eq!(
        unchanged.state.lock().expect("state").subscriptions.len(),
        1
    );
    drop(unchanged);
    let rotated = start("rotated_token_abcdefghijklmnopqrstuvwxyz_0123456789");
    assert!(
        rotated
            .state
            .lock()
            .expect("state")
            .subscriptions
            .is_empty()
    );
    assert_eq!(rotated.public_key(), public_key);
    assert!(
        !std::fs::read_to_string(&rotated.state_path)
            .expect("state JSON")
            .contains(INSTANCE_TOKEN)
    );
}
