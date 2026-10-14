//! This crate links only the published library surface, just like a third-party client.
use spout_transparent::{
    visible_bounds, AutoCrop, CropLayout, CropRect, FrameOutcome, FramePacer, FrameRate,
    SenderName, SourceController, SpoutReceiver,
};
use std::{
    io,
    time::{Duration, Instant},
};

#[test]
fn sender_names_validate_public_inputs_and_preserve_native_bytes() {
    for bytes in [Vec::new(), vec![b'x'; 256], b"source\0suffix".to_vec()] {
        assert_eq!(
            SenderName::from_bytes(bytes).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert!(SenderName::from_bytes([b'x'; 255]).is_ok());
    let name = SenderName::from_bytes([0xff, b'A']).unwrap();
    assert_eq!(name.as_bytes(), &[0xff, b'A']);
    let configured: SenderName = "VTubeStudioSpout".parse().unwrap();
    assert_eq!(configured.as_bytes(), b"VTubeStudioSpout");
}

#[test]
fn public_policies_can_be_composed_without_a_window_or_logger() {
    let now = Instant::now();
    let mut pacer = FramePacer::new(now);
    pacer.set_rate(FrameRate::Fps30);
    pacer.presented(now);
    assert_eq!(
        pacer.next_wake(now, true, FrameOutcome::Presented),
        Some(now + Duration::from_nanos(33_333_333))
    );

    let mut alpha = vec![0; 64 * 64];
    alpha[32 * 64 + 32] = 255;
    let bounds = visible_bounds(&alpha, 64, 64).unwrap();
    let mut crop = AutoCrop::new(64, 64);
    crop.observe(Some(bounds), now);
    assert_eq!(crop.rect(now), CropRect::full(64, 64));
    assert_eq!(crop.rect(now + Duration::from_millis(500)), bounds);
    let layout = CropLayout::new(bounds, 2.0);
    assert_eq!(layout.offset_x, bounds.x as i32 * 2);

    let receiver = SpoutReceiver::default();
    assert!(receiver.identity().is_none());
    assert!(receiver.sender_info().is_none());
    assert!(SourceController::default().next_wake(None, now).is_none());
}
