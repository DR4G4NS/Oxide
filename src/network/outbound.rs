//! Narrow outbound delivery port.
//!
//! Domain logic emits completed 158.1 frames through [`FrameEmit`]. It never
//! holds a connection registry or talks to sockets. The registry-backed
//! implementation lives here so `wire` does not depend on `listener`, and the
//! tick/session coordinators bind the sink after domain mutation.

use dashmap::DashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::warn;

use crate::network::protocol::packet_unreliable;
use crate::network::world::PendingConnection;

/// Domain-facing delivery sink. Adapter code implements this for the live
/// connection registry; tests use [`NOOP`].
pub trait FrameEmit {
    fn broadcast(&self, frame: Vec<u8>);
    fn enqueue_to(&self, connection_id: i32, frame: Vec<u8>, critical: bool) -> bool;
    fn for_each_connection(&self, visit: &mut dyn FnMut(i32));
    fn connection_ip(&self, connection_id: i32) -> String {
        let _ = connection_id;
        String::new()
    }
}

/// Test/helper sink that drops every frame.
pub struct NoopEmit;

pub const NOOP: NoopEmit = NoopEmit;

impl FrameEmit for NoopEmit {
    fn broadcast(&self, _frame: Vec<u8>) {}
    fn enqueue_to(&self, _connection_id: i32, _frame: Vec<u8>, _critical: bool) -> bool {
        true
    }
    fn for_each_connection(&self, _visit: &mut dyn FnMut(i32)) {}
}

impl FrameEmit for DashMap<i32, PendingConnection> {
    fn broadcast(&self, frame: Vec<u8>) {
        broadcast(self, frame);
    }
    fn enqueue_to(&self, connection_id: i32, frame: Vec<u8>, critical: bool) -> bool {
        self.get(&connection_id)
            .map(|connection| enqueue_outbound(connection.value(), frame, critical))
            .unwrap_or(false)
    }
    fn for_each_connection(&self, visit: &mut dyn FnMut(i32)) {
        for entry in self.iter() {
            visit(*entry.key());
        }
    }
    fn connection_ip(&self, connection_id: i32) -> String {
        self.get(&connection_id)
            .map(|connection| connection.ip.to_string())
            .unwrap_or_default()
    }
}

impl FrameEmit for Arc<DashMap<i32, PendingConnection>> {
    fn broadcast(&self, frame: Vec<u8>) {
        self.as_ref().broadcast(frame);
    }
    fn enqueue_to(&self, connection_id: i32, frame: Vec<u8>, critical: bool) -> bool {
        self.as_ref().enqueue_to(connection_id, frame, critical)
    }
    fn for_each_connection(&self, visit: &mut dyn FnMut(i32)) {
        self.as_ref().for_each_connection(visit);
    }
    fn connection_ip(&self, connection_id: i32) -> String {
        self.as_ref().connection_ip(connection_id)
    }
}

/// TCP-prefixed generated frames store the PacketSerializer body after the
/// u16 length. Framework keepalives and synthetic test frames do not match.
fn generated_tcp_body(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 3 {
        return None;
    }
    let prefix = u16::from_be_bytes([frame[0], frame[1]]) as usize;
    if prefix.checked_add(2) != Some(frame.len()) {
        return None;
    }
    Some(&frame[2..])
}

/// P0-9/P0-12: enqueue one outbound frame. Unreliable Call packets go UDP
/// (serializer body, no length prefix) when the client finished RegisterUDP
/// unless `force_tcp`. Missing endpoint or a failed `try_send_to` falls back
/// to the TCP queue — never silently dropped.
pub(crate) fn enqueue_outbound_routed(
    connection: &PendingConnection,
    frame: Vec<u8>,
    critical: bool,
    force_tcp: bool,
) -> bool {
    if !force_tcp {
        if let Some(body) = generated_tcp_body(&frame) {
            let packet_id = body[0];
            if packet_unreliable(packet_id) {
                if let (Some(endpoint), Some(socket)) = (
                    *connection.udp_endpoint.read(),
                    connection.udp_socket.as_ref(),
                ) {
                    if socket.send_to(body, endpoint).is_ok() {
                        return true;
                    }
                }
            }
        }
    }
    match connection.outbound.try_send(frame) {
        Ok(()) => {
            connection.outbound_queued.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            connection.outbound_drops.fetch_add(1, Ordering::Relaxed);
            if critical {
                connection.critical_drops.fetch_add(1, Ordering::Relaxed);
                warn!(
                    "dropped critical outbound frame for {} (queue full, {} total drops)",
                    connection.ip,
                    connection.outbound_drops.load(Ordering::Relaxed)
                );
            }
            false
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
    }
}

pub(crate) fn broadcast(connections: &DashMap<i32, PendingConnection>, frame: Vec<u8>) {
    for connection in connections.iter() {
        enqueue_outbound_routed(connection.value(), frame.clone(), false, false);
    }
}

pub(crate) fn broadcast_except(
    connections: &DashMap<i32, PendingConnection>,
    excluded: i32,
    frame: Vec<u8>,
) {
    for entry in connections.iter().filter(|entry| *entry.key() != excluded) {
        enqueue_outbound_routed(entry.value(), frame.clone(), false, false);
    }
}

pub(crate) fn enqueue_outbound(
    connection: &PendingConnection,
    frame: Vec<u8>,
    critical: bool,
) -> bool {
    enqueue_outbound_routed(connection, frame, critical, true)
}
