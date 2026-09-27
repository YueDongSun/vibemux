//! Bounded daemon-owned request handles. No arbitrary plugin method dispatch.

use crate::plugin_registry::{PluginState, PluginStatus};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, timeout};
use vibemux_plugin_protocol::terminal::{
    FOCUS_METHOD, INVENTORY_METHOD, MAX_TERMINAL_PAYLOAD_BYTES,
};

pub const REQUEST_CAPACITY: usize = 8;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

pub(crate) struct PluginRequest {
    pub session_id: String,
    pub method: String,
    pub payload: Vec<u8>,
    pub deadline: Instant,
    pub response: oneshot::Sender<Result<Vec<u8>, &'static str>>,
}

#[derive(Clone)]
pub(crate) struct RequestRoute {
    pub requests: mpsc::Sender<PluginRequest>,
    pub status: watch::Receiver<PluginStatus>,
}

#[derive(Clone)]
pub struct PluginRequestClient {
    pub(crate) routes: watch::Receiver<BTreeMap<String, RequestRoute>>,
}

pub struct PluginReply {
    pub session_id: String,
    pub payload: Vec<u8>,
}

impl PluginRequestClient {
    pub fn active_session(&self, plugin_id: &str) -> Option<String> {
        let routes = self.routes.borrow();
        let status = routes.get(plugin_id)?.status.borrow();
        (status.state == PluginState::Active)
            .then(|| status.session_id.clone())
            .flatten()
    }

    pub async fn request(
        &self,
        plugin_id: &str,
        method: &str,
        payload: Vec<u8>,
    ) -> Result<PluginReply, &'static str> {
        if !matches!(method, INVENTORY_METHOD | FOCUS_METHOD)
            || payload.len() > MAX_TERMINAL_PAYLOAD_BYTES
        {
            return Err("terminal_invalid_request");
        }
        let route = self
            .routes
            .borrow()
            .get(plugin_id)
            .cloned()
            .ok_or("terminal_plugin_unavailable")?;
        let session_id = self
            .active_session(plugin_id)
            .ok_or("terminal_plugin_unavailable")?;
        let (response, result) = oneshot::channel();
        route
            .requests
            .try_send(PluginRequest {
                session_id: session_id.clone(),
                method: method.to_owned(),
                payload,
                deadline: Instant::now() + REQUEST_TIMEOUT,
                response,
            })
            .map_err(|_| "terminal_queue_unavailable")?;
        let payload = timeout(REQUEST_TIMEOUT + Duration::from_millis(100), result)
            .await
            .map_err(|_| "terminal_request_timeout")?
            .map_err(|_| "terminal_plugin_unavailable")??;
        if self.active_session(plugin_id).as_deref() != Some(&session_id) {
            return Err("terminal_session_changed");
        }
        Ok(PluginReply {
            session_id,
            payload,
        })
    }
}
