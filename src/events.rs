use winit::event_loop::EventLoopProxy;

use crate::frame_pacer::FrameRate;
use crate::spout::SenderName;

#[derive(Debug)]
pub struct PassthroughEvent {
    pub msg: u32,
    pub mouse_data: u32,
    pub screen_pt: windows::Win32::Foundation::POINT,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    ToggleVisibility,
    ToggleInteraction,
    ToggleTopmost,
    ToggleAutoCrop,
    CycleFrameRate,
    SetFrameRate(FrameRate),
    SelectSource(SenderName),
    Quit,
}

#[derive(Debug)]
pub enum AppEvent {
    Action(AppAction),
    DiscoveryChanged,
    Mouse(PassthroughEvent),
}

pub type EventSender = EventLoopProxy<AppEvent>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use winit::event::{Event, StartCause};
    use winit::event_loop::{ControlFlow, EventLoop};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    #[test]
    #[ignore = "Requires the Windows event loop"]
    fn repeated_actions_are_delivered_in_order() {
        let event_loop = EventLoop::<AppEvent>::with_user_event()
            .with_any_thread(true)
            .build()
            .unwrap();
        let proxy = event_loop.create_proxy();
        let expected = vec![
            AppAction::ToggleInteraction,
            AppAction::ToggleInteraction,
            AppAction::ToggleTopmost,
            AppAction::ToggleTopmost,
            AppAction::Quit,
        ];
        let sent = expected.clone();
        let worker = std::thread::spawn(move || {
            for action in sent {
                proxy.send_event(AppEvent::Action(action)).unwrap();
            }
        });
        let received = Rc::new(RefCell::new(Vec::new()));
        let output = Rc::clone(&received);
        let deadline = Instant::now() + Duration::from_secs(2);
        #[allow(deprecated)]
        event_loop
            .run(move |event, elwt| {
                elwt.set_control_flow(ControlFlow::WaitUntil(deadline));
                match event {
                    Event::UserEvent(AppEvent::Action(action)) => {
                        let quit = action == AppAction::Quit;
                        output.borrow_mut().push(action);
                        if quit {
                            elwt.exit();
                        }
                    }
                    Event::NewEvents(StartCause::ResumeTimeReached { .. }) => elwt.exit(),
                    _ => {}
                }
            })
            .unwrap();
        worker.join().unwrap();
        assert_eq!(*received.borrow(), expected);
    }
}
