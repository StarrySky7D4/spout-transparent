//! Coordinates independent deadlines; drawing remains exclusive to RedrawRequested.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use super::platform::{receiver_window_attributes, ComGuard, TaskbarPolicy};
use crate::events::{AppAction, AppEvent, EventSender};
use crate::interaction::hotkeys::HotkeySession;
use crate::interaction::{self, InteractionState};
use crate::tray::{TrayIcon, TrayState};
use spout_transparent::crop::{CropLayout, CropRect};
use spout_transparent::dx::composition::DCompResources;
use spout_transparent::dx::device::create_dx11_device_auto;
use spout_transparent::error::{AppError, ResultContext};
use spout_transparent::frame_pacer::FrameOutcome;
use spout_transparent::renderer::{AlphaReadResult, AlphaSetup, Renderer};
use spout_transparent::scheduler::{Scheduler, Task};
use spout_transparent::source::SourceController;
use spout_transparent::spout::{SenderDiscovery, SenderName};

const READBACK_RETRY: Duration = Duration::from_millis(2);
const ALPHA_SETUP_RETRY: Duration = Duration::from_millis(500);
const LAYOUT_RETRY: Duration = Duration::from_millis(100);

pub(super) fn window_should_be_visible(source_ready: bool, visibility_requested: bool) -> bool {
    source_ready && visibility_requested
}

// Fields are ordered so thread owners and GPU resources are released before the
// window, and the COM apartment outlives every composition object.
pub(super) struct Runtime {
    discovery: SenderDiscovery,
    tray: TrayIcon,
    interaction: InteractionState,
    _hotkeys: HotkeySession,
    _taskbar_policy: TaskbarPolicy,
    dcomp: DCompResources,
    renderer: Renderer,
    source: SourceController,
    scheduler: Scheduler,
    source_ready: bool,
    requested_visible: bool,
    visible: bool,
    redraw_queued: bool,
    senders: Vec<SenderName>,
    menu_source: Option<SenderName>,
    last_tray_state: Option<TrayState>,
    hwnd: HWND,
    window: Window,
    fatal_error: Option<AppError>,
    _com: ComGuard,
}

impl Runtime {
    pub(super) fn window_id(&self) -> WindowId {
        self.window.id()
    }
    pub(super) fn take_error(&mut self) -> Option<AppError> {
        self.fatal_error.take()
    }
    pub(super) fn dispatch(&mut self, event: AppEvent, elwt: &ActiveEventLoop) {
        match event {
            AppEvent::Action(action) => self.action(action, elwt),
            AppEvent::DiscoveryChanged => self.source_event(),
            AppEvent::Mouse(event) => interaction::handle_passthrough_event(&event, self.hwnd),
        }
    }

    pub(super) fn new(elwt: &ActiveEventLoop, events: EventSender) -> Result<Self, AppError> {
        let com = ComGuard::init().context("Initialize COM")?;
        let (device, context) = create_dx11_device_auto().context("Create DX11 device")?;
        let renderer = Renderer::new(device, context).context("Create render pipeline")?;
        let window = elwt
            .create_window(receiver_window_attributes())
            .context("Create receiver window")?;
        let hwnd = match window
            .window_handle()
            .context("Get native window handle")?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return Err(AppError::message("Unsupported window handle")),
        };
        let taskbar_policy = TaskbarPolicy::install(hwnd).context("Keep receiver off taskbar")?;
        InteractionState::init_window_style(hwnd);
        #[cfg(debug_assertions)]
        super::diagnostics::log_window_style(hwnd);
        let tray = TrayIcon::install(events.clone()).context("Install tray")?;
        let dxgi: IDXGIDevice = renderer.device().cast().context("Get DXGI device")?;
        let dcomp = spout_transparent::dx::composition::setup_dcomp(&dxgi, hwnd)
            .context("Create composition")?;
        let mut interaction = InteractionState::new();
        let hotkeys = HotkeySession::install(hwnd, events.clone()).context("Install hotkeys")?;
        interaction
            .install_mouse_hook(hwnd, events.clone())
            .map_err(AppError::message)?;
        let discovery =
            SenderDiscovery::start(move || events.send_event(AppEvent::DiscoveryChanged).is_ok())
                .context("Start sender discovery")?;
        let mut scheduler = Scheduler::default();
        scheduler.set(Task::Source, Some(Instant::now()));
        let mut runtime = Self {
            discovery,
            tray,
            interaction,
            _hotkeys: hotkeys,
            _taskbar_policy: taskbar_policy,
            dcomp,
            renderer,
            source: SourceController::new(),
            scheduler,
            source_ready: false,
            requested_visible: true,
            visible: false,
            redraw_queued: false,
            senders: Vec::new(),
            menu_source: None,
            last_tray_state: None,
            hwnd,
            window,
            fatal_error: None,
            _com: com,
        };
        runtime.tray.update_sources(Vec::new(), None);
        runtime.update_tray();
        log::info!("Event-driven receiver ready; waiting for a Spout sender");
        Ok(runtime)
    }

    fn active(&self) -> bool {
        self.source_ready && self.visible && self.source.matches_installed(self.renderer.identity())
    }

    fn request_frame(&mut self) {
        self.renderer.request_frame();
        if self.active() {
            self.scheduler.set(Task::Frame, Some(Instant::now()));
        }
    }

    fn schedule_alpha_setup(&mut self) {
        let needed = self.interaction.enabled || self.renderer.auto_crop_enabled();
        self.scheduler.set(
            Task::AlphaSetup,
            (self.active() || !needed).then_some(Instant::now()),
        );
        if !needed {
            self.scheduler.set(Task::Readback, None);
        }
    }

    fn source_event(&mut self) {
        if let Some(result) = self.discovery.take_latest() {
            match result {
                Ok(senders) => {
                    self.senders = senders;
                    self.source.update_discovered(self.senders.clone());
                    self.update_menu_source();
                }
                Err(error) => log::warn!("Sender discovery failed: {error}"),
            }
            self.scheduler.set(Task::Source, Some(Instant::now()));
        }
    }

    fn action(&mut self, action: AppAction, elwt: &ActiveEventLoop) {
        match action {
            AppAction::Quit => {
                elwt.exit();
                return;
            }
            AppAction::ToggleVisibility => {
                self.requested_visible = !self.requested_visible;
                self.sync_visibility();
            }
            AppAction::ToggleInteraction => {
                self.interaction.toggle_enabled(self.hwnd);
                self.renderer.cancel_readback();
                self.scheduler.set(Task::Readback, None);
                self.schedule_alpha_setup();
                self.request_frame();
            }
            AppAction::ToggleTopmost => self.interaction.toggle_topmost(self.hwnd),
            AppAction::ToggleAutoCrop => {
                self.renderer.toggle_crop();
                self.schedule_alpha_setup();
                self.scheduler
                    .set(Task::Layout, self.active().then_some(Instant::now()));
                self.request_frame();
            }
            AppAction::CycleFrameRate => {
                let rate = self.renderer.cycle_rate();
                log::info!("Frame rate: {}", rate.display_name());
                self.request_frame();
            }
            AppAction::SetFrameRate(rate) => {
                self.renderer.set_rate(rate);
                self.request_frame();
            }
            AppAction::SelectSource(name) => {
                self.source.select(name);
                self.scheduler.set(Task::Source, Some(Instant::now()));
            }
        }
        self.update_tray();
    }

    fn update_menu_source(&mut self) {
        let name = self.source.identity().map(|identity| identity.name);
        self.tray.update_sources(self.senders.clone(), name.clone());
        self.menu_source = name;
    }

    fn refresh_source(&mut self, now: Instant) {
        if let Err(error) = self.source.poll() {
            log::warn!("Metadata poll failed: {error}");
        }
        let desired = self.source.identity();
        if let Some(identity) = desired.as_ref().filter(|identity| {
            Some(*identity) != self.renderer.identity() && self.source.can_open(identity, now)
        }) {
            let replacement = self
                .renderer
                .prepare(identity.clone())
                .and_then(|candidate| {
                    let (width, height) = candidate.size();
                    let position = self
                        .window
                        .outer_position()
                        .context("Read window position")?;
                    let full = CropRect::full(width, height);
                    let new_layout = CropLayout::new(full, 1.0);
                    self.dcomp
                        .install_content(
                            candidate.swapchain(),
                            self.renderer.swapchain(),
                            self.hwnd,
                            (position.x, position.y),
                            self.renderer.layout(),
                            new_layout,
                        )
                        .context("Install composition")?;
                    Ok(candidate)
                });
            match replacement {
                Ok(candidate) => {
                    self.renderer.install(candidate);
                    self.source.installed();
                    self.interaction.cancel_drag();
                    self.interaction.invalidate_alpha_mask();
                    self.interaction.scale_factor = 1.0;
                    self.interaction.update_crop_layout(self.renderer.layout());
                    self.scheduler.set(Task::Readback, None);
                    self.scheduler.set(Task::AlphaSetup, Some(now));
                    self.request_frame();
                }
                Err(error) => {
                    log::warn!("Sender installation failed: {error}");
                    self.source.failed(identity.clone(), now);
                }
            }
        }
        self.source_ready = desired
            .as_ref()
            .is_some_and(|identity| Some(identity) == self.renderer.identity());
        self.sync_visibility();
        let name = desired.map(|identity| identity.name);
        if name != self.menu_source {
            self.update_menu_source();
        }
        self.scheduler.set(
            Task::Source,
            self.source
                .next_wake(self.renderer.identity(), Instant::now()),
        );
        self.update_tray();
    }

    fn sync_visibility(&mut self) {
        let visible = window_should_be_visible(self.source_ready, self.requested_visible);
        if visible == self.visible {
            return;
        }
        self.visible = visible;
        self.window.set_visible(visible);
        self.interaction.cancel_drag();
        self.interaction.invalidate_alpha_mask();
        self.renderer.cancel_readback();
        self.scheduler.set(Task::Readback, None);
        if visible {
            self.scheduler.set(Task::AlphaSetup, Some(Instant::now()));
            self.scheduler.set(Task::Layout, Some(Instant::now()));
            self.request_frame();
        } else {
            self.scheduler.set(Task::Frame, None);
            self.scheduler.set(Task::Layout, None);
            self.scheduler.set(Task::AlphaSetup, None);
            self.redraw_queued = false;
        }
    }

    fn update_tray(&mut self) {
        let state = TrayState {
            visible: self.visible,
            interaction: self.interaction.enabled,
            topmost: self.interaction.topmost,
            frame_rate: self.renderer.rate(),
            has_source: self.source_ready,
            auto_crop: self.renderer.auto_crop_enabled(),
        };
        if self.last_tray_state != Some(state) {
            self.tray.update_state(state);
            self.last_tray_state = Some(state);
        }
    }

    fn update_layout(&mut self, now: Instant) {
        if !self.active() || self.interaction.is_dragging() {
            return;
        }
        let (width, height) = self.renderer.size();
        self.interaction.constrain_scale(width, height);
        let crop = self.renderer.target_crop(now);
        let scale = self.interaction.scale_factor;
        if self.renderer.viewport_changed(crop, scale) {
            let result = self
                .window
                .outer_position()
                .context("Read window position")
                .and_then(|position| {
                    self.dcomp
                        .apply_layout(
                            self.hwnd,
                            (position.x, position.y),
                            self.renderer.layout(),
                            CropLayout::new(crop, scale),
                        )
                        .context("Update crop layout")
                });
            if let Err(error) = result {
                self.interaction.scale_factor = self.renderer.layout().scale;
                log::warn!("Layout update failed: {error}");
                self.scheduler.set(Task::Layout, Some(now + LAYOUT_RETRY));
                return;
            }
            let layout = self.renderer.commit_viewport(crop, scale);
            self.interaction.update_crop_layout(layout);
        }
        self.scheduler
            .set(Task::Layout, self.renderer.crop_deadline());
    }

    pub(super) fn maintain(&mut self, elwt: &ActiveEventLoop) {
        // Use fresh timestamps between stages: source installation or a driver
        // call may make another independent deadline due during this pass.
        if self.scheduler.take_due(Task::Source, Instant::now()) {
            self.refresh_source(Instant::now());
        }
        if self.scheduler.take_due(Task::AlphaSetup, Instant::now()) {
            self.configure_alpha();
        }
        if self.scheduler.take_due(Task::Readback, Instant::now()) && self.active() {
            self.read_alpha();
        }
        if self.scheduler.take_due(Task::Layout, Instant::now()) {
            self.update_layout(Instant::now());
        }
        let now = Instant::now();
        if self.scheduler.take_due(Task::Frame, now) {
            self.queue_redraw(now);
        }
        self.set_wait(elwt);
    }

    fn configure_alpha(&mut self) {
        let needed = self.interaction.enabled || self.renderer.auto_crop_enabled();
        if !self.active() && needed {
            return;
        }
        match self.renderer.configure_alpha(self.interaction.enabled) {
            Ok(AlphaSetup::Created) => self.request_frame(),
            Ok(AlphaSetup::Unchanged) => {}
            Err(error) => {
                log::warn!("Alpha setup failed: {error:?}");
                self.scheduler
                    .set(Task::AlphaSetup, Some(Instant::now() + ALPHA_SETUP_RETRY));
            }
        }
    }

    fn read_alpha(&mut self) {
        match self
            .renderer
            .read_alpha(self.interaction.enabled, self.interaction.mask_epoch)
        {
            Ok(AlphaReadResult::NotReady) => {}
            Ok(AlphaReadResult::Completed { mask }) => {
                if let Some(sample) = mask {
                    if let Some(buffer) = self.interaction.update_alpha_mask(
                        sample.alpha,
                        sample.width,
                        sample.height,
                    ) {
                        self.renderer.recycle_alpha(buffer);
                    }
                }
                self.scheduler.set(Task::Layout, Some(Instant::now()));
            }
            Err(error) => log::warn!("Alpha readback failed: {error:?}"),
        }
        if self.renderer.has_readback() {
            self.scheduler
                .set(Task::Readback, Some(Instant::now() + READBACK_RETRY));
        }
    }

    fn queue_redraw(&mut self, now: Instant) {
        if !self.active() || self.redraw_queued {
            return;
        }
        if self.renderer.frame_due(now) {
            self.redraw_queued = true;
            self.window.request_redraw();
        } else {
            self.scheduler.set(
                Task::Frame,
                self.renderer.next_frame(now, true, FrameOutcome::Busy),
            );
        }
    }

    fn render(&mut self, elwt: &ActiveEventLoop) {
        self.redraw_queued = false;
        if !self.active() {
            return;
        }
        let now = Instant::now();
        if !self.renderer.frame_due(now) {
            self.scheduler.set(
                Task::Frame,
                self.renderer.next_frame(now, true, FrameOutcome::Busy),
            );
            return;
        }
        match self
            .renderer
            .render(self.interaction.mask_epoch, self.interaction.is_dragging())
        {
            Ok(outcome) => {
                self.scheduler.set(
                    Task::Frame,
                    self.renderer.next_frame(Instant::now(), true, outcome),
                );
                if self.renderer.has_readback() {
                    self.scheduler
                        .set(Task::Readback, Some(Instant::now() + READBACK_RETRY));
                }
            }
            Err(error) => {
                self.fatal_error = Some(AppError::context("Render/Present", error));
                elwt.exit();
            }
        }
    }

    fn set_wait(&self, elwt: &ActiveEventLoop) {
        elwt.set_control_flow(
            self.scheduler
                .next_wake()
                .map_or(ControlFlow::Wait, ControlFlow::WaitUntil),
        );
    }

    pub(super) fn window_event(&mut self, event: WindowEvent, elwt: &ActiveEventLoop) {
        match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::RedrawRequested => self.render(elwt),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                {
                    elwt.exit();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.interaction.handle_scroll(delta);
                self.scheduler
                    .set(Task::Layout, self.active().then_some(Instant::now()));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.interaction
                    .handle_mouse_input(state, button, &self.window);
                if !self.interaction.is_dragging() {
                    self.scheduler
                        .set(Task::Layout, self.active().then_some(Instant::now()));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.interaction.handle_cursor_moved(position, &self.window)
            }
            _ => {}
        }
    }
}
