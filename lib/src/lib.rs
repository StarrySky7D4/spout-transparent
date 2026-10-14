#![doc = include_str!("../README.md")]

/// Alpha bounds, stable crop policy and source/window coordinate conversion.
pub mod crop;
#[cfg(debug_assertions)]
mod diagnostics;
/// Low-level Windows D3D11 and DirectComposition primitives.
pub mod dx;
/// Contextual errors that retain their native cause.
pub mod error;
/// Frame-rate policy and sender-idle backoff.
pub mod frame_pacer;
/// Single-context rendering, prepared source installation and alpha sampling.
pub mod renderer;
/// Independent deadlines for source, frame, alpha and layout work.
pub mod scheduler;
/// Identity-scoped reconnection policy without a window/event-loop dependency.
pub mod source;
/// Spout sender discovery, metadata, identities and synchronization.
pub mod spout;

pub use crop::{visible_bounds, AutoCrop, CropLayout, CropRect};
pub use error::AppError;
pub use frame_pacer::{FrameOutcome, FramePacer, FrameRate};
pub use renderer::{AlphaReadResult, AlphaSample, AlphaSetup, PreparedSource, Renderer};
pub use source::SourceController;
pub use spout::{
    discover_sender_names, read_sender_info, SenderDiscovery, SenderIdentity, SenderInfo,
    SenderName, SpoutReceiver,
};

/// Matching Win32/COM types for integrations; avoids incompatible windows crate versions.
pub use windows;
