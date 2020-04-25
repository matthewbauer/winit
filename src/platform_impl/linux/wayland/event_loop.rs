use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use smithay_client_toolkit::{
    reexports::calloop::{EventLoop, channel::{channel as unbounded, Sender, Channel as Receiver}},
    default_environment,
    environment::{Environment, SimpleGlobal},
    init_default_environment,
    reexports::{
        client::{
            ConnectError, Display,
            EventQueue, Attached,
            protocol::{
                wl_output,
                wl_seat::WlSeat,
                wl_surface::WlSurface,
            },
        },
        protocols::unstable::{
            pointer_constraints::v1::client::{
                zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            },
            relative_pointer::v1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        },
    },
    output::with_output_info,
    window::{Window, ConceptFrame},
};
use crate::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{
        DeviceEvent, DeviceId as RootDeviceId, Event as Variant, ModifiersState, StartCause, WindowEvent as Event,
    },
    event_loop::{ControlFlow, EventLoopClosed, EventLoopWindowTarget as WinitData},
    monitor::{MonitorHandle as RootMonitorHandle, VideoMode as RootVideoMode},
    platform_impl::platform::{
        DeviceId as PlatformDeviceId, MonitorHandle as PlatformMonitorHandle,
        VideoMode as PlatformVideoMode, WindowId,
    },
    window::WindowId as NewTypeWindowId, // should be an alias !
};
use super::{
    window::{Window as WindowState, DecorationsAction},
    DeviceId, WaylandWindowId,
};

default_environment!(Env, desktop,
    fields = [
        relative_pointer_manager: SimpleGlobal<ZwpRelativePointerManagerV1>,
        pointer_constraints: SimpleGlobal<ZwpPointerConstraintsV1>
    ],
    singles = [
        ZwpRelativePointerManagerV1 => relative_pointer_manager,
        ZwpPointerConstraintsV1 => pointer_constraints
    ]
);

pub fn wid(window: &Window) { NewTypeWindowId(PlatformWindowId::Wayland(window.surface.id()) }
pub fn window_event(event: WindowEvent<'static>, surface: &WlSurface) -> Event {
    Event { event, window_id: wid(surface.id()) }
}

/// Mutable state, time shared by handlers on main thread
pub struct State {
    pub display: Display,
    pub env: Environment<Env>,
    keyboard: super::keyboard::Keyboard,
    pointers: Vec<super::pointer::Pointer>,
    windows: Vec<Window<ConceptFrame>>,
    window_states: Vec<Weak<Mutex<WindowState>>>, // Configure
    update: Sender<WindowState>,
}

// pub impl Deref<..> EventLoop, Window::new(..)
type EventLoopWindowTarget<T> = State; // +Marker?

/*impl<T> EventLoopWindowTarget<T> {
    pub fn display(&self) -> &Display {
        &*self.display
    }
}*/

pub struct EventLoop<T: 'static> {
    state: crate::EventLoopWindowTarget<T>, //state: State,
    pub channel: (Sender<T>, Receiver<T>), // EventProxy
}

impl<T> EventLoop<T> {
    // crate::EventLoop::Deref
    pub fn window_target(&self) -> &crate::EventLoopWindowTarget<T> {
        &self.state
    }

    pub fn new() -> Result<Self<T>, ConnectError> {
        struct DispatchData<'t, T> {
            frame: Frame<S>,
            state: &'t mut State,
        }
        let mut event_loop = calloop::EventLoop::<DispatchData<T>>::new().unwrap();

        let (waker, receiver) = unbounded();
        event_loop.insert_source(receiver, |id, _, state| {

        use smithay_client_toolkit::{default_environment, init_default_environment, WaylandSource, seat};
        default_environment!(Env, desktop);
        let (env, display, queue) = init_default_environment!(
            Env,
            desktop,
            fields = [
                relative_pointer_manager: SimpleGlobal::new(),
                pointer_constraints: SimpleGlobal::new()
            ]
        )?;
        WaylandSource::new(queue)
            .quick_insert(event_loop.handle())
            .unwrap();

        let seat_handler = { // for a simple setup
            use seat::{
                pointer::{ThemeManager, ThemeSpec},
                keyboard::{map_keyboard, RepeatKind},
            };

            let theme_manager = ThemeManager::init(
                ThemeSpec::System,
                env.require_global(),
                env.require_global(),
            );

            let relative_pointer_manager = env.get_global::<ZwpRelativePointerManagerV1>();

            env.listen_for_seats(move |seat, seat_data, mut data| {
                let DispatchData{state:State{pointers, .. }} = data.get().unwrap();
                if seat_data.has_pointer {
                    let pointer = theme_manager.theme_pointer_with_impl(&seat,
                        {
                            let pointer = super::pointer::Pointer::default(); // Track focus and reconstruct scroll events
                            move/*pointer*/ |event, themed_pointer, data| {
                                let DispatchData{frame, state:State{ window, current_cursor, .. }} = data.get().unwrap();
                                pointer.handle(event, themed_pointer, frame, window, current_cursor);
                            }
                        }
                    ).unwrap();

                    if let Some(manager) = relative_pointer_manager {
                        manager.get_relative_pointer(pointer).quick_assign(move |_, event, data| match event {
                            Event::RelativeMotion { dx, dy, .. } => {
                                let Context{frame: Frame{sink}} = data.get().unwrap();
                                sink(Event::DeviceEvent {
                                    event: DeviceEvent::MouseMotion { delta: (dx, dy) }
                                    device_id: RootDeviceId(PlatformDeviceId::Wayland(DeviceId)),
                                }
                            }
                            _ => unreachable!(),
                        });
                    }

                    pointers.push(pointer);
                }

                if seat_data.has_keyboard {
                    let _ = map_keyboard(&seat, None, RepeatKind::System,
                        |event, _, data| {
                            let DispatchData{frame, state} = data.get().unwrap();
                            state.keyboard.handle(event, frame);
                        }
                    ).unwrap();
                }

                if seat_data.has_touch {
                    seat.get_touch().quick_assign({
                        let touch = super::touch::Touch::default(); // Track touch points
                        move |_, event, data| {
                            let DispatchData{frame, ..} = data.get().unwrap();
                            touch.handle(event, frame);
                        }
                    }).unwrap();
                }
            });
        };

        // Sync window state
        let (update, receiver) = unbounded();
        event_loop.insert_source(receiver, |id, _, data| {
            let DispatchData{frame: Frame{sink}, state} = data;
            let state = state.window_states.find(id);
            if let Some(state) = state && let Some(state) = state.upgrade() {
                if let Some(window) = state.windows.find(id) {
                    if state.decorate {
                        window.set_decorate(window::Decorations::FollowServer);
                    } else {
                        window.set_decorate(window::Decorations::None);
                    }

                    {let (w, h) = window.size; window.resize(w, h);}
                    window.refresh();
                }
                if let Some(pointer_constraints) = env.get_global() {
                    state.pointer_constraints = self.pointers.iter().filter(|_| state.grab_cursor).map(
                        |pointer| pointer_constraints.lock_pointer(surface, pointer, None, Lifetime::Persistent.to_raw())
                    ).collect();
                }
            } else {
                sink(Event{ window_id: wid(&window.surface), event: WindowEvent::Destroyed });
                if let Some(window) = state.windows.remove_item(id) { window.surface().destroy(); }
            }
        });

        Ok(Self{
            state: State{
                display
                env
                update,
                sink,
                ..//Default::default()
            },
            waker
        })
    }
}

#[derive(Clone)] struct EventLoopProxy<T>(Sender<T>);

impl<T: 'static> EventLoopProxy<T> {
    pub fn send_event(&self, event: T) -> Result<(), EventLoopClosed<T>> {
        self.user_sender.send(event).map_err(|e| {
            EventLoopClosed(if let std::sync::mpsc::SendError(x) = e {
                x
            } else {
                unreachable!()
            })
        })
    }
}

impl<T> EventLoop<T> {
    pub fn create_proxy(&self) -> EventLoopProxy<T> { self.channel.0.clone() }

    pub fn run<S:Sink<T>>(mut self, sink: S) -> ! {
        self.run_return(sink);
        std::process::exit(0);
    }

    pub fn run_return<S:Sink<T>>(&mut self, mut sink: S) {
        let yield = |event, state| {
            sink(
                event,
                &mut crate::EventLoopWindowTarget{p: crate::platform_impl::EventLoopWindowTarget::Wayland(state), ../*Default::default()*/ },
                if state.control_flow == ControlFlow::Exit { &mut ControlFlow::Exit } else { &mut control_flow }, // sticky exit
            )
        };

        yield(Event::NewEvents(StartCause::Init), state);

        loop {
            match control_flow {
                ControlFlow::Exit => break,
                ControlFlow::Poll => {
                    event_loop.dispatch(std::time::Duration(0,0), &mut DispatchData{frame: Frame{sink}, state: &mut state});
                    yield(Event::NewEvents(StartCause::Poll), state);
                }
                ControlFlow::Wait => {
                    event_loop.dispatch(None, &mut DispatchData{frame: Frame{sink}, state: &mut state});
                    yield(Event::NewEvents(StartCause::WaitCancelled{start: Instant::now(), requested_resume: None}, state);
                }
                ControlFlow::WaitUntil(deadline) => {
                    let start = Instant::now();
                    let duration = deadline.saturating_duration_since(start);
                    event_loop.dispatch(Some(duration), &mut DispatchData{frame: Frame{sink}, state: &mut state});

                    let now = Instant::now();
                    if now < deadline {
                        yield(
                            Event::NewEvents(StartCause::WaitCancelled {
                                start,
                                requested_resume: Some(deadline),
                            }),
                            state
                        );
                    } else {
                        callback(
                            Event::NewEvents(StartCause::ResumeTimeReached {
                                start,
                                requested_resume: deadline,
                            }),
                            state
                        );
                    }
                }
            }
            yield(Event::MainEventsCleared, state);
            for surface in state.redraw_requests {
                if redraw_requested {
                    windows.find(surface).refresh();
                    yield(Event::RedrawRequested(wid(surface), state);
                }
            }
            yield(Event::RedrawEventsCleared, state);
        }
        yield(Event::LoopDestroyed, state);
    }

    pub fn primary_monitor(&self) -> MonitorHandle {
        primary_monitor(&self.env.lock().unwrap())
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        available_monitors(&self.env.lock().unwrap())
    }
}

/*
 * Monitor stuff
 */

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VideoMode {
    pub(crate) size: (u32, u32),
    pub(crate) bit_depth: u16,
    pub(crate) refresh_rate: u16,
    pub(crate) monitor: MonitorHandle,
}

impl VideoMode {
    #[inline]
    pub fn size(&self) -> PhysicalSize<u32> {
        self.size.into()
    }

    #[inline]
    pub fn bit_depth(&self) -> u16 {
        self.bit_depth
    }

    #[inline]
    pub fn refresh_rate(&self) -> u16 {
        self.refresh_rate
    }

    #[inline]
    pub fn monitor(&self) -> RootMonitorHandle {
        RootMonitorHandle {
            inner: PlatformMonitorHandle::Wayland(self.monitor.clone()),
        }
    }
}

#[derive(Clone)]
pub struct MonitorHandle(pub(crate) wl_output::WlOutput);

impl PartialEq for MonitorHandle {
    fn eq(&self, other: &Self) -> bool {
        self.native_identifier() == other.native_identifier()
    }
}

impl Eq for MonitorHandle {}

impl PartialOrd for MonitorHandle {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl Ord for MonitorHandle {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.native_identifier().cmp(&other.native_identifier())
    }
}

impl std::hash::Hash for MonitorHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.native_identifier().hash(state);
    }
}

impl fmt::Debug for MonitorHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[derive(Debug)]
        struct MonitorHandle {
            name: Option<String>,
            native_identifier: u32,
            size: PhysicalSize<u32>,
            position: PhysicalPosition<i32>,
            scale_factor: i32,
        }

        let monitor_id_proxy = MonitorHandle {
            name: self.name(),
            native_identifier: self.native_identifier(),
            size: self.size(),
            position: self.position(),
            scale_factor: self.scale_factor(),
        };

        monitor_id_proxy.fmt(f)
    }
}

impl MonitorHandle {
    pub fn name(&self) -> Option<String> {
        with_output_info(&self.0, |info| format!("{} ({})", info.model, info.make))
    }

    #[inline]
    pub fn native_identifier(&self) -> u32 {
        with_output_info(&self.0, |info| info.id).unwrap_or(0)
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        match with_output_info(&self.0, |info| {
            info.modes
                .iter()
                .find(|m| m.is_current)
                .map(|m| m.dimensions)
        }) {
            Some(Some((w, h))) => (w as u32, h as u32),
            _ => (0, 0),
        }
        .into()
    }

    pub fn position(&self) -> PhysicalPosition<i32> {
        with_output_info(&self.0, |info| info.location)
            .unwrap_or((0, 0))
            .into()
    }

    #[inline]
    pub fn scale_factor(&self) -> i32 {
        with_output_info(&self.0, |info| info.scale_factor).unwrap_or(1)
    }

    #[inline]
    pub fn video_modes(&self) -> impl Iterator<Item = RootVideoMode> {
        let monitor = self.clone();

        with_output_info(&self.0, |info| info.modes.clone())
            .unwrap_or(vec![])
            .into_iter()
            .map(move |x| RootVideoMode {
                video_mode: PlatformVideoMode::Wayland(VideoMode {
                    size: (x.dimensions.0 as u32, x.dimensions.1 as u32),
                    refresh_rate: (x.refresh_rate as f32 / 1000.0).round() as u16,
                    bit_depth: 32,
                    monitor: monitor.clone(),
                }),
            })
    }
}

pub fn primary_monitor(env: &Environment<Env>) -> MonitorHandle {
    MonitorHandle(
        env.get_all_outputs()
            .first()
            .expect("No monitor is available.")
            .clone(),
    )
}

pub fn available_monitors(env: &Environment<Env>) -> VecDeque<MonitorHandle> {
    env.get_all_outputs()
        .iter()
        .map(|proxy| MonitorHandle(proxy.clone()))
        .collect()
}
