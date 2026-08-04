use std::path::PathBuf;
use std::time::{Duration, Instant};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D::D3D10_1_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Com::{CoInitialize, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_NOREDIRECTIONBITMAP,
};
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, WindowEvent};
use winit::event_loop::ControlFlow;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

use crate::config::FrameRate;
use crate::dx::composition::DCompResources;
use crate::dx::constants;
use crate::dx::device::create_dx11_device_auto;
use crate::dx::keyed_mutex::KeyedMutexGuard;
use crate::dx::pipeline::create_pipeline;
use crate::dx::staging::{create_staging_texture, read_alpha_from_staging};
use crate::dx::swapchain::create_swapchain;
use crate::interaction;
use crate::spout::{NamedMutex, SenderName, SpoutReceiver};

struct ComGuard;
impl ComGuard {
    fn init() -> windows::core::Result<Self> {
        unsafe { CoInitialize(None).ok()? };
        log::info!("COM initialized");
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct SwapchainResources {
    swapchain: IDXGISwapChain1,
    rtv: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl SwapchainResources {
    fn new(device: &ID3D11Device, width: u32, height: u32) -> windows::core::Result<Self> {
        let (swapchain, rtv) = create_swapchain(device, width, height)?;
        let staging = create_staging_texture(device, width, height)?;
        Ok(Self {
            swapchain,
            rtv,
            staging,
            width,
            height,
        })
    }

    fn rebuild(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> windows::core::Result<()> {
        let (swapchain, rtv) = create_swapchain(device, width, height)?;
        let staging = create_staging_texture(device, width, height)?;
        self.swapchain = swapchain;
        self.rtv = rtv;
        self.staging = staging;
        self.width = width;
        self.height = height;
        Ok(())
    }
}

struct SenderResources {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    keyed_mutex: Option<IDXGIKeyedMutex>,
    named_mutex: Option<NamedMutex>,
    handle: *mut std::ffi::c_void,
    width: u32,
    height: u32,
}

impl SenderResources {
    fn open(
        device: &ID3D11Device,
        handle: *mut std::ffi::c_void,
        sender_name: &SenderName,
    ) -> Result<Self, String> {
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { device.OpenSharedResource(HANDLE(handle), &mut tex) }
            .map_err(|error| format!("OpenSharedResource: {error:?}"))?;
        let tex = tex.ok_or_else(|| "OpenSharedResource returned no texture".to_string())?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        log::info!(
            "Sender texture: {}x{} Format={} Usage={} BindFlags=0x{:X} MiscFlags=0x{:X}",
            desc.Width,
            desc.Height,
            desc.Format.0,
            desc.Usage.0,
            desc.BindFlags,
            desc.MiscFlags
        );
        let mut srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: desc.Format,
            ViewDimension: D3D10_1_SRV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        srv_desc.Anonymous.Texture2D.MostDetailedMip = 0;
        srv_desc.Anonymous.Texture2D.MipLevels = 1;
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&tex, Some(&srv_desc), Some(&mut srv)) }
            .map_err(|error| format!("CreateShaderResourceView: {error:?}"))?;
        let srv = srv.ok_or_else(|| "CreateShaderResourceView returned no view".to_string())?;
        let keyed_mutex = tex.cast::<IDXGIKeyedMutex>().ok();
        let named_mutex = if keyed_mutex.is_none() {
            Some(
                NamedMutex::for_sender_texture(sender_name)
                    .map_err(|error| format!("Spout access mutex: {error}"))?,
            )
        } else {
            None
        };
        log::info!(
            "KeyedMutex: {}",
            if keyed_mutex.is_some() {
                "present"
            } else {
                "none"
            }
        );
        Ok(Self {
            tex,
            srv,
            keyed_mutex,
            named_mutex,
            handle,
            width: desc.Width,
            height: desc.Height,
        })
    }
}

#[cfg(debug_assertions)]
fn debug_capture_enabled() -> bool {
    std::env::var("SPOUT_DEBUG_CAPTURE")
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

#[cfg(debug_assertions)]
fn save_texture_to_bmp(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    label: &str,
    path: &std::path::Path,
) {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        tex.GetDesc(&mut desc);
    }
    let cap_desc = D3D11_TEXTURE2D_DESC {
        Width: desc.Width,
        Height: desc.Height,
        Format: desc.Format,
        MipLevels: 1,
        ArraySize: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut cap = None;
    if unsafe { device.CreateTexture2D(&cap_desc, None, Some(&mut cap)) }.is_err() {
        log::error!("save_texture_to_bmp: CreateTexture2D staging failed");
        return;
    }
    let cap = match cap {
        Some(c) => c,
        None => return,
    };
    let src = match tex.cast::<ID3D11Resource>() {
        Ok(r) => r,
        Err(e) => {
            log::error!("cast src failed: {e:?}");
            return;
        }
    };
    let dst = match cap.cast::<ID3D11Resource>() {
        Ok(r) => r,
        Err(e) => {
            log::error!("cast dst failed: {e:?}");
            return;
        }
    };
    unsafe {
        ctx.CopyResource(&dst, &src);
    }
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { ctx.Map(&cap, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
        log::error!("save_texture_to_bmp: Map failed");
        return;
    }
    let w = desc.Width as usize;
    let h = desc.Height as usize;
    let pitch = mapped.RowPitch as usize;
    let ptr = mapped.pData as *const u8;
    let bpp = constants::BPP_RGBA;
    let row_bytes = w * bpp;
    let bmp_row = (row_bytes + 3) & !3;
    let img_size = bmp_row * h;
    let file_size = 54 + img_size;
    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&[0u8; 4]);
    bmp.extend_from_slice(&(54u32).to_le_bytes());
    bmp.extend_from_slice(&(40u32).to_le_bytes());
    bmp.extend_from_slice(&(w as u32).to_le_bytes());
    bmp.extend_from_slice(&(h as u32).to_le_bytes());
    bmp.extend_from_slice(&[1u8, 0, 32, 0]);
    bmp.extend_from_slice(&[0u8; 4]);
    bmp.extend_from_slice(&(img_size as u32).to_le_bytes());
    bmp.extend_from_slice(&[0x13u8, 0x0B, 0, 0, 0x13u8, 0x0B, 0, 0]);
    bmp.extend_from_slice(&[0u8; 8]);
    for y in (0..h).rev() {
        let row_ptr = unsafe { ptr.add(y * pitch) };
        let row = unsafe { std::slice::from_raw_parts(row_ptr, row_bytes) };
        bmp.extend_from_slice(row);
        let pad = bmp_row - row_bytes;
        bmp.extend(std::iter::repeat_n(0, pad));
    }
    unsafe {
        ctx.Unmap(&cap, 0);
    }
    match std::fs::write(path, &bmp) {
        Ok(_) => log::info!(
            "{label} saved: {path:?} ({w}x{h}, Format={})",
            desc.Format.0
        ),
        Err(e) => log::error!("Write {label} failed: {e}"),
    }
}

#[cfg(debug_assertions)]
fn sample_pixel(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    x: u32,
    y: u32,
    label: &str,
) {
    let cap_desc = D3D11_TEXTURE2D_DESC {
        Width: 1,
        Height: 1,
        Format: unsafe {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut d);
            d.Format
        },
        MipLevels: 1,
        ArraySize: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut cap = None;
    if unsafe { device.CreateTexture2D(&cap_desc, None, Some(&mut cap)) }.is_err() {
        return;
    }
    if let Some(cap) = cap {
        let src = tex.cast::<ID3D11Resource>().ok();
        let dst = cap.cast::<ID3D11Resource>().ok();
        if let (Some(s), Some(d)) = (src, dst) {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                tex.GetDesc(&mut desc);
            }
            let box0 = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + 1,
                bottom: y + 1,
                back: 1,
            };
            unsafe {
                ctx.CopySubresourceRegion(&d, 0, 0, 0, 0, &s, 0, Some(&box0));
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if unsafe { ctx.Map(&cap, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_ok() {
                let ptr = mapped.pData as *const u8;
                let b = unsafe { *ptr };
                let g = unsafe { *ptr.offset(1) };
                let r = unsafe { *ptr.offset(2) };
                let a = unsafe { *ptr.offset(3) };
                log::info!("{label} pixel({x},{y}) BGRA=({b}, {g}, {r}, {a})");
                unsafe {
                    ctx.Unmap(&cap, 0);
                }
            }
        }
    }
}

fn init_spout() -> Result<SpoutReceiver, String> {
    let mut spout = SpoutReceiver::new();
    let sender_name = spout
        .connect_first(Duration::from_secs(5))
        .map_err(|error| format!("Spout connection: {error}"))?;
    log::info!("Connected to '{sender_name}'");
    Ok(spout)
}

struct AppInit {
    #[allow(dead_code)]
    com_guard: ComGuard,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    spout: SpoutReceiver,
    sender: SenderResources,
    swapchain_res: SwapchainResources,
    pipeline: crate::dx::pipeline::Pipeline,
}

fn init_app() -> Result<AppInit, String> {
    let com_guard = ComGuard::init().map_err(|e| format!("COM init: {e:?}"))?;

    let (device, context) = create_dx11_device_auto().map_err(|e| format!("DX11 device: {e:?}"))?;
    log::info!("DX11 device OK");

    let spout = init_spout()?;

    let raw_handle = spout.sender_handle();
    let sender_name = spout
        .current_name()
        .ok_or_else(|| "Spout receiver has no selected sender".to_string())?;
    log::info!("Shared handle={raw_handle:?}");

    let sender = SenderResources::open(&device, raw_handle, sender_name)?;

    #[cfg(debug_assertions)]
    {
        if debug_capture_enabled() {
            let exe_dir = std::env::current_exe()
                .map_err(|e| format!("exe path: {e}"))?
                .parent()
                .ok_or("no parent")?
                .to_path_buf();
            save_texture_to_bmp(
                &device,
                &context,
                &sender.tex,
                "SenderTexture",
                &exe_dir.join("debug_01_sender_texture.bmp"),
            );
            sample_pixel(
                &device,
                &context,
                &sender.tex,
                sender.width / 2,
                sender.height / 2,
                "SenderCenter",
            );
        }
    }

    let pipeline = create_pipeline(&device).map_err(|e| format!("Pipeline: {e:?}"))?;
    log::info!("Pipeline OK");

    let swapchain_res = SwapchainResources::new(&device, sender.width, sender.height)
        .map_err(|e| format!("Swapchain: {e:?}"))?;
    log::info!("Swapchain OK ({}x{})", sender.width, sender.height);

    Ok(AppInit {
        com_guard,
        device,
        context,
        spout,
        sender,
        swapchain_res,
        pipeline,
    })
}

fn render_frame(
    context: &ID3D11DeviceContext,
    pipeline: &crate::dx::pipeline::Pipeline,
    srv: &ID3D11ShaderResourceView,
    rtv: &ID3D11RenderTargetView,
    width: u32,
    height: u32,
) {
    unsafe {
        context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        context.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 0.0]);
        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]));
        context.RSSetState(&pipeline.raster_state);
        context.VSSetShader(&pipeline.vs, None);
        context.PSSetShader(&pipeline.ps, None);
        context.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        context.PSSetSamplers(0, Some(&[Some(pipeline.sampler.clone())]));
        context.IASetInputLayout(None);
        context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context.Draw(3, 0);
    }
}

fn clear_frame(context: &ID3D11DeviceContext, rtv: &ID3D11RenderTargetView) {
    unsafe {
        context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        context.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 0.0]);
    }
}

fn update_alpha(
    context: &ID3D11DeviceContext,
    swapchain_res: &SwapchainResources,
    interaction_state: &mut interaction::InteractionState,
    alpha_buf: &mut Vec<u8>,
) {
    if let Ok(bb) = unsafe { swapchain_res.swapchain.GetBuffer::<ID3D11Texture2D>(0) } {
        let dst = swapchain_res.staging.cast::<ID3D11Resource>().ok();
        let src = bb.cast::<ID3D11Resource>().ok();
        if let (Some(d), Some(s)) = (dst, src) {
            unsafe {
                context.CopyResource(&d, &s);
            }
            read_alpha_from_staging(
                context,
                &swapchain_res.staging,
                swapchain_res.width,
                swapchain_res.height,
                alpha_buf,
            );
            if !alpha_buf.is_empty() {
                let w = swapchain_res.width;
                let h = swapchain_res.height;
                interaction_state.update_alpha_mask(std::mem::take(alpha_buf), w, h);
            }
        }
    }
}

struct RenderState {
    spout: SpoutReceiver,
    sender: Option<SenderResources>,
    swapchain_res: Option<SwapchainResources>,
    pipeline: crate::dx::pipeline::Pipeline,
    frame_count: u64,
    last_fail_time: Instant,
    first_render_logged: bool,
    base_width: u32,
    base_height: u32,
    current_handle: *mut std::ffi::c_void,
    current_sender_name: SenderName,
    current_sender_generation: u64,
    last_scale: f32,
    alpha_buf: Vec<u8>,
    frame_rate: FrameRate,
    last_render_time: Instant,
}

impl RenderState {
    fn should_render(&self, has_new_frame: bool) -> bool {
        if !has_new_frame {
            return false;
        }
        match self.frame_rate.interval() {
            None => true,
            Some(interval) => self.last_render_time.elapsed() >= interval,
        }
    }
}

pub fn run() -> Result<(), String> {
    log::info!("=== Spout Transparent ===");
    let app_init = init_app()?;

    #[allow(deprecated)]
    let event_loop =
        winit::event_loop::EventLoop::new().map_err(|e| format!("EventLoop: {e:?}"))?;
    #[allow(deprecated)]
    let window = event_loop
        .create_window(
            Window::default_attributes()
                .with_title("Spout Receiver (Transparent)")
                .with_inner_size(winit::dpi::LogicalSize::new(
                    app_init.sender.width as f64,
                    app_init.sender.height as f64,
                ))
                .with_transparent(true)
                .with_decorations(false),
        )
        .map_err(|e| format!("Window: {e:?}"))?;
    window.set_visible(false);

    let hwnd = match window
        .window_handle()
        .map_err(|e| format!("handle: {e:?}"))?
        .as_raw()
    {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
        _ => return Err("Unsupported window handle".into()),
    };
    log::info!("Window OK, HWND={hwnd:?}");

    unsafe {
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOREDIRECTIONBITMAP.0 as i32));
    }

    interaction::InteractionState::init_window_style(hwnd);

    let hotkey_path = if std::path::Path::new("hotkeys.json").exists() {
        PathBuf::from("hotkeys.json")
    } else {
        std::env::current_exe()
            .map_err(|e| format!("exe: {e}"))?
            .parent()
            .ok_or("no parent")?
            .join("hotkeys.json")
    };
    if hotkey_path.exists() {
        interaction::load_hotkey_config(&hotkey_path, hwnd)
            .map_err(|e| format!("Hotkey config: {e}"))?;
    } else {
        interaction::register_default_hotkeys(hwnd).map_err(|e| format!("Hotkey register: {e}"))?;
    }
    interaction::install_hotkey_subclass(hwnd).map_err(|e| format!("Hotkey subclass: {e}"))?;
    log::info!("Hotkeys OK");

    let dxgi_device: IDXGIDevice = app_init
        .device
        .cast()
        .map_err(|e| format!("IDXGIDevice: {e:?}"))?;
    let dcomp = crate::dx::composition::setup_dcomp(&dxgi_device, hwnd)
        .map_err(|e| format!("DComp setup: {e:?}"))?;

    let _ = window.request_inner_size(PhysicalSize::new(
        app_init.sender.width,
        app_init.sender.height,
    ));

    unsafe {
        let _ = dcomp
            .root_visual
            .SetContent(&app_init.swapchain_res.swapchain);
        let _ = dcomp.device.Commit();
    }

    unsafe {
        context_clear_present(&app_init.context, &app_init.swapchain_res, &dcomp);
    }
    log::info!("Initial frame presented");

    #[cfg(debug_assertions)]
    {
        let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
        log::info!(
            "ExStyle=0x{ex:X} LAYERED={} TRANSPARENT={} NOREDIR={}",
            (ex & constants::EXSTYLE_LAYERED) != 0,
            (ex & constants::EXSTYLE_TRANSPARENT) != 0,
            (ex & constants::EXSTYLE_NOREDIRECTIONBITMAP) != 0,
        );
    }

    let mut interaction_state = interaction::InteractionState::new();
    interaction_state
        .install_mouse_hook(hwnd)
        .map_err(|e| format!("Mouse hook: {e}"))?;
    log::info!("InteractionState OK");

    window.set_visible(true);
    log::info!("=== Entering render loop ===");

    let context = app_init.context.clone();
    let device = app_init.device.clone();
    let dcomp_device = dcomp.device.clone();
    let root_visual = dcomp.root_visual.clone();

    let init_handle = app_init.sender.handle;
    let init_sender_name = app_init
        .spout
        .current_name()
        .cloned()
        .ok_or_else(|| "Spout receiver lost its sender during initialization".to_string())?;
    let init_sender_generation = app_init.spout.generation();
    let init_w = app_init.sender.width;
    let init_h = app_init.sender.height;

    let mut rs = RenderState {
        spout: app_init.spout,
        sender: Some(app_init.sender),
        swapchain_res: Some(app_init.swapchain_res),
        pipeline: app_init.pipeline,
        frame_count: 0,
        last_fail_time: Instant::now(),
        first_render_logged: false,
        base_width: init_w,
        base_height: init_h,
        current_handle: init_handle,
        current_sender_name: init_sender_name,
        current_sender_generation: init_sender_generation,
        last_scale: interaction_state.scale_factor,
        alpha_buf: Vec::new(),
        frame_rate: FrameRate::Unlimited,
        last_render_time: Instant::now(),
    };

    #[allow(deprecated)]
    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Poll);
            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
                        interaction_state.cleanup(hwnd);
                        elwt.exit();
                    }
                    WindowEvent::ModifiersChanged(mods) => {
                        interaction_state.update_modifiers(mods.state());
                    }
                    WindowEvent::KeyboardInput { event: ref ke, .. } => {
                        if ke.state == ElementState::Pressed
                            && !ke.repeat
                            && ke.physical_key == PhysicalKey::Code(KeyCode::Escape)
                        {
                            interaction_state.cleanup(hwnd);
                            elwt.exit();
                            return;
                        }
                        interaction_state.handle_keyboard(ke, hwnd);
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        interaction_state.handle_scroll(delta);
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        interaction_state.handle_mouse_input(state, button, &window);
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        interaction_state.handle_cursor_moved(position, &window);
                    }
                    _ => {}
                },
                Event::AboutToWait => {
                    for ev in interaction_state.poll_hook_events() {
                        interaction::handle_passthrough_event(&ev, hwnd);
                    }

                    if interaction::poll_quit() {
                        interaction_state.cleanup(hwnd);
                        elwt.exit();
                        return;
                    }
                    if interaction::poll_toggle_interaction() {
                        interaction_state.toggle_enabled(hwnd);
                    }
                    if interaction::poll_toggle_topmost() {
                        interaction_state.toggle_topmost(hwnd);
                    }
                    if interaction::poll_cycle_framerate() {
                        rs.frame_rate = rs.frame_rate.cycle();
                        log::info!("Frame rate: {}", rs.frame_rate.display_name());
                    }

                    let recv = match rs.spout.poll() {
                        Ok(has_sender) => has_sender,
                        Err(error) => {
                            log::warn!("Spout metadata poll failed: {error}");
                            false
                        }
                    };
                    rs.frame_count += 1;

                    let new_handle = rs.spout.sender_handle();
                    let new_sender_name = rs.spout.current_name().cloned();
                    let new_sender_generation = rs.spout.generation();
                    let handle_ok = !new_handle.is_null();
                    let cooldown = rs.last_fail_time.elapsed() > Duration::from_millis(500);

                    if rs.frame_count <= 3 || rs.frame_count.is_multiple_of(300) {
                        log::debug!(
                            "[Frame {}] recv={} handle_valid={} same={}",
                            rs.frame_count, recv, handle_ok, new_handle == rs.current_handle
                        );
                    }

                    let sender_changed = new_sender_name
                        .as_ref()
                        .is_some_and(|name| name != &rs.current_sender_name);
                    if handle_ok
                        && cooldown
                        && (new_handle != rs.current_handle
                            || sender_changed
                            || new_sender_generation != rs.current_sender_generation)
                    {
                        log::info!("[Frame {}] Sender resource changed, rebuilding...", rs.frame_count);
                        let old_sender = rs.sender.take();
                        if let Some(mut sr) = rs.swapchain_res.take() {
                            let Some(sender_name) = new_sender_name.as_ref() else {
                                rs.sender = old_sender;
                                rs.swapchain_res = Some(sr);
                                return;
                            };
                            match handle_sender_change(
                                &device,
                                new_handle,
                                sender_name,
                                &mut sr,
                            ) {
                                Ok(new_sender) => {
                                    rs.base_width = new_sender.width;
                                    rs.base_height = new_sender.height;
                                    interaction_state.scale_factor = 1.0;
                                    rs.last_scale = 1.0;
                                    unsafe {
                                        let _ = root_visual.SetContent(&sr.swapchain);
                                        let _ = dcomp_device.Commit();
                                    }
                                    let _ = window.request_inner_size(
                                        PhysicalSize::new(sr.width, sr.height),
                                    );
                                    rs.swapchain_res = Some(sr);
                                    rs.sender = Some(new_sender);
                                    rs.current_handle = new_handle;
                                    rs.current_sender_name = sender_name.clone();
                                    rs.current_sender_generation = new_sender_generation;
                                    drop(old_sender);
                                    log::info!("[Frame {}] Texture rebuild complete", rs.frame_count);
                                }
                                Err(e) => {
                                    log::error!("Rebuild failed: {e:?}");
                                    rs.sender = old_sender;
                                    rs.swapchain_res = Some(sr);
                                    rs.last_fail_time = Instant::now();
                                }
                            }
                        }
                    }

                    let should_render = rs.should_render(recv);

                    if let (Some(sr), Some(sn)) = (rs.swapchain_res.as_mut(), rs.sender.as_ref()) {
                        let target_size =
                            interaction_state.scaled_size(rs.base_width, rs.base_height);
                        let target_w = target_size.width;
                        let target_h = target_size.height;

                        if (rs.last_scale - interaction_state.scale_factor).abs() > f32::EPSILON {
                            if target_w == sr.width && target_h == sr.height {
                                rs.last_scale = interaction_state.scale_factor;
                            } else {
                                match sr.rebuild(&device, target_w, target_h) {
                                    Ok(()) => {
                                        rs.last_scale = interaction_state.scale_factor;
                                        unsafe {
                                            let _ = root_visual.SetContent(&sr.swapchain);
                                            let _ = dcomp_device.Commit();
                                        }
                                        let _ = window.request_inner_size(target_size);
                                    }
                                    Err(error) => {
                                        interaction_state.scale_factor = rs.last_scale;
                                        log::warn!(
                                            "Resize to {target_w}x{target_h} failed; keeping the previous scale: {error:?}"
                                        );
                                    }
                                }
                            }
                        }

                        if should_render {
                            let mutex_guard = sn
                                .keyed_mutex
                                .as_ref()
                                .and_then(KeyedMutexGuard::try_acquire);
                            let named_mutex_guard = if sn.keyed_mutex.is_none() {
                                sn.named_mutex
                                    .as_ref()
                                    .and_then(|mutex| mutex.try_lock().ok().flatten())
                            } else {
                                None
                            };
                            let owns_sender_texture = mutex_guard.is_some()
                                || (sn.keyed_mutex.is_none() && named_mutex_guard.is_some());

                            if owns_sender_texture {
                                render_frame(
                                    &context,
                                    &rs.pipeline,
                                    &sn.srv,
                                    &sr.rtv,
                                    sr.width,
                                    sr.height,
                                );

                                #[cfg(debug_assertions)]
                                if !rs.first_render_logged {
                                    log::info!("[First frame] Draw complete");
                                    if debug_capture_enabled() {
                                        if let Ok(bb) = unsafe {
                                            sr.swapchain.GetBuffer::<ID3D11Texture2D>(0)
                                        } {
                                            let exe_dir = std::env::current_exe()
                                                .ok()
                                                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                                                .unwrap_or_default();
                                            save_texture_to_bmp(
                                                &device, &context, &bb, "BackBuffer",
                                                &exe_dir.join("debug_02_backbuffer_after_draw.bmp"),
                                            );
                                            sample_pixel(&device, &context, &bb,
                                                         sr.width / 2, sr.height / 2, "BB center");
                                        }
                                    }
                                    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
                                    log::info!(
                                        "ExStyle=0x{ex:X} LAYERED={} TRANSPARENT={} NOREDIR={}",
                                        (ex & constants::EXSTYLE_LAYERED) != 0,
                                        (ex & constants::EXSTYLE_TRANSPARENT) != 0,
                                        (ex & constants::EXSTYLE_NOREDIRECTIONBITMAP) != 0,
                                    );
                                    rs.first_render_logged = true;
                                }

                                rs.last_render_time = Instant::now();
                            } else {
                                log::trace!("Skipped frame: sender keyed mutex is busy");
                            }
                        }

                        if interaction_state.should_update_alpha() {
                            update_alpha(&context, sr, &mut interaction_state, &mut rs.alpha_buf);
                        }
                    } else if let Some(sr) = rs.swapchain_res.as_ref() {
                        clear_frame(&context, &sr.rtv);
                    }

                    if let Some(sr) = rs.swapchain_res.as_ref() {
                        let present_result = unsafe { sr.swapchain.Present(1, DXGI_PRESENT(0)) };
                        if present_result.is_err() {
                            log::error!("Swapchain Present failed: {present_result:?}");
                            interaction_state.cleanup(hwnd);
                            elwt.exit();
                            return;
                        }
                    }

                    if !recv {
                        let sleep_dur = match rs.frame_rate.interval() {
                            Some(interval) => {
                                let remaining = interval.saturating_sub(rs.last_render_time.elapsed());
                                remaining.min(Duration::from_millis(2))
                            }
                            None => Duration::from_millis(1),
                        };
                        if !sleep_dur.is_zero() {
                            std::thread::sleep(sleep_dur);
                        }
                    }
                }
                _ => {}
            }
        })
        .map_err(|e| format!("EventLoop: {e:?}"))?;

    Ok(())
}

fn handle_sender_change(
    device: &ID3D11Device,
    new_handle: *mut std::ffi::c_void,
    sender_name: &SenderName,
    swapchain_res: &mut SwapchainResources,
) -> Result<SenderResources, String> {
    let new_sender = SenderResources::open(device, new_handle, sender_name)?;
    if new_sender.width != swapchain_res.width || new_sender.height != swapchain_res.height {
        swapchain_res
            .rebuild(device, new_sender.width, new_sender.height)
            .map_err(|error| format!("Swapchain rebuild: {error:?}"))?;
    }
    Ok(new_sender)
}

unsafe fn context_clear_present(
    context: &ID3D11DeviceContext,
    sr: &SwapchainResources,
    dcomp: &DCompResources,
) {
    context.OMSetRenderTargets(Some(&[Some(sr.rtv.clone())]), None);
    context.ClearRenderTargetView(&sr.rtv, &[0.0, 0.0, 0.0, 0.0]);
    let _ = sr.swapchain.Present(0, DXGI_PRESENT(0));
    let _ = dcomp.device.Commit();
}
