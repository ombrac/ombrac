use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_channel::{Receiver, Sender};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use dashmap::{DashMap, Entry};
use ombrac_macros::{debug, error};
use quinn::Connection;

pub struct Datagram(Sender<Bytes>, Receiver<Bytes>);

type SessionId = u32;

enum SessionState {
    Pending(Sender<Bytes>, Receiver<Bytes>),
    Active(Sender<Bytes>),
}

pub struct Session {
    conn: Connection,
    next_session_id: AtomicU32,
    notify: Receiver<SessionId>,
    is_connected: Arc<AtomicBool>,
    sessions: Arc<DashMap<SessionId, SessionState>>,
}

impl Session {
    const DEFAULT_BUFFER_SIZE: usize = 32;
    const SESSION_ID_LENGTH: usize = 4;

    pub fn with_client(conn: Connection) -> Self {
        Self::with_config(conn, 0)
    }

    pub fn with_server(conn: Connection) -> Self {
        Self::with_config(conn, 1)
    }

    fn with_config(conn: Connection, init_session_id: SessionId) -> Self {
        use async_channel::{bounded, unbounded};

        let sessions = Arc::new(DashMap::new());
        let conn_recv = conn.clone();
        let sessions_recv = sessions.clone();
        let is_connected = Arc::new(AtomicBool::new(true));
        let is_connected_clone = is_connected.clone();

        let (notify_sender, notify) = unbounded();

        tokio::spawn(async move {
            let notify_sender = notify_sender.clone();

            while let Ok(datagram) = conn_recv.read_datagram().await {
                let sessions = sessions_recv.clone();
                let notify_sender = notify_sender.clone();

                tokio::spawn(async move {
                    if datagram.len() < Session::SESSION_ID_LENGTH {
                        error!("Received datagram too short: {} bytes", datagram.len());
                        return;
                    }

                    let mut cursor = Buf::take(&datagram[..], Session::SESSION_ID_LENGTH);
                    let session_id = cursor.get_u32();
                    let payload = datagram.slice(Session::SESSION_ID_LENGTH..);

                    match sessions.entry(session_id) {
                        Entry::Occupied(entry) => match entry.get() {
                            SessionState::Pending(sender, ..) | SessionState::Active(sender) => {
                                if sender.send(payload).await.is_err() {
                                    entry.remove();
                                    error!("Failed to forward datagram to session {}", session_id);
                                }
                            }
                        },
                        Entry::Vacant(entry) => {
                            let (sender, receiver) = bounded(Self::DEFAULT_BUFFER_SIZE);

                            if sender.send(payload).await.is_err() {
                                error!(
                                    "Failed to send first datagram to new session {}",
                                    session_id
                                );
                                return;
                            }

                            entry.insert(SessionState::Pending(sender, receiver));

                            if notify_sender.send(session_id).await.is_err() {
                                error!("Failed to notify about new session {}", session_id);
                                sessions.remove(&session_id);
                            }
                        }
                    }
                });
            }

            // Connection closed
            notify_sender.close();
            is_connected_clone.store(false, Ordering::Release);
        });

        Self {
            conn,
            sessions,
            notify,
            is_connected,
            next_session_id: AtomicU32::new(init_session_id),
        }
    }

    #[inline]
    fn spawn_datagram_sender(
        conn: Connection,
        session_id: SessionId,
        sessions: Arc<DashMap<SessionId, SessionState>>,
    ) -> Sender<Bytes> {
        use async_channel::bounded;

        let (sender, receiver) = bounded::<Bytes>(Self::DEFAULT_BUFFER_SIZE);

        tokio::spawn(async move {
            let mut buf = BytesMut::with_capacity(4096);

            while let Ok(data) = receiver.recv().await {
                buf.clear();
                buf.put_u32(session_id);
                buf.extend_from_slice(&data);

                if let Err(_error) = conn.send_datagram(buf.split().freeze()) {
                    error!(
                        "Failed to send datagram for session {}: {}, length: {}",
                        session_id,
                        _error,
                        data.len()
                    );
                    break;
                }
            }

            // Sender closed
            if sessions.remove(&session_id).is_some() {
                debug!("Session {} removed from sessions map", session_id);
            }
        });

        sender
    }

    #[inline]
    fn open(&self, session_id: SessionId) -> Datagram {
        use async_channel::bounded;

        let conn = self.conn.clone();
        let (forwarder, receiver) = bounded(Self::DEFAULT_BUFFER_SIZE);

        let sender = Self::spawn_datagram_sender(conn, session_id, self.sessions.clone());

        self.sessions
            .insert(session_id, SessionState::Active(forwarder.clone()));

        Datagram(sender, receiver)
    }

    #[inline(always)]
    fn next_session_id(&self) -> SessionId {
        self.next_session_id.fetch_add(2, Ordering::Relaxed)
    }

    pub async fn open_datagram(&self) -> Option<Datagram> {
        if self.is_connected.load(Ordering::Acquire) {
            return Some(self.open(self.next_session_id()));
        }

        None
    }

    pub async fn accept_datagram(&self) -> Option<Datagram> {
        use async_channel::bounded;
        use std::mem::replace;

        if let Ok(session_id) = self.notify.recv().await {
            // ```text
            // +---------+       +---------+
            // | Pending | ----> | Active  |
            // +---------+       +---------+
            // ```
            if let Some(mut entry) = self.sessions.get_mut(&session_id) {
                match replace(&mut *entry, SessionState::Active(bounded(1).0)) {
                    SessionState::Pending(sender, receiver) => {
                        *entry = SessionState::Active(sender.clone());

                        let user_sender = Self::spawn_datagram_sender(
                            self.conn.clone(),
                            session_id,
                            self.sessions.clone(),
                        );

                        return Some(Datagram(user_sender, receiver));
                    }
                    state => {
                        *entry = state;
                        error!("Unexpected session state for {}", session_id);
                    }
                }
            }

            error!("Session {} notified but not found in map", session_id);
            unreachable!()
        };

        None // Notify channel closed
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.sessions.clear();
    }
}

impl crate::Unreliable for Datagram {
    #[inline]
    async fn recv(&self) -> io::Result<Bytes> {
        self.1
            .recv()
            .await
            .map_err(|e| io::Error::other(e.to_string()))
    }

    #[inline]
    async fn send(&self, data: Bytes) -> io::Result<()> {
        self.0
            .send(data)
            .await
            .map_err(|e| io::Error::other(e.to_string()))
    }
}
