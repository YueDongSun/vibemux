//! Self-hosted, content-free Web Push delivery for the read-only mobile board.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    net::IpAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;
use url::Url;
use web_push::{
    ContentEncoding, SubscriptionInfo, SubscriptionKeys, VapidSignatureBuilder,
    WebPushMessageBuilder, request_builder::build_request,
};

const MAX_SUBSCRIPTIONS: usize = 32;
const MAX_PUSH_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEDUPE_ENTRIES: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 8 * 1024;
const MAX_ENDPOINT_CHARS: usize = 2048;
const MAX_KEY_CHARS: usize = 128;
const MAX_PAYLOAD_BYTES: usize = 1024;
const PRUNE_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const SWEEP_DEADLINE: Duration = Duration::from_secs(30);
const MAX_CANDIDATES_PER_SNAPSHOT: usize = 512;
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
const CONNECT_DEADLINE: Duration = Duration::from_secs(3);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(600)];
const MAX_CONSECUTIVE_FAILURES: u8 = 5;
const MAX_KEY_GENERATION_ATTEMPTS: usize = 64;
const PUSH_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
#[error("web_push_failed")]
pub struct PushError;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PushSubscription {
    pub endpoint: String,
    #[serde(rename = "expirationTime", default)]
    pub expiration_time: Option<u64>,
    pub keys: PushKeys,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

impl PushSubscription {
    fn validate(&self, allowed_hosts: &BTreeSet<String>) -> Result<(), PushError> {
        if self.endpoint.len() > MAX_ENDPOINT_CHARS
            || self.keys.p256dh.len() > MAX_KEY_CHARS
            || self.keys.auth.len() > MAX_KEY_CHARS
        {
            return Err(PushError);
        }
        let url = Url::parse(&self.endpoint).map_err(|_| PushError)?;
        let host = url.host_str().ok_or(PushError)?.to_ascii_lowercase();
        if url.scheme() != "https"
            || url.port().is_some_and(|port| port != 443)
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || host.is_empty()
            || host.parse::<IpAddr>().is_ok()
            || !allowed_hosts.contains(host.trim_end_matches('.'))
            || !valid_base64url(&self.keys.p256dh, 65)
            || !valid_base64url(&self.keys.auth, 16)
        {
            return Err(PushError);
        }
        if self
            .expiration_time
            .is_some_and(|expiration| expiration <= now_millis())
        {
            return Err(PushError);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PushState {
    schema_version: u32,
    vapid_private_key: String,
    #[serde(default)]
    instance_token_epoch: Option<String>,
    subscriptions: Vec<StoredSubscription>,
    recent_events: BTreeMap<String, EventVersion>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSubscription {
    subscription: PushSubscription,
    created_at: u64,
    consecutive_failures: u8,
    quarantined: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventVersion {
    generation: String,
    sequence: u64,
    state: String,
}

pub struct PushService {
    state_path: PathBuf,
    allowed_hosts: BTreeSet<String>,
    vapid_subject: String,
    state: Arc<Mutex<PushState>>,
    public_key: String,
}

impl PushService {
    pub fn startup(
        state_dir: &Path,
        allowed_hosts: Vec<String>,
        vapid_subject: String,
        instance_token: &str,
    ) -> Result<Self, PushError> {
        if !(43..=128).contains(&instance_token.len())
            || !instance_token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(PushError);
        }
        if !valid_vapid_subject(&vapid_subject) {
            return Err(PushError);
        }
        let allowed_hosts = allowed_hosts
            .into_iter()
            .map(|host| {
                let normalized = host.trim_end_matches('.').to_ascii_lowercase();
                if normalized.is_empty()
                    || normalized.contains('/')
                    || normalized.contains(':')
                    || normalized.parse::<IpAddr>().is_ok()
                {
                    return Err(PushError);
                }
                let parsed =
                    Url::parse(&format!("https://{normalized}/")).map_err(|_| PushError)?;
                if parsed.host_str() != Some(normalized.as_str()) {
                    return Err(PushError);
                }
                Ok(normalized)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if allowed_hosts.is_empty() {
            return Err(PushError);
        }
        crate::private_state::prepare_directory(state_dir).map_err(|_| PushError)?;
        let state_path = state_dir.join("push_state.json");
        let mut state = if state_path.exists() {
            crate::private_state::verify_private_file(&state_path).map_err(|_| PushError)?;
            serde_json::from_slice::<PushState>(
                &crate::private_state::read_bounded(&state_path, MAX_PUSH_STATE_BYTES)
                    .map_err(|_| PushError)?,
            )
            .map_err(|_| PushError)?
        } else {
            PushState {
                schema_version: PUSH_SCHEMA_VERSION,
                vapid_private_key: generate_vapid_private_key()?,
                instance_token_epoch: None,
                subscriptions: Vec::new(),
                recent_events: BTreeMap::new(),
            }
        };
        if state.schema_version != PUSH_SCHEMA_VERSION
            || state.subscriptions.len() > MAX_SUBSCRIPTIONS
            || state.recent_events.len() > MAX_DEDUPE_ENTRIES
        {
            return Err(PushError);
        }
        let public_key = public_key_from_private(&state.vapid_private_key)?;
        let instance_token_epoch =
            URL_SAFE_NO_PAD.encode(Sha256::digest(instance_token.as_bytes()));
        if state.instance_token_epoch.as_deref() != Some(instance_token_epoch.as_str()) {
            // A rotated shared credential revokes notification recipients as well as HTTP access.
            state.subscriptions.clear();
            state.instance_token_epoch = Some(instance_token_epoch);
        }
        prune_subscriptions(&mut state);
        persist_state(&state_path, &state)?;
        Ok(Self {
            state_path,
            allowed_hosts,
            vapid_subject,
            state: Arc::new(Mutex::new(state)),
            public_key,
        })
    }

    #[must_use]
    pub fn public_key(&self) -> String {
        self.public_key.clone()
    }

    /// Persist an authenticated browser subscription. Call from spawn_blocking.
    pub fn subscribe(&self, subscription: PushSubscription) -> Result<(), PushError> {
        subscription.validate(&self.allowed_hosts)?;
        mutate_state(&self.state, &self.state_path, move |state| {
            let now = now_seconds();
            prune_subscriptions(state);
            if let Some(existing) = state
                .subscriptions
                .iter_mut()
                .find(|entry| entry.subscription.endpoint == subscription.endpoint)
            {
                existing.subscription = subscription;
                existing.created_at = now;
                existing.consecutive_failures = 0;
                existing.quarantined = false;
            } else {
                if state.subscriptions.len() >= MAX_SUBSCRIPTIONS {
                    return Err(PushError);
                }
                state.subscriptions.push(StoredSubscription {
                    subscription,
                    created_at: now,
                    consecutive_failures: 0,
                    quarantined: false,
                });
            }
            Ok(((), true))
        })
    }

    /// Observe snapshots and emit at most one delivery attempt per persisted event key.
    pub async fn run(
        self: Arc<Self>,
        snapshot_state: crate::control_bridge::SnapshotState,
        mut stop: watch::Receiver<bool>,
    ) {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                changed = stop.changed() => if changed.is_err() || *stop.borrow() { break; },
                _ = interval.tick() => {
                    let snapshot = snapshot_state.read().await.clone();
                    if self.prune_stale_subscriptions().await.is_err() {
                        eprintln!("web_push_subscription_prune_failed");
                    }
                    if !snapshot.stale {
                        tokio::select! {
                            biased;
                            changed = stop.changed() => if changed.is_err() || *stop.borrow() { break; },
                            result = tokio::time::timeout(SWEEP_DEADLINE, self.observe_snapshot(&snapshot)) => {
                                match result {
                                    Ok(Ok(())) => {},
                                    Ok(Err(_)) => eprintln!("web_push_observation_failed"),
                                    Err(_) => eprintln!("web_push_sweep_deadline_exceeded"),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn observe_snapshot(
        &self,
        snapshot: &crate::control_bridge::BridgeSnapshot,
    ) -> Result<(), PushError> {
        self.observe_snapshot_with(snapshot, self).await
    }

    async fn observe_snapshot_with<D: EventDelivery + ?Sized>(
        &self,
        snapshot: &crate::control_bridge::BridgeSnapshot,
        delivery: &D,
    ) -> Result<(), PushError> {
        let Some(generation) = snapshot.generation.as_deref() else {
            return Ok(());
        };
        let mut candidates = Vec::new();
        if let Some(dsh) = &snapshot.dsh_status {
            candidates.push(("dsh", "session", dsh.phase.as_str(), dsh.records));
        }
        for task in snapshot.tasks.iter().take(256) {
            if candidates.len() >= MAX_CANDIDATES_PER_SNAPSHOT {
                break;
            }
            candidates.push((
                "task",
                task.task_id.as_str(),
                task.state.as_str(),
                task.latest_sequence,
            ));
            for run in task.runs.iter().take(MAX_CANDIDATES_PER_SNAPSHOT) {
                if candidates.len() >= MAX_CANDIDATES_PER_SNAPSHOT {
                    break;
                }
                candidates.push((
                    "run",
                    run.run_id.as_str(),
                    run.state.as_str(),
                    run.latest_sequence,
                ));
            }
            if let Some(linked) = &task.dispatch
                && let Some(sequence) = linked.sequence
            {
                candidates.push((
                    "dispatch",
                    linked.id.as_str(),
                    linked.state.as_str(),
                    sequence,
                ));
            }
            if let Some(linked) = &task.workflow
                && let Some(sequence) = linked.sequence
            {
                candidates.push((
                    "workflow",
                    linked.id.as_str(),
                    linked.state.as_str(),
                    sequence,
                ));
            }
        }
        for (kind, id, state_name, sequence) in candidates {
            let Some(event) = make_event(generation, kind, id, state_name, sequence) else {
                continue;
            };
            match self.claim_event_async(event.clone()).await? {
                EventDecision::Send => delivery.deliver(&event).await?,
                EventDecision::Baseline | EventDecision::Unchanged => {}
            }
        }
        Ok(())
    }

    async fn claim_event_async(&self, event: PushEvent) -> Result<EventDecision, PushError> {
        let state = Arc::clone(&self.state);
        let state_path = self.state_path.clone();
        tokio::task::spawn_blocking(move || claim_event(&state, &state_path, &event))
            .await
            .map_err(|_| PushError)?
    }

    async fn deliver_to_subscriptions(&self, event: &PushEvent) -> Result<(), PushError> {
        let endpoints = self
            .state
            .lock()
            .map_err(|_| PushError)?
            .subscriptions
            .iter()
            .filter(|entry| !entry.quarantined)
            .map(|entry| entry.subscription.endpoint.clone())
            .collect::<Vec<_>>();
        for endpoint in endpoints {
            let result = send_with_retries(
                &endpoint,
                &self.allowed_hosts,
                &self.state,
                &self.vapid_subject,
                event,
            )
            .await;
            match result {
                Ok(()) => {
                    self.record_delivery_success(endpoint.clone()).await?;
                }
                Err(disposition) => {
                    self.record_delivery_failure(endpoint.clone(), disposition)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn record_delivery_success(&self, endpoint: String) -> Result<(), PushError> {
        let state = Arc::clone(&self.state);
        let state_path = self.state_path.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&state, &state_path, |state| {
                let Some(entry) = state
                    .subscriptions
                    .iter_mut()
                    .find(|entry| entry.subscription.endpoint == endpoint)
                else {
                    return Ok(((), false));
                };
                let changed = entry.consecutive_failures != 0;
                entry.consecutive_failures = 0;
                Ok(((), changed))
            })
        })
        .await
        .map_err(|_| PushError)?
    }

    async fn prune_stale_subscriptions(&self) -> Result<(), PushError> {
        let state = Arc::clone(&self.state);
        let state_path = self.state_path.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&state, &state_path, |state| {
                let previous_count = state.subscriptions.len();
                prune_subscriptions(state);
                Ok(((), state.subscriptions.len() != previous_count))
            })
        })
        .await
        .map_err(|_| PushError)?
    }

    async fn record_delivery_failure(
        &self,
        endpoint: String,
        disposition: DeliveryFailure,
    ) -> Result<(), PushError> {
        let state = Arc::clone(&self.state);
        let state_path = self.state_path.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&state, &state_path, |state| match disposition {
                DeliveryFailure::Expired => {
                    let previous_count = state.subscriptions.len();
                    state
                        .subscriptions
                        .retain(|entry| entry.subscription.endpoint != endpoint);
                    Ok(((), state.subscriptions.len() != previous_count))
                }
                DeliveryFailure::Permanent => {
                    if let Some(entry) = state
                        .subscriptions
                        .iter_mut()
                        .find(|entry| entry.subscription.endpoint == endpoint)
                    {
                        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
                        if entry.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            entry.quarantined = true;
                        }
                        Ok(((), true))
                    } else {
                        Ok(((), false))
                    }
                }
            })
        })
        .await
        .map_err(|_| PushError)?
    }
}

trait EventDelivery {
    fn deliver<'a>(
        &'a self,
        event: &'a PushEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), PushError>> + Send + 'a>>;
}

impl EventDelivery for PushService {
    fn deliver<'a>(
        &'a self,
        event: &'a PushEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), PushError>> + Send + 'a>> {
        Box::pin(self.deliver_to_subscriptions(event))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventDecision {
    Baseline,
    Unchanged,
    Send,
}

fn claim_event(
    state: &Mutex<PushState>,
    state_path: &Path,
    event: &PushEvent,
) -> Result<EventDecision, PushError> {
    mutate_state(state, state_path, |state| {
        let entity_key = format!("{}:{}", event.entity_kind, event.entity_id);
        let decision = match state.recent_events.get(&entity_key) {
            None => EventDecision::Baseline,
            Some(previous) if previous.generation != event.generation => {
                if previous.state == event.state {
                    EventDecision::Baseline
                } else {
                    EventDecision::Send
                }
            }
            Some(previous) if event.control_sequence <= previous.sequence => {
                EventDecision::Unchanged
            }
            Some(previous) if previous.state == event.state => EventDecision::Unchanged,
            Some(_) => EventDecision::Send,
        };
        if matches!(decision, EventDecision::Unchanged) {
            if let Some(previous) = state.recent_events.get_mut(&entity_key)
                && previous.generation == event.generation
                && event.control_sequence > previous.sequence
                && previous.state == event.state
            {
                previous.sequence = event.control_sequence;
                return Ok((decision, true));
            }
            return Ok((decision, false));
        }
        state.recent_events.insert(
            entity_key,
            EventVersion {
                generation: event.generation.clone(),
                sequence: event.control_sequence,
                state: event.state.clone(),
            },
        );
        // Persist the claim before POST. A crash after this point can lose one
        // notification; bounded retries happen only within this observation call.
        while state.recent_events.len() > MAX_DEDUPE_ENTRIES {
            if let Some(first) = state.recent_events.keys().next().cloned() {
                state.recent_events.remove(&first);
            }
        }
        Ok((decision, true))
    })
}

#[derive(Clone)]
struct PushEvent {
    generation: String,
    entity_kind: String,
    entity_id: String,
    state: String,
    control_sequence: u64,
}

#[derive(Serialize)]
struct PushPayload<'a> {
    kind: &'a str,
    id: &'a str,
    state: &'a str,
}

impl<'a> From<&'a PushEvent> for PushPayload<'a> {
    fn from(event: &'a PushEvent) -> Self {
        Self {
            kind: &event.entity_kind,
            id: &event.entity_id,
            state: &event.state,
        }
    }
}

fn make_event(
    generation: &str,
    kind: &str,
    id: &str,
    state: &str,
    sequence: u64,
) -> Option<PushEvent> {
    if generation.is_empty()
        || generation.len() > 64
        || id.is_empty()
        || id.len() > 64
        || !generation.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !is_known_state(kind, state)
    {
        return None;
    }
    Some(PushEvent {
        generation: generation.to_owned(),
        entity_kind: kind.to_owned(),
        entity_id: id.to_owned(),
        state: state.to_owned(),
        control_sequence: sequence,
    })
}

fn is_known_state(kind: &str, state: &str) -> bool {
    match kind {
        "task" => matches!(
            state,
            "open" | "in_progress" | "blocked" | "done" | "cancelled"
        ),
        "run" => matches!(
            state,
            "preparing" | "running" | "stopped" | "failed" | "succeeded" | "stale"
        ),
        "dispatch" => matches!(
            state,
            "admitted"
                | "running"
                | "cancel_requested"
                | "recovery_pending"
                | "completed"
                | "failed"
                | "unverified"
                | "cancelled"
        ),
        "workflow" => matches!(
            state,
            "prepared"
                | "running"
                | "paused"
                | "integrating"
                | "accepted"
                | "failed"
                | "cancel_requested"
                | "cancelled"
                | "blocked"
        ),
        "dsh" => matches!(
            state,
            "starting" | "initializing" | "ready" | "prompting" | "closing" | "closed" | "failed"
        ),
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum DeliveryFailure {
    Expired,
    Permanent,
}

async fn send_with_retries(
    endpoint: &str,
    allowed_hosts: &BTreeSet<String>,
    state_lock: &Mutex<PushState>,
    subject: &str,
    event: &PushEvent,
) -> Result<(), DeliveryFailure> {
    let delivery = async {
        for retry_delay in RETRY_DELAYS
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
        {
            match send_one(endpoint, allowed_hosts, state_lock, subject, event).await {
                Ok(()) => return Ok(()),
                Err(DeliveryFailure::Expired) => return Err(DeliveryFailure::Expired),
                Err(DeliveryFailure::Permanent) => match retry_delay {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => return Err(DeliveryFailure::Permanent),
                },
            }
        }
        Err(DeliveryFailure::Permanent)
    };
    match tokio::time::timeout(REQUEST_DEADLINE, delivery).await {
        Ok(result) => result,
        Err(_) => Err(DeliveryFailure::Permanent),
    }
}

async fn send_one(
    endpoint: &str,
    allowed_hosts: &BTreeSet<String>,
    state_lock: &Mutex<PushState>,
    subject: &str,
    event: &PushEvent,
) -> Result<(), DeliveryFailure> {
    let subscription = {
        let state = state_lock.lock().map_err(|_| DeliveryFailure::Permanent)?;
        state
            .subscriptions
            .iter()
            .find(|entry| entry.subscription.endpoint == endpoint)
            .map(|entry| entry.subscription.clone())
            .ok_or(DeliveryFailure::Permanent)?
    };
    subscription
        .validate(allowed_hosts)
        .map_err(|_| DeliveryFailure::Permanent)?;
    let parsed = Url::parse(endpoint).map_err(|_| DeliveryFailure::Permanent)?;
    let host = parsed
        .host_str()
        .ok_or(DeliveryFailure::Permanent)?
        .to_ascii_lowercase();
    let mut addresses = Vec::new();
    for address in tokio::net::lookup_host((host.as_str(), 443))
        .await
        .map_err(|_| DeliveryFailure::Permanent)?
    {
        if addresses.len() == 16 {
            return Err(DeliveryFailure::Permanent);
        }
        addresses.push(address);
    }
    if !are_public_addresses(&addresses) {
        return Err(DeliveryFailure::Permanent);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_DEADLINE)
        .timeout(REQUEST_DEADLINE)
        .resolve_to_addrs(&host, &addresses)
        .build()
        .map_err(|_| DeliveryFailure::Permanent)?;
    let subscription_info = SubscriptionInfo {
        endpoint: endpoint.to_owned(),
        keys: SubscriptionKeys {
            p256dh: subscription.keys.p256dh,
            auth: subscription.keys.auth,
        },
    };
    let private_key = {
        let state = state_lock.lock().map_err(|_| DeliveryFailure::Permanent)?;
        state.vapid_private_key.clone()
    };
    let mut vapid = VapidSignatureBuilder::from_base64_no_sub(&private_key)
        .map_err(|_| DeliveryFailure::Permanent)?
        .add_sub_info(&subscription_info);
    vapid.add_claim("sub", subject.to_owned());
    let signature = vapid.build().map_err(|_| DeliveryFailure::Permanent)?;
    // Generation and control sequence are local dedupe keys, never notification content.
    let payload =
        serde_json::to_vec(&PushPayload::from(event)).map_err(|_| DeliveryFailure::Permanent)?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(DeliveryFailure::Permanent);
    }
    let mut message = WebPushMessageBuilder::new(&subscription_info);
    message.set_ttl(300);
    message.set_urgency(web_push::Urgency::Low);
    message.set_payload(ContentEncoding::Aes128Gcm, &payload);
    message.set_vapid_signature(signature);
    let built = build_request::<Vec<u8>>(message.build().map_err(|_| DeliveryFailure::Permanent)?);
    let mut request = client.post(endpoint);
    for (name, value) in built.headers() {
        let value = value.to_str().map_err(|_| DeliveryFailure::Permanent)?;
        request = request.header(name.as_str(), value);
    }
    let body = built.body().clone();
    let response = request
        .body(body)
        .send()
        .await
        .map_err(|_| DeliveryFailure::Permanent)?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
        return Err(DeliveryFailure::Expired);
    }
    if !response_length_allowed(response.content_length()) {
        return Err(DeliveryFailure::Permanent);
    }
    let mut stream = response.bytes_stream();
    let mut received = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| DeliveryFailure::Permanent)?;
        received =
            bounded_response_size(received, chunk.len()).ok_or(DeliveryFailure::Permanent)?;
    }
    classify_response(status.as_u16())
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_multicast()
                || address == std::net::Ipv4Addr::BROADCAST
                || octets[0] == 0
                || octets[0] >= 224
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && matches!(octets[2], 0 | 2))
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(mapped));
            }
            if segments[0] == 0x2002 {
                let embedded = std::net::Ipv4Addr::new(
                    (segments[1] >> 8) as u8,
                    segments[1] as u8,
                    (segments[2] >> 8) as u8,
                    segments[2] as u8,
                );
                return is_public_address(IpAddr::V4(embedded));
            }
            let special_2001 = segments[0] == 0x2001 && segments[1] <= 0x01ff;
            let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
            let documentation_3fff = segments[0] == 0x3fff && (segments[1] & 0xfff0) == 0;
            (segments[0] & 0xe000) == 0x2000
                && !special_2001
                && !documentation
                && !documentation_3fff
        }
    }
}

fn are_public_addresses(addresses: &[std::net::SocketAddr]) -> bool {
    !addresses.is_empty()
        && addresses.len() <= 16
        && addresses
            .iter()
            .all(|address| is_public_address(address.ip()))
}

fn classify_response(status: u16) -> Result<(), DeliveryFailure> {
    match status {
        200..=299 => Ok(()),
        404 | 410 => Err(DeliveryFailure::Expired),
        _ => Err(DeliveryFailure::Permanent),
    }
}

fn response_length_allowed(content_length: Option<u64>) -> bool {
    content_length.is_none_or(|length| length <= MAX_RESPONSE_BYTES as u64)
}

fn bounded_response_size(current: usize, next_chunk: usize) -> Option<usize> {
    current
        .checked_add(next_chunk)
        .filter(|total| *total <= MAX_RESPONSE_BYTES)
}

fn valid_base64url(value: &str, expected_bytes: usize) -> bool {
    URL_SAFE_NO_PAD.decode(value).is_ok_and(|bytes| {
        bytes.len() == expected_bytes && (expected_bytes != 65 || bytes.first() == Some(&4))
    })
}

fn valid_vapid_subject(subject: &str) -> bool {
    if subject.len() > 256 {
        return false;
    }
    if let Some(address) = subject.strip_prefix("mailto:") {
        return !address.is_empty()
            && address.contains('@')
            && !address.chars().any(char::is_whitespace);
    }
    let Ok(url) = Url::parse(subject) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn generate_vapid_private_key() -> Result<String, PushError> {
    for _ in 0..MAX_KEY_GENERATION_ATTEMPTS {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| PushError)?;
        let candidate = URL_SAFE_NO_PAD.encode(bytes);
        if VapidSignatureBuilder::from_base64_no_sub(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(PushError)
}

fn public_key_from_private(private_key: &str) -> Result<String, PushError> {
    let builder = VapidSignatureBuilder::from_base64_no_sub(private_key).map_err(|_| PushError)?;
    Ok(URL_SAFE_NO_PAD.encode(builder.get_public_key()))
}

fn prune_subscriptions(state: &mut PushState) {
    let now = now_seconds();
    state.subscriptions.retain(|entry| {
        entry.created_at <= now
            && now.saturating_sub(entry.created_at) <= PRUNE_AGE.as_secs()
            && entry
                .subscription
                .expiration_time
                .is_none_or(|expiry| expiry > now_millis())
    });
}

fn persist_state(path: &Path, state: &PushState) -> Result<(), PushError> {
    let bytes = serde_json::to_vec(state).map_err(|_| PushError)?;
    if bytes.len() > MAX_PUSH_STATE_BYTES {
        return Err(PushError);
    }
    crate::private_state::atomic_save(path, &bytes).map_err(|_| PushError)
}

fn mutate_state<T>(
    state: &Mutex<PushState>,
    state_path: &Path,
    update: impl FnOnce(&mut PushState) -> Result<(T, bool), PushError>,
) -> Result<T, PushError> {
    let mut current = state.lock().map_err(|_| PushError)?;
    let mut candidate = current.clone();
    let (result, changed) = update(&mut candidate)?;
    if changed {
        persist_state(state_path, &candidate)?;
        *current = candidate;
    }
    Ok(result)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_millis() -> u64 {
    now_seconds().saturating_mul(1000)
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
