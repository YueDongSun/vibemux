//! Lease loss reaches the execution owner independently of the writer or caller.
//! This owner always joins execution; interruption never implies persisted finish.

use std::future::Future;
use tokio::sync::watch;

pub(super) async fn join_with_lease_loss<T>(
    mut lease_loss: watch::Receiver<bool>,
    cancellation: watch::Sender<bool>,
    execution: impl Future<Output = T>,
) -> T {
    let mut lost = *lease_loss.borrow() || lease_loss.has_changed().is_err();
    if lost {
        cancellation.send_replace(true);
    }
    tokio::pin!(execution);
    loop {
        tokio::select! {
            biased;
            changed = lease_loss.changed(), if !lost => {
                if changed.is_err() || *lease_loss.borrow() {
                    lost = true;
                    cancellation.send_replace(true);
                }
            }
            result = &mut execution => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::{sync::oneshot, time::timeout};

    const TEST_DEADLINE: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn loss_before_registration_is_seen_before_execution_starts() {
        let (lease, lease_loss) = watch::channel(false);
        lease.send_replace(true);
        let (cancellation, cancelled) = watch::channel(false);
        join_with_lease_loss(lease_loss, cancellation, async {
            assert!(
                *cancelled.borrow(),
                "launch started before observing lost lease"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn lost_renewal_owner_cancels_without_a_durable_writer_call() {
        let (lease, lease_loss) = watch::channel(false);
        drop(lease);
        let (cancellation, cancelled) = watch::channel(false);
        join_with_lease_loss(lease_loss, cancellation, async {
            assert!(
                *cancelled.borrow(),
                "closed owner was mistaken for a valid lease"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn loss_while_admission_is_pending_stays_latched_and_cleanup_is_joined() {
        let (lease, lease_loss) = watch::channel(false);
        let (cancellation, mut cancelled) = watch::channel(false);
        let (release, admitted) = oneshot::channel();
        let (cleanup, cleaned) = oneshot::channel();
        let admission_cancelled = cancelled.clone();
        let owner = tokio::spawn(join_with_lease_loss(lease_loss, cancellation, async move {
            admitted.await.expect("admission barrier");
            assert!(
                *admission_cancelled.borrow(),
                "late admission lost cancellation"
            );
            cleaned.await.expect("cleanup barrier");
            "joined"
        }));
        lease.send_replace(true);
        timeout(TEST_DEADLINE, cancelled.changed())
            .await
            .expect("prompt interruption")
            .expect("signal");
        // A later successful notification cannot revive an interrupted attempt.
        lease.send_replace(false);
        release.send(()).expect("release admission");
        tokio::task::yield_now().await;
        assert!(!owner.is_finished(), "execution owner abandoned cleanup");
        cleanup.send(()).expect("cleanup");
        assert_eq!(
            timeout(TEST_DEADLINE, owner)
                .await
                .expect("join")
                .expect("owner"),
            "joined"
        );
        assert!(*cancelled.borrow(), "loss was reset");
    }

    #[tokio::test]
    async fn healthy_completion_does_not_cancel_a_turn() {
        let (_lease, lease_loss) = watch::channel(false);
        let (cancellation, cancelled) = watch::channel(false);
        assert_eq!(
            join_with_lease_loss(lease_loss, cancellation, async { 7 }).await,
            7
        );
        assert!(!*cancelled.borrow());
    }
}
