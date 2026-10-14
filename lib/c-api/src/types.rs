//! Only fixed-width POD values cross the ABI; no Rust enums, strings or allocations.
use spout_transparent::{CropLayout, CropRect, SenderInfo};

pub const OK: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const INVALID_HANDLE: i32 = 2;
pub const BUFFER_TOO_SMALL: i32 = 3;
pub const NOT_FOUND: i32 = 4;
pub const WOULD_BLOCK: i32 = 5;
pub const IO_ERROR: i32 = 6;
pub const GPU_ERROR: i32 = 7;
pub const PANIC: i32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SptSenderInfo {
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub usage: u32,
    pub partner_id: u32,
    pub share_handle: u32,
    pub reserved: u32,
    pub generation: u64,
}
impl SptSenderInfo {
    pub fn new(info: &SenderInfo, generation: u64) -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            width: info.width,
            height: info.height,
            format: info.format,
            usage: info.usage,
            partner_id: info.partner_id,
            share_handle: info.share_handle,
            reserved: 0,
            generation,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SptRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl From<CropRect> for SptRect {
    fn from(rect: CropRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}
impl From<SptRect> for CropRect {
    fn from(rect: SptRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SptLayout {
    pub crop: SptRect,
    pub scale: f32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
}
// Changing these layouts requires an ABI-major review and matching header changes.
const _: () = {
    assert!(size_of::<SptSenderInfo>() == 40);
    assert!(std::mem::offset_of!(SptSenderInfo, generation) == 32);
    assert!(size_of::<SptRect>() == 16);
    assert!(size_of::<SptLayout>() == 36);
};
impl SptLayout {
    pub fn new(crop: CropRect, layout: CropLayout) -> Self {
        Self {
            crop: crop.into(),
            scale: layout.scale,
            offset_x: layout.offset_x,
            offset_y: layout.offset_y,
            width: layout.width,
            height: layout.height,
        }
    }
}
