use crate::{
    boundary::{self, api, insert, with_registry, Error, Resource},
    types::*,
    wrong_resource,
};
use spout_transparent::{visible_bounds, AutoCrop};
use std::time::{Duration, Instant};

pub struct CropState {
    crop: AutoCrop,
    width: u32,
    height: u32,
    origin: Instant,
    last_ms: u64,
}
impl CropState {
    fn time(&mut self, elapsed_ms: u64) -> Result<Instant, Error> {
        if elapsed_ms < self.last_ms {
            return Err(Error::argument("Crop time must be monotonic"));
        }
        let now = self
            .origin
            .checked_add(Duration::from_millis(elapsed_ms))
            .filter(|now| {
                now.checked_add(spout_transparent::crop::SHRINK_DELAY)
                    .is_some()
            })
            .ok_or_else(|| Error::argument("Crop timestamp out of range"))?;
        self.last_ms = elapsed_ms;
        Ok(now)
    }
}

pub unsafe fn alpha<'a>(
    data: *const u8,
    length: usize,
    width: u32,
    height: u32,
) -> Result<&'a [u8], Error> {
    let count = (width as usize)
        .checked_mul(height as usize)
        .filter(|count| *count <= isize::MAX as usize)
        .ok_or_else(|| Error::argument("Alpha dimensions overflow"))?;
    if width == 0 || height == 0 || length < count || length > isize::MAX as usize {
        return Err(Error::argument("Invalid alpha dimensions or buffer length"));
    }
    boundary::pointer(data)?;
    Ok(unsafe { std::slice::from_raw_parts(data, count) })
}

/// Compute padded nonzero-alpha bounds. Empty content is OK with has_content=0.
/// # Safety
/// Data covers length readable bytes; rect and has_content are writable aligned storage.
#[no_mangle]
pub unsafe extern "C" fn spt_visible_bounds(
    data: *const u8,
    length: usize,
    width: u32,
    height: u32,
    rect: *mut SptRect,
    has_content: *mut u32,
) -> i32 {
    api(|| {
        boundary::pointer(rect)?;
        boundary::pointer(has_content)?;
        let data = unsafe { alpha(data, length, width, height)? };
        let bounds = visible_bounds(data, width, height);
        unsafe {
            boundary::output(rect, bounds.map_or(SptRect::default(), Into::into))?;
            boundary::output(has_content, u32::from(bounds.is_some()))
        }
    })
}

/// Create stable crop state; elapsed times supplied to this handle start at zero.
/// # Safety
/// Out is writable, aligned u64 storage.
#[no_mangle]
pub unsafe extern "C" fn spt_crop_create(width: u32, height: u32, out: *mut u64) -> i32 {
    api(|| {
        unsafe { boundary::output(out, 0)? };
        if width == 0 || height == 0 {
            return Err(Error::argument("Crop dimensions must be nonzero"));
        }
        let state = CropState {
            crop: AutoCrop::new(width, height),
            width,
            height,
            origin: Instant::now(),
            last_ms: 0,
        };
        let handle = insert(Resource::Crop(state))?;
        unsafe { boundary::output(out, handle) }
    })
}

/// Observe an alpha sample. Times are monotonic elapsed milliseconds per crop handle.
/// # Safety
/// Data covers length readable bytes; rect points to writable aligned storage.
#[no_mangle]
pub unsafe extern "C" fn spt_crop_observe(
    handle: u64,
    data: *const u8,
    length: usize,
    elapsed_ms: u64,
    rect: *mut SptRect,
) -> i32 {
    api(|| {
        boundary::pointer(rect)?;
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Crop(state)) => {
                let data = unsafe { alpha(data, length, state.width, state.height)? };
                let now = state.time(elapsed_ms)?;
                state
                    .crop
                    .observe(visible_bounds(data, state.width, state.height), now);
                unsafe { boundary::output(rect, state.crop.rect(now).into()) }
            }
            _ => wrong_resource(),
        })
    })
}

/// Advance stable shrink without a new alpha sample.
/// # Safety
/// Rect is writable aligned storage.
#[no_mangle]
pub unsafe extern "C" fn spt_crop_rect(handle: u64, elapsed_ms: u64, rect: *mut SptRect) -> i32 {
    api(|| {
        boundary::pointer(rect)?;
        with_registry(|registry| match registry.get_mut(&handle) {
            Some(Resource::Crop(state)) => {
                let now = state.time(elapsed_ms)?;
                unsafe { boundary::output(rect, state.crop.rect(now).into()) }
            }
            _ => wrong_resource(),
        })
    })
}
