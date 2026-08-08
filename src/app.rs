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

use crate::config::FramePacer;
#[cfg(debug_assertions)]
use crate::dx::constants;
use crate::dx::device::create_dx11_device_auto;
use crate::dx::keyed_mutex::KeyedMutexGuard;
use crate::dx::pipeline::create_pipeline;
use crate::dx::staging::AlphaReadback;
use crate::dx::swapchain::create_swapchain;
use crate::interaction;
use crate::spout::{FrameCounter, NamedMutex, SenderName, SpoutReceiver};
use crate::tray::{TrayAction, TrayIcon, TrayState};

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
    alpha_readback: Option<AlphaReadback>,
    width: u32,
    height: u32,
}

impl SwapchainResources {
    fn new(device: &ID3D11Device, width: u32, height: u32) -> windows::core::Result<Self> {
        let (swapchain, rtv) = create_swapchain(device, width, height)?;
        Ok(Self {
            swapchain,
            rtv,
            alpha_readback: None,
            width,
            height,
        })
    }

    fn set_alpha_enabled(
        &mut self,
        device: &ID3D11Device,
        enabled: bool,
    ) -> windows::core::Result<bool> {
        if !enabled {
            self.alpha_readback = None;
            return Ok(false);
        }
        if self.alpha_readback.is_none() {
            self.alpha_readback = Some(AlphaReadback::new(device, self.width, self.height)?);
            return Ok(true);
        }
        Ok(false)
    }
}

struct SenderResources {
    #[cfg(debug_assertions)]
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    keyed_mutex: Option<IDXGIKeyedMutex>,
    named_mutex: Option<NamedMutex>,
    frame_counter: Option<FrameCounter>,
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
        let frame_counter = match FrameCounter::for_sender(sender_name) {
            Ok(counter) => Some(counter),
            Err(error) => {
                log::warn!("Spout frame counter unavailable for '{sender_name}': {error}");
                None
            }
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
            #[cfg(debug_assertions)]
            tex,
            srv,
            keyed_mutex,
            named_mutex,
            frame_counter,
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

#[cfg(debug_assertions)]
fn capture_sender_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    sender: &SenderResources,
) {
    if !debug_capture_enabled() {
        return;
    }
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
    else {
        log::warn!("Could not locate the executable directory for sender capture");
        return;
    };
    save_texture_to_bmp(
        device,
        context,
        &sender.tex,
        "SenderTexture",
        &exe_dir.join("debug_01_sender_texture.bmp"),
    );
    sample_pixel(
        device,
        context,
        &sender.tex,
        sender.width / 2,
        sender.height / 2,
        "SenderCenter",
    );
}

struct AppInit {
    #[allow(dead_code)]
    com_guard: ComGuard,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    spout: SpoutReceiver,
    pipeline: crate::dx::pipeline::Pipeline,
}

fn init_app() -> Result<AppInit, String> {
    let com_guard = ComGuard::init().map_err(|e| format!("COM init: {e:?}"))?;

    let (device, context) = create_dx11_device_auto().map_err(|e| format!("DX11 device: {e:?}"))?;
    log::info!("DX11 device OK");

    let spout = SpoutReceiver::new();

    let pipeline = create_pipeline(&device).map_err(|e| format!("Pipeline: {e:?}"))?;
    log::info!("Pipeline OK");

    Ok(AppInit {
        com_guard,
        device,
        context,
        spout,
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

fn update_alpha(
    context: &ID3D11DeviceContext,
    swapchain_res: &mut SwapchainResources,
    pipeline: &crate::dx::pipeline::Pipeline,
    source: &ID3D11ShaderResourceView,
    interaction_state: &mut interaction::InteractionState,
    alpha_buf: &mut Vec<u8>,
    schedule_extract: bool,
) {
    let Some(readback) = swapchain_res.alpha_readback.as_mut() else {
        return;
    };
    match readback.try_read(context, alpha_buf) {
        Ok(true) => {
            let reusable = interaction_state.update_alpha_mask(
                std::mem::take(alpha_buf),
                swapchain_res.width,
                swapchain_res.height,
            );
            if let Some(reusable) = reusable {
                *alpha_buf = reusable;
            }
        }
        Ok(false) => {}
        Err(error) => log::warn!("Alpha readback failed: {error:?}"),
    }

    if !schedule_extract {
        return;
    }

    if let Err(error) = readback.enqueue_extract(context, pipeline, source) {
        log::warn!("Alpha R8 extraction failed: {error:?}");
    }
}

struct RenderState {
    spout: SpoutReceiver,
    sender: Option<SenderResources>,
    swapchain_res: Option<SwapchainResources>,
    pipeline: crate::dx::pipeline::Pipeline,
    frame_count: u64,
    last_fail_time: Instant,
    #[cfg(debug_assertions)]
    first_render_logged: bool,
    base_width: u32,
    base_height: u32,
    current_handle: *mut std::ffi::c_void,
    current_sender_name: Option<SenderName>,
    current_sender_generation: u64,
    last_scale: f32,
    alpha_buf: Vec<u8>,
    pacer: FramePacer,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FrameUpdate {
    #[default]
    Unchanged,
    Rendered,
}

impl FrameUpdate {
    fn needs_present(self) -> bool {
        self != Self::Unchanged
    }
}

fn window_should_be_visible(source_ready: bool, visibility_requested: bool) -> bool {
    source_ready && visibility_requested
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
                .with_inner_size(winit::dpi::LogicalSize::new(1.0, 1.0))
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
    let tray_icon = TrayIcon::install(hwnd).map_err(|e| format!("Tray icon: {e:?}"))?;

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

    let mut window_requested_visible = true;
    let mut window_visible = false;
    let mut source_available = false;
    tray_icon.update_state(TrayState {
        visible: window_visible,
        interaction: interaction_state.enabled,
        topmost: interaction_state.topmost,
        frame_rate: crate::config::FrameRate::Unlimited,
        has_source: source_available,
    });
    tray_icon.update_sources(Vec::new(), None);
    log::info!("No Spout sender yet; waiting in the system tray");
    log::info!("=== Entering render loop ===");

    let context = app_init.context.clone();
    let device = app_init.device.clone();
    let dcomp_device = dcomp.device.clone();
    let root_visual = dcomp.root_visual.clone();
    let scale_transform = dcomp.scale_transform.clone();

    let mut rs = RenderState {
        spout: app_init.spout,
        sender: None,
        swapchain_res: None,
        pipeline: app_init.pipeline,
        frame_count: 0,
        last_fail_time: Instant::now() - Duration::from_secs(1),
        #[cfg(debug_assertions)]
        first_render_logged: false,
        base_width: 1,
        base_height: 1,
        current_handle: std::ptr::null_mut(),
        current_sender_name: None,
        current_sender_generation: 0,
        last_scale: interaction_state.scale_factor,
        alpha_buf: Vec::new(),
        pacer: FramePacer::new(Instant::now()),
    };
    let mut available_senders = Vec::new();
    let mut menu_selected_source: Option<SenderName> = None;
    let mut next_sender_refresh = Instant::now();
    let mut redraw_requested = true;

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
                        let frame_rate = rs.pacer.cycle();
                        log::info!("Frame rate: {}", frame_rate.display_name());
                    }

                    if let Some(action) = tray_icon.take_action() {
                        match action {
                            TrayAction::ToggleVisibility => {
                                if source_available {
                                    window_requested_visible = !window_visible;
                                    window_visible = window_requested_visible;
                                    window.set_visible(window_visible);
                                    if window_visible {
                                        redraw_requested = true;
                                        rs.pacer.request_frame();
                                    }
                                }
                            }
                            TrayAction::ToggleInteraction => {
                                interaction_state.toggle_enabled(hwnd);
                            }
                            TrayAction::ToggleTopmost => {
                                interaction_state.toggle_topmost(hwnd);
                            }
                            TrayAction::SetFrameRate(frame_rate) => {
                                rs.pacer.set_rate(frame_rate);
                                log::info!("Frame rate: {}", frame_rate.display_name());
                            }
                            TrayAction::SelectSource(index) => {
                                if let Some(sender_name) = available_senders.get(index).cloned() {
                                    log::info!("Selecting Spout sender '{sender_name}'");
                                    rs.spout.select(sender_name);
                                    rs.pacer.request_frame();
                                }
                            }
                            TrayAction::Quit => {
                                interaction_state.cleanup(hwnd);
                                elwt.exit();
                                return;
                            }
                        }
                    }

                    let now = Instant::now();
                    if now >= next_sender_refresh {
                        match rs.spout.sender_names() {
                            Ok(senders) => {
                                available_senders = senders;
                                tray_icon.update_sources(
                                    available_senders
                                        .iter()
                                        .map(SenderName::display_name)
                                        .collect(),
                                    rs.spout.current_name().map(SenderName::display_name),
                                );
                                menu_selected_source = rs.spout.current_name().cloned();
                            }
                            Err(error) => {
                                log::warn!("Failed to refresh Spout sender list: {error}");
                            }
                        }
                        next_sender_refresh = now + Duration::from_millis(500);
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

                    if new_sender_name != menu_selected_source {
                        tray_icon.update_sources(
                            available_senders
                                .iter()
                                .map(SenderName::display_name)
                                .collect(),
                            new_sender_name.as_ref().map(SenderName::display_name),
                        );
                        menu_selected_source = new_sender_name.clone();
                    }

                    if rs.frame_count <= 3 || rs.frame_count.is_multiple_of(300) {
                        log::debug!(
                            "[Frame {}] recv={} handle_valid={} same={}",
                            rs.frame_count, recv, handle_ok, new_handle == rs.current_handle
                        );
                    }

                    let sender_changed = new_sender_name != rs.current_sender_name;
                    if handle_ok
                        && cooldown
                        && (new_handle != rs.current_handle
                            || sender_changed
                            || new_sender_generation != rs.current_sender_generation)
                    {
                        log::info!("[Frame {}] Sender resource changed, rebuilding...", rs.frame_count);
                        let Some(sender_name) = new_sender_name.as_ref() else {
                            return;
                        };
                        let replacement = SenderResources::open(&device, new_handle, sender_name)
                            .and_then(|sender| {
                                SwapchainResources::new(&device, sender.width, sender.height)
                                    .map(|swapchain| (sender, swapchain))
                                    .map_err(|error| format!("Swapchain: {error:?}"))
                            });
                        match replacement {
                            Ok((new_sender, sr)) => {
                                #[cfg(debug_assertions)]
                                capture_sender_texture(&device, &context, &new_sender);
                                rs.base_width = new_sender.width;
                                rs.base_height = new_sender.height;
                                interaction_state.scale_factor = 1.0;
                                rs.last_scale = 1.0;
                                unsafe {
                                    let _ = scale_transform.SetScaleX2(1.0);
                                    let _ = scale_transform.SetScaleY2(1.0);
                                    let _ = root_visual.SetContent(&sr.swapchain);
                                    let _ = dcomp_device.Commit();
                                }
                                let _ = window
                                    .request_inner_size(PhysicalSize::new(sr.width, sr.height));
                                rs.swapchain_res = Some(sr);
                                rs.sender = Some(new_sender);
                                rs.current_handle = new_handle;
                                rs.current_sender_name = Some(sender_name.clone());
                                rs.current_sender_generation = new_sender_generation;
                                redraw_requested = true;
                                rs.pacer.request_frame();
                                log::info!("[Frame {}] Texture rebuild complete", rs.frame_count);
                            }
                            Err(error) => {
                                log::error!("Rebuild failed: {error}");
                                rs.last_fail_time = Instant::now();
                            }
                        }
                    }

                    let source_ready = handle_ok
                        && rs.sender.is_some()
                        && rs.swapchain_res.is_some()
                        && rs.current_handle == new_handle
                        && rs.current_sender_name == new_sender_name;
                    if source_ready != source_available {
                        source_available = source_ready;
                        window_visible =
                            window_should_be_visible(source_available, window_requested_visible);
                        window.set_visible(window_visible);
                        if window_visible {
                            redraw_requested = true;
                            rs.pacer.request_frame();
                            log::info!("Spout sender available; showing the window");
                        } else {
                            log::info!("No usable Spout sender; hiding the window");
                        }
                    }

                    tray_icon.update_state(TrayState {
                        visible: window_visible,
                        interaction: interaction_state.enabled,
                        topmost: interaction_state.topmost,
                        frame_rate: rs.pacer.rate(),
                        has_source: source_available,
                    });

                    let mut frame_update = FrameUpdate::Unchanged;

                    if let (Some(sr), Some(sn)) =
                        (rs.swapchain_res.as_mut(), rs.sender.as_mut())
                    {
                        let target_size =
                            interaction_state.scaled_size(rs.base_width, rs.base_height);

                        let scale_changed =
                            (rs.last_scale - interaction_state.scale_factor).abs() > f32::EPSILON;
                        if scale_changed {
                            let scale = interaction_state.scale_factor;
                            let transform_result = unsafe {
                                scale_transform
                                    .SetScaleX2(scale)
                                    .and_then(|_| scale_transform.SetScaleY2(scale))
                                    .and_then(|_| dcomp_device.Commit())
                            };
                            if let Err(error) = transform_result {
                                interaction_state.scale_factor = rs.last_scale;
                                log::warn!(
                                    "DirectComposition scale to {scale:.3} failed; keeping the previous scale: {error:?}"
                                );
                            } else {
                                rs.last_scale = interaction_state.scale_factor;
                                let _ = window.request_inner_size(target_size);
                            }
                        }

                        match sr.set_alpha_enabled(&device, interaction_state.enabled) {
                            Ok(true) => {
                                redraw_requested = true;
                                rs.pacer.request_frame();
                            }
                            Ok(false) => {}
                            Err(error) => {
                                log::warn!("Could not create Alpha R8 readback resources: {error:?}")
                            }
                        }
                        if interaction_state.enabled {
                            update_alpha(
                                &context,
                                sr,
                                &rs.pipeline,
                                &sn.srv,
                                &mut interaction_state,
                                &mut rs.alpha_buf,
                                false,
                            );
                        }

                        if window_visible && recv && rs.pacer.is_due(Instant::now()) {
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
                                let sender_has_new_frame = sn
                                    .frame_counter
                                    .as_mut()
                                    .is_none_or(FrameCounter::is_new_frame);
                                if redraw_requested || sender_has_new_frame {
                                    render_frame(
                                        &context,
                                        &rs.pipeline,
                                        &sn.srv,
                                        &sr.rtv,
                                        sr.width,
                                        sr.height,
                                    );
                                    redraw_requested = false;
                                    frame_update = FrameUpdate::Rendered;

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

                                    if interaction_state.enabled
                                        && !interaction_state.is_dragging()
                                    {
                                        let schedule_extract = interaction_state
                                            .should_update_alpha(Instant::now());
                                        update_alpha(
                                            &context,
                                            sr,
                                            &rs.pipeline,
                                            &sn.srv,
                                            &mut interaction_state,
                                            &mut rs.alpha_buf,
                                            schedule_extract,
                                        );
                                    }
                                }
                            } else {
                                log::trace!("Skipped frame: sender keyed mutex is busy");
                            }
                        }
                    }

                    let presented_frame = frame_update.needs_present();
                    if presented_frame {
                        let Some(sr) = rs.swapchain_res.as_ref() else {
                            return;
                        };
                        let present_result = unsafe { sr.swapchain.Present(1, DXGI_PRESENT(0)) };
                        if present_result.is_err() {
                            log::error!("Swapchain Present failed: {present_result:?}");
                            interaction_state.cleanup(hwnd);
                            elwt.exit();
                            return;
                        }
                        rs.pacer.presented(Instant::now());
                    }

                    let next_wake = rs
                        .pacer
                        .next_wake(Instant::now(), handle_ok && window_visible, presented_frame);
                    if let Some(deadline) = next_wake {
                        elwt.set_control_flow(ControlFlow::WaitUntil(deadline));
                    } else {
                        elwt.set_control_flow(ControlFlow::Poll);
                    }
                }
                _ => {}
            }
        })
        .map_err(|e| format!("EventLoop: {e:?}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{window_should_be_visible, FrameUpdate};

    #[test]
    fn unchanged_backbuffer_is_not_presented() {
        assert!(!FrameUpdate::Unchanged.needs_present());
        assert!(FrameUpdate::Rendered.needs_present());
    }

    #[test]
    fn window_stays_hidden_without_a_sender() {
        assert!(!window_should_be_visible(false, true));
        assert!(window_should_be_visible(true, true));
        assert!(!window_should_be_visible(true, false));
    }
}
