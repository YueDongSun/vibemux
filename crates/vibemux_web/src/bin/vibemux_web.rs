#![forbid(unsafe_code)]
//! Single operator-started process; no daemon autostart or canonical writes.
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, watch};
use vibemux_web::{
    config::WebConfig,
    control_bridge::{BridgePoller, BridgeSnapshot},
    private_state::InstanceLease,
    push::PushService,
    server::build_router,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let prepared = tokio::task::spawn_blocking(
        move || -> Result<_, Box<dyn std::error::Error + Send + Sync>> {
            let project_root = std::fs::canonicalize(project_root)?;
            let Some(config) = WebConfig::load(&project_root)? else {
                return Ok(None);
            };
            let instance_token = config.instance_token.resolve()?;
            let paths = vibemuxd::process::DaemonPaths::from_project_root(&project_root)?;
            let lease = InstanceLease::acquire(&project_root)?;
            let push = Arc::new(PushService::startup(
                &lease.state_dir,
                config.push_allowed_hosts.clone(),
                config.vapid_subject.clone(),
                &instance_token,
            )?);
            Ok(Some((config, instance_token, paths, lease, push)))
        },
    )
    .await?
    .map_err(|_| "web_startup_failed")?;
    let Some((config, instance_token, paths, _lease, push)) = prepared else {
        eprintln!("web_disabled");
        return Ok(());
    };
    let snapshot_state = Arc::new(RwLock::new(BridgeSnapshot::default()));
    let listener = tokio::net::TcpListener::bind(config.bind_address)
        .await
        .map_err(|_| "web_loopback_bind_failed")?;
    let poller = BridgePoller::new(paths, Duration::from_secs(config.poll_seconds))
        .with_task_links(config.task_links)?;
    let router = build_router(
        snapshot_state.clone(),
        push.clone(),
        instance_token,
        config.enrollment_url,
    );
    let (stop_tx, stop_rx) = watch::channel(false);
    let bridge_task = tokio::spawn(poller.run(snapshot_state.clone(), stop_rx.clone()));
    let push_task = tokio::spawn(push.run(snapshot_state, stop_rx.clone()));
    let mut http_stop = stop_rx;
    let mut http_task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                while !*http_stop.borrow() {
                    if http_stop.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await
    });
    let mut http_finished = false;
    let outcome = tokio::select! {
        result=tokio::signal::ctrl_c()=>result.map_err(|_|"web_shutdown_signal_failed"),
        result=&mut http_task=>{http_finished=true;match result {Ok(Ok(()))=>Ok(()),_=>Err("web_http_failed")}},
    };
    stop_tx.send_replace(true);
    let joined = tokio::time::timeout(Duration::from_secs(45), async {
        bridge_task.await.map_err(|_| "web_bridge_failed")?;
        push_task.await.map_err(|_| "web_push_failed")?;
        if !http_finished {
            http_task
                .await
                .map_err(|_| "web_http_failed")?
                .map_err(|_| "web_http_failed")?;
        }
        Ok::<_, &str>(())
    })
    .await
    .map_err(|_| "web_shutdown_deadline")?;
    joined?;
    outcome?;
    Ok(())
}
