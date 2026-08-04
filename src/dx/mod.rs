pub mod composition;
pub mod constants;
pub mod device;
pub mod keyed_mutex;
pub mod pipeline;
pub mod staging;
pub mod swapchain;

pub(crate) fn missing_object() -> windows::core::Error {
    windows::core::Error::from_hresult(windows::core::HRESULT(0x8000_FFFFu32 as i32))
}

pub(crate) fn invalid_argument() -> windows::core::Error {
    windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0057u32 as i32))
}
