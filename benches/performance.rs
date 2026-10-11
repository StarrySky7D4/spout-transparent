//! Separate benchmark executable: does not share the test harness's singleton
//! Windows event loop and does not compile measurement code into the application.
// Cargo sets cfg(test) for benches, but a disabled harness does not retain their
// #[test] functions. Embedded UI APIs and test imports are intentionally unused.
#![allow(dead_code, unused_imports)]

#[path = "../src/crop.rs"]
mod crop;
#[cfg(debug_assertions)]
#[path = "../src/diagnostics.rs"]
mod diagnostics;
#[path = "../src/dx/mod.rs"]
mod dx;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/events.rs"]
mod events;
#[path = "../src/frame_pacer.rs"]
mod frame_pacer;
#[path = "../src/interaction/mod.rs"]
mod interaction;
#[path = "support/measurements.rs"]
mod measurements;
#[path = "../src/renderer.rs"]
mod renderer;
#[path = "../src/scheduler.rs"]
mod scheduler;
#[path = "../src/spout/mod.rs"]
mod spout;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    measurements::measure_performance()
}
