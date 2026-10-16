//! In-process pub/sub hub for live game events.
//!
//! Connections subscribe to the games they follow; the transactional outbox
//! dispatcher (and presence changes) publish `ServerMessage`s here. Senders
//! are unbounded channels so publishing never blocks a database transaction;
//! slow consumers are dropped only if their task is gone.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use protocol::ServerMessage;
use tokio::sync::mpsc;
use uuid::Uuid;

type Subscribers = HashMap<Uuid, HashMap<Uuid, mpsc::UnboundedSender<ServerMessage>>>;

#[derive(Clone, Default)]
pub struct Hub {
    inner: Arc<RwLock<Subscribers>>,
}

impl Hub {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Subscribers> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Subscribers> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Register `conn_id` as a subscriber of `game_id`.
    pub fn subscribe(
        &self,
        game_id: Uuid,
        conn_id: Uuid,
        tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        self.write().entry(game_id).or_default().insert(conn_id, tx);
    }

    /// Drop one connection's subscription to one game.
    pub fn unsubscribe(&self, game_id: Uuid, conn_id: Uuid) {
        let mut map = self.write();
        if let Some(subs) = map.get_mut(&game_id) {
            subs.remove(&conn_id);
            if subs.is_empty() {
                map.remove(&game_id);
            }
        }
    }

    /// Drop a connection from every game and return the games it was in, so
    /// the caller can announce its departure.
    pub fn unsubscribe_conn(&self, conn_id: Uuid) -> Vec<Uuid> {
        let mut map = self.write();
        let mut left = Vec::new();
        map.retain(|game_id, subs| {
            if subs.remove(&conn_id).is_some() {
                left.push(*game_id);
            }
            !subs.is_empty()
        });
        left
    }

    /// Fan a message out to every live subscriber of `game_id`.
    /// Dead senders are pruned as they are discovered.
    pub fn publish(&self, game_id: Uuid, msg: &ServerMessage) {
        let mut map = self.write();
        if let Some(subs) = map.get_mut(&game_id) {
            subs.retain(|_, tx| tx.send(msg.clone()).is_ok());
            if subs.is_empty() {
                map.remove(&game_id);
            }
        }
    }

    pub fn subscriber_count(&self, game_id: Uuid) -> usize {
        self.read().get(&game_id).map_or(0, |s| s.len())
    }

    /// Games with at least one subscriber (used by tests/metrics).
    pub fn watched_games(&self) -> HashSet<Uuid> {
        self.read().keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ErrorCode;

    fn error_msg() -> ServerMessage {
        ServerMessage::Error {
            command_id: None,
            code: ErrorCode::Internal,
            message: "x".into(),
        }
    }

    #[test]
    fn publish_reaches_only_subscribers_and_prunes_dead_ones() {
        let hub = Hub::default();
        let game = Uuid::new_v4();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, rx2) = mpsc::unbounded_channel();
        hub.subscribe(game, Uuid::new_v4(), tx1);
        hub.subscribe(game, Uuid::new_v4(), tx2);
        assert_eq!(hub.subscriber_count(game), 2);

        hub.publish(game, &error_msg());
        assert!(matches!(rx1.try_recv(), Ok(ServerMessage::Error { .. })));
        drop(rx2); // receiver gone → send fails → pruned

        hub.publish(game, &error_msg());
        assert_eq!(hub.subscriber_count(game), 1);
        assert!(matches!(rx1.try_recv(), Ok(ServerMessage::Error { .. })));

        let left = hub.unsubscribe_conn(Uuid::new_v4());
        assert!(left.is_empty());
        assert_eq!(hub.watched_games(), HashSet::from([game]));

        // Unsubscribing a real connection reports the games it left.
        let conn = Uuid::new_v4();
        let (tx3, _rx3) = mpsc::unbounded_channel();
        hub.subscribe(game, conn, tx3);
        assert_eq!(hub.subscriber_count(game), 2);
        assert_eq!(hub.unsubscribe_conn(conn), vec![game]);
        // The anonymous subscriber from the top of the test is still there.
        assert_eq!(hub.subscriber_count(game), 1);
    }
}
