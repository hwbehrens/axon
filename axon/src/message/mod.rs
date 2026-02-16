mod envelope;
mod kind;
mod wire;

pub use envelope::{AgentId, Envelope};
pub use kind::MessageKind;
pub use wire::{MAX_MESSAGE_SIZE, decode, encode, now_millis};
