use std::{collections::VecDeque, fmt, sync::{Arc, Mutex}, time::Instant};
use smithay_client_toolkit::{
    reexports::calloop::{self, channel::{channel as unbounded, Sender, Channel as Receiver}},
    environment::{Environment, SimpleGlobal},
    default_environment,
    reexports::{
        client::{ConnectError, Display, protocol::{wl_output, wl_surface::WlSurface}},
        protocols::unstable::{
            pointer_constraints::v1::client::zwp_pointer_constraints_v1::{ZwpPointerConstraintsV1, Lifetime},
            relative_pointer::v1::client::{
                zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
                zwp_relative_pointer_v1
            }
        },
    },
    output::with_output_info,
    seat::pointer::ThemedPointer,
    window::{Window as SCTKWindow, ConceptFrame, Decorations},
};
use crate::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{DeviceEvent, StartCause, WindowEvent as Event},
    event_loop::{ControlFlow, EventLoopClosed},
    platform_impl::platform,
};
use super::{Update, Sink,  window::{event, WindowState}};

default_environment!{Env, desktop,
    fields = [
        relative_pointer_manager: SimpleGlobal<ZwpRelativePointerManagerV1>,
        pointer_constraints: SimpleGlobal<ZwpPointerConstraintsV1>
    ],
    singles = [
        ZwpRelativePointerManagerV1 => relative_pointer_manager,
        ZwpPointerConstraintsV1 => pointer_constraints
    ]
}

pub struct Window {
    surface: WlSurface,
    size: (u32, u32), scale_factor: u32,
    fullscreen: bool
}

/// Mutable state, time shared by handlers on main thread
pub struct State {
    pub display: Display,
    pub env: Environment<Env>,
    keyboard: super::keyboard::Keyboard,
    pointers: Vec<ThemedPointer>,
    current_cursor: &'static str,
    sctk_windows: Vec<SCTKWindow<ConceptFrame>>,
    windows: Arc<Mutex<Vec<Window>>>,
    update: Sender<WindowState>,
    control_flow: ControlFlow, // for EventLoopWindowTarget
}

// pub impl Deref<..> EventLoop, Window::new(..)
pub struct EventLoopWindowTarget<T> {
    state: &'static mut State,
    _marker: std::marker::PhantomData<T> // Mark whole backend with custom user event type...
}

/*impl<T> EventLoopWindowTarget<T> {
    pub fn display(&self) -> &Display {
        &*self.display
    }
}*/

pub struct EventLoop<T: 'static> {
    state: crate::event_loop::EventLoopWindowTarget<T>, //state: State,
    pub channel: (Sender<T>, Receiver<T>), // EventProxy
    window_target: Option<crate::event_loop::EventLoopWindowTarget<T>>, // crate::EventLoop::Deref -> &EventLoopWindowTarget
}

pub(crate) struct DispatchData<'t, T:'static> {
    update: Update<'t, T>,
    state: &'t mut State,
}

fn sink<T>(sink: &dyn Sink<T>, state: &'static mut State, event: crate::event::Event<T>) {
    sink(
        event,
        &mut crate::event_loop::EventLoopWindowTarget{
            p: crate::platform_impl::EventLoopWindowTarget::Wayland(EventLoopWindowTarget{state, _marker: Default::default() } ),
            _marker: Default::default()
        },
        if state.control_flow == ControlFlow::Exit { &mut ControlFlow::Exit } else { &mut state.control_flow }, // sticky exit
    )
}

impl<T> DispatchData<'static, T> {
    fn sink(&'static self, event: crate::event::Event<T>) { sink(self.update.sink, self.state, event) }
}

impl<T> EventLoop<T> {
    pub fn new() -> Result<Self, ConnectError> {
        let mut event_loop = calloop::EventLoop::<DispatchData<T>>::new().unwrap();

        let (user, receiver) = unbounded();
        /*event_loop.handle().insert_source(receiver, |item:T, _, data:&mut DispatchData<T>| { // calloop::sources::channel::Event<_> ? should be T
            data.sink(crate::event::Event::UserEvent(item));
        });*/

        use smithay_client_toolkit::{init_default_environment, WaylandSource, seat};
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
            let loop_handle = event_loop.handle();

            use seat::{
                pointer::{ThemeManager, ThemeSpec},
                keyboard::{map_keyboard_repeat, RepeatKind},
            };

            let theme_manager = ThemeManager::init(
                ThemeSpec::System,
                env.require_global(),
                env.require_global(),
            );

            let relative_pointer_manager = env.get_global::<ZwpRelativePointerManagerV1>();

            env.listen_for_seats(move |seat, seat_data, mut data| {
                let DispatchData{state:State{pointers, .. }, ..} = data.get().unwrap();
                if seat_data.has_pointer {
                    let pointer = theme_manager.theme_pointer_with_impl(&seat,
                        {
                            let pointer = super::pointer::Pointer::default(); // Track focus and reconstruct scroll events
                            move/*pointer*/ |event, themed_pointer, data| {
                                let DispatchData{update, state:State{sctk_windows, current_cursor, .. }} = data.get().unwrap();
                                pointer.handle(event, themed_pointer, update, sctk_windows, current_cursor);
                            }
                        }
                    );

                    if let Some(manager) = relative_pointer_manager {
                        use zwp_relative_pointer_v1::Event::*;
                        manager.get_relative_pointer(&pointer).quick_assign(move |_, event, data| match event {
                            RelativeMotion { dx, dy, .. } => {
                                let data @ DispatchData{update: Update{sink}, ..} = data.get().unwrap();
                                let device_id = crate::event::DeviceId(super::super::DeviceId::Wayland(super::DeviceId));
                                data.sink(crate::event::Event::DeviceEvent{event: DeviceEvent::MouseMotion { delta: (dx, dy) }, device_id});
                            }
                            _ => unreachable!(),
                        });
                    }

                    pointers.push(pointer);
                }

                if seat_data.has_keyboard {
                    let _ = map_keyboard_repeat(loop_handle, &seat, None, RepeatKind::System,
                        |event, _, data| {
                            let DispatchData{update, state} = data.get().unwrap();
                            state.keyboard.handle(update, event, false);
                        }
                    ).unwrap();
                }

                if seat_data.has_touch {
                    seat.get_touch().quick_assign({
                        let touch = super::touch::Touch::default(); // Track touch points
                        move |_, event, data| {
                            let DispatchData{update: Update{sink}, ..} = data.get().unwrap();
                            touch.handle(sink, event);
                        }
                    });
                }
            });
        };

        // Sync window state
        let (update, receiver) = unbounded();
        event_loop.handle().insert_source(receiver, |state@WindowState{surface}, _, data| {
            let DispatchData{update:Update{sink}, state:State{pointers, windows, sctk_windows, redraw_events}} = data;
            let window = windows.find(surface).unwrap();
            let sctk_window = sctk_windows.find(surface).unwrap();

            if window.size != state.size || window.scale_factor != state.scale_factor {
                redraw_events.push(sink(event(Event::RedrawRequested, window.surface)));
                {let (w, h) = state.size; sctk_window.resize(w, h);}
                sctk_window.refresh();
            }

            if state.decorate { sctk_window.set_decorate(Decorations::FollowServer); }
            else { sctk_window.set_decorate(Decorations::None); }

            if let Some(pointer_constraints) = env.get_global() {
                state.pointer_constraints = pointers.iter().filter(|_| state.grab_cursor).map(
                    |pointer| pointer_constraints.lock_pointer(surface, pointer, None, Lifetime::Persistent.to_raw())
                ).collect();
            }

            if state.drop {
                surface.destroy();
                state.windows.remove_item(surface);
                sink(event(Event::Destroyed, surface));
            }
        });

        Ok(Self{
            state: State{
                display,
                env,
                update,
            },
        })
    }
    // required by linux/mod.rs for crate::EventLoop::Deref
    pub fn window_target(&self) -> &crate::event_loop::EventLoopWindowTarget<T> {
        &self.window_target.get_or_insert( crate::event_loop::EventLoopWindowTarget{p: crate::platform_impl::EventLoopWindowTarget::Wayland(&self.state)} )
    }
}

#[derive(Clone)] pub struct EventLoopProxy<T>(Sender<T>);

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
        let Self{event_loop, state} = self;
        let sink = |event, state| {
            sink(
                event,
                &mut crate::event_loop::EventLoopWindowTarget{p: crate::platform_impl::EventLoopWindowTarget::Wayland(state)},
                if state.control_flow == ControlFlow::Exit { &mut ControlFlow::Exit } else { &mut state.control_flow }, // sticky exit
            )
        };

        sink(Event::NewEvents(StartCause::Init), self.state);

        loop {
            match state.control_flow {
                ControlFlow::Exit => break,
                ControlFlow::Poll => {
                    event_loop.dispatch(std::time::Duration::new(0,0), &mut DispatchData{update: Update{sink}, state: &mut state});
                    sink(Event::NewEvents(StartCause::Poll), state);
                }
                ControlFlow::Wait => {
                    event_loop.dispatch(None, &mut DispatchData{update: Update{sink}, state: &mut state});
                    sink(Event::NewEvents(StartCause::WaitCancelled{start: Instant::now(), requested_resume: None}), state);
                }
                ControlFlow::WaitUntil(deadline) => {
                    let start = Instant::now();
                    let duration = deadline.saturating_duration_since(start);
                    event_loop.dispatch(Some(duration), &mut DispatchData{update: Update{sink}, state: &mut state});

                    let now = Instant::now();
                    if now < deadline {
                        sink(
                            Event::NewEvents(StartCause::WaitCancelled {
                                start,
                                requested_resume: Some(deadline),
                            }),
                            state
                        );
                    } else {
                        sink(
                            Event::NewEvents(StartCause::ResumeTimeReached {
                                start,
                                requested_resume: deadline,
                            }),
                            state
                        );
                    }
                }
            }
            sink(Event::MainEventsCleared, state);
            // sink.send_all(state.redraw_events.drain(..));
            for event in state.redraw_events.drain(..) { sink(event) }
            sink(Event::RedrawEventsCleared, state);
        }
        sink(Event::LoopDestroyed, state);
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
    pub fn monitor(&self) -> crate::monitor::MonitorHandle {
        crate::monitor::MonitorHandle {
            inner: platform::MonitorHandle::Wayland(self.monitor.clone()),
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
    pub fn video_modes(&self) -> impl Iterator<Item = crate::monitor::VideoMode> {
        let monitor = self.clone();

        with_output_info(&self.0, |info| info.modes.clone())
            .unwrap_or(vec![])
            .into_iter()
            .map(move |x| crate::monitor::VideoMode {
                video_mode: platform::VideoMode::Wayland(VideoMode {
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
