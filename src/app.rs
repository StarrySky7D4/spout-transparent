//! Thin winit lifecycle adapter. Runtime owns application state and scheduling.
mod platform;
mod runtime;
use crate::{
    error::{AppError, ResultContext},
    events::{AppEvent, EventSender},
};
use runtime::Runtime;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::WindowId,
};

struct Application {
    runtime: Option<Runtime>,
    events: EventSender,
    error: Option<AppError>,
}

impl ApplicationHandler<AppEvent> for Application {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        if self.runtime.is_some() {
            return;
        }
        match Runtime::new(elwt, self.events.clone()) {
            Ok(runtime) => self.runtime = Some(runtime),
            Err(error) => {
                self.error = Some(error);
                elwt.exit();
            }
        }
    }

    fn user_event(&mut self, elwt: &ActiveEventLoop, event: AppEvent) {
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        runtime.dispatch(event, elwt);
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        if let Some(runtime) = self.runtime.as_mut() {
            if runtime.window_id() == window_id {
                runtime.window_event(event, elwt);
            }
        }
    }

    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.maintain(elwt);
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(mut runtime) = self.runtime.take() {
            if self.error.is_none() {
                self.error = runtime.take_error();
            }
        }
    }
}

pub fn run() -> Result<(), AppError> {
    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .context("Create event loop")?;
    let mut application = Application {
        runtime: None,
        events: event_loop.create_proxy(),
        error: None,
    };
    event_loop
        .run_app(&mut application)
        .context("Run event loop")?;
    application.error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[path = "app/tests.rs"]
mod tests;
