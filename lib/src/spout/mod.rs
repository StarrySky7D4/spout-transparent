//! Spout façade. Protocol, polling, discovery and native-handle ownership stay separate.
mod discovery;
mod frame_counter;
mod protocol;
mod receiver;
mod shared_memory;

pub use discovery::{discover_sender_names, SenderDiscovery};
pub use frame_counter::FrameCounter;
pub use protocol::read_sender_info;
pub use protocol::{SenderIdentity, SenderInfo, SenderName};
pub use receiver::SpoutReceiver;
pub use shared_memory::NamedMutex;
