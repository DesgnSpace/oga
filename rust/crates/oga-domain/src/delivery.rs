//! Durable consumer cursors and deliveries.

use serde::{Deserialize, Serialize};

use crate::event::EventPointer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sent,
    Seen,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerCursor {
    pub consumer_id: String,
    pub cursor: i64,
    pub updated_at: String,
}

/// One durable event-feed delivery routed to one consumer channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerDelivery {
    pub consumer_id: String,
    pub event_id: i64,
    pub channel: String,
    pub status: DeliveryStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<String>,
    pub event: EventPointer,
}
