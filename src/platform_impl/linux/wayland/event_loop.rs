use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    reexports::calloop::{self, channel::{channel as unbounded, Sender, Channel as Receiver}},
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
    window,
};

use crate::{
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{
        DeviceEvent, DeviceId as RootDeviceId, Event, ModifiersState, StartCause, WindowEvent,
    },
    event_loop::{ControlFlow, EventLoopClosed, EventLoopWindowTarget as RootELW},
    monitor::{MonitorHandle as RootMonitorHandle, VideoMode as RootVideoMode},
    platform_impl::platform::{
        sticky_exit_callback/*?*/ as callback_wrapped, DeviceId as PlatformDeviceId, MonitorHandle as PlatformMonitorHandle,
        VideoMode as PlatformVideoMode, WindowId as PlatformWindowId,
    },
    window::{WindowId as RootWindowId},
};

use super::{
    window::{DecorationsAction},
    DeviceId, WindowId,
    cursor::CursorManager,
};

#[derive(Clone)]
pub struct EventsSink {
    sender: Sender<Event<'static, ()>>,
}

impl EventsSink {
    pub fn new(sender: Sender<Event<'static, ()>>) -> EventsSink {
        EventsSink { sender }
    }

    pub fn send_event(&self, event: Event<'static, ()>) {
        self.sender.send(event).unwrap()
    }

    pub fn send_device_event(&self, event: DeviceEvent, device_id: DeviceId) {
        self.send_event(Event::DeviceEvent {
            event,
            device_id: RootDeviceId(PlatformDeviceId::Wayland(device_id)),
        });
    }

    pub fn send_window_event(&self, event: WindowEvent<'static>, window_id: WindowId) {
        self.send_event(Event::WindowEvent {
            event,
            window_id: RootWindowId(PlatformWindowId::Wayland(window_id)),
        });
    }
}

pub struct EventLoop<T: 'static> {
    user_channel: calloop::channel::Channel<T>,
    pub event_loop: calloop::EventLoop<DispatchData<T>>,
    pub env: Arc<Mutex<Environment<Env>>>,
    cursor_manager: Arc<Mutex<CursorManager>>,
    window_target: RootELW<T>,
}

// A handle that can be sent across threads and used to wake up the `EventLoop`.
//
// We should only try and wake up the `EventLoop` if it still exists, so we hold Weak ptrs.
pub struct EventLoopProxy<T: 'static> {
    user_sender: Sender<T>,
}

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

pub struct EventLoopWindowTarget<T: 'static> {
    pub env: Environment<Env>,
    pub cursor_manager: Arc<Mutex<CursorManager>>,
    pub modifiers_tracker: Arc<Mutex<ModifiersState>>,
    // A cleanup switch to prune dead windows
    pub cleanup_needed: Arc<Mutex<bool>>,
    // The wayland display
    pub display: Arc<Display>,
    _marker: ::std::marker::PhantomData<T>,
}
type DispatchData<T> = EventLoopWindowTarget<T>;

impl<T: 'static> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        EventLoopProxy {
            user_sender: self.user_sender.clone(),
        }
    }
}

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

impl<T: 'static> EventLoop<T> {
    pub fn new() -> Result<EventLoop<T>, ConnectError> {
        let mut event_loop = calloop::EventLoop::<DispatchData<T>>::new().unwrap();

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


        let cursor_manager = Arc::new(Mutex::new(CursorManager::new(env)));

        /// Mutable state time shared by stream handlers on main thread
        struct State {
            keyboard: super::keyboard::Keyboard,
            pointers: Vec<super::pointer::Pointer>,

            window: Window<ConceptFrame>,
            current_cursor: &'static str,
            scale_factor: u32,
            size: (u32, u32),
            resized: bool,
            need_refresh: bool,
        }
        //
        struct DispatchData<'t, St:Stream+Unpin> {
            frame: &'t mut Frame<'t, St>,
            state: &'t mut State,
        }

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
                let DispatchData{state:State{pointer, .. }} = data.get().unwrap();
                if seat_data.has_pointer {
                    pointers.push(theme_manager.theme_pointer_with_impl(&seat,
                        {
                            let pointer = super::pointer::Pointer::default(); // Track focus and reconstruct scroll events
                            move/*pointer*/ |event, themed_pointer, data| {
                                let DispatchData{frame, state:State{ window, current_cursor, .. }} = data.get().unwrap();
                                pointer.handle(event, themed_pointer, frame, window, current_cursor);
                            }
                        }
                    ).unwrap());

                    if Some(manager) = relative_pointer_manager {
                        manager.get_relative_pointer(pointer).quick_assign(move |_, event, data| match event {
                            Event::RelativeMotion { dx, dy, .. } => {
                                data.get().unwrap().sink.send_device_event(DeviceEvent::MouseMotion { delta: (dx, dy) }, DeviceId)
                            }
                            _ => unreachable!(),
                        });
                    }
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
                        |_, event, data| {
                            let DispatchData{frame:Frame{sink}, ..} = data.get().unwrap();
                            state.touch.handle(event, sink);
                        }
                    }).unwrap();
                }
            });
        };

        let display = Arc::new(display);
        Ok(EventLoop {
            channel: unbounded(),
            display: display.clone(),
            env: Arc::new(Mutex::new(env.clone())),
            cursor_manager: cursor_manager.clone(), // grab
            window_target: RootELW {
                p: crate::platform_impl::EventLoopWindowTarget::Wayland(DispatchData {
                    store,
                    env,
                    cleanup_needed: Arc::new(Mutex::new(false)),
                    display,
                    _marker: ::std::marker::PhantomData,
                }),
                _marker: ::std::marker::PhantomData,
            },
        })
    }

    pub fn create_proxy(&self) -> EventLoopProxy<T> {
        EventLoopProxy {
            user_sender: self.user_sender.clone(),
        }
    }

    pub fn run<F>(mut self, callback: F) -> !
    where
        F: 'static + FnMut(Event<'_, T>, &RootELW<T>, &mut ControlFlow),
    {
        self.run_return(callback);
        std::process::exit(0);
    }

    pub fn run_return<F>(&mut self, mut callback: F)
    where
        F: FnMut(Event<'_, T>, &RootELW<T>, &mut ControlFlow),
    {
        // send pending events to the server
        self.display.flush().expect("Wayland connection lost.");

        let mut control_flow = ControlFlow::default();

        callback(
            Event::NewEvents(StartCause::Init),
            &self.window_target,
            &mut control_flow,
        );

        loop {
            self.event_loop.dispatch_pending(&mut get_target(&self.window_target), |_, _, _| {}).expect("Wayland connection lost.");

            // send pending events to the server
            self.display.flush().expect("Wayland connection lost.");

            // During the run of the user callback, some other code monitoring and reading the
            // wayland socket may have been run (mesa for example does this with vsync), if that
            // is the case, some events may have been enqueued in our event queue.
            //
            // If some messages are there, the event loop needs to behave as if it was instantly
            // woken up by messages arriving from the wayland socket, to avoid getting stuck.
            let instant_wakeup = {
                 /*let window_target = match self.window_target.p {
                     crate::platform_impl::EventLoopWindowTarget::Wayland(ref wt) => wt,
                     _ => unreachable!(),
                 };*/
                 let dispatched = get_target(&self.window_target) //window_target
                     .queue
                     .borrow_mut()
                     .dispatch_pending(&mut (), |_, _, _| {})
                     .expect("Wayland connection lost.");
                 dispatched > 0
            };

            // send Events cleared
            callback_wrapped(
                    Event::MainEventsCleared,
                    &self.window_target,
                    &mut control_flow,
                    &mut callback,
                );

            // handle request-redraw
            self.redraw_triggers(|wid, window_target| {
                    callback_wrapped(
                        Event::RedrawRequested(crate::window::WindowId(
                            crate::platform_impl::WindowId::Wayland(wid),
                        )),
                        window_target,
                        &mut control_flow,
                        &mut callback,
                    );
                });

            // send RedrawEventsCleared
            callback_wrapped(
                    Event::RedrawEventsCleared,
                    &self.window_target,
                    &mut control_flow,
                    &mut callback,
                );

            match control_flow {
                ControlFlow::Exit => self.event_loop.get_signal().stop(),
                ControlFlow::Poll => {
                    self.event_loop.dispatch_pending(Some(Duration::new(0,0)), &mut get_target(&self.window_target));

                    callback(
                        Event::NewEvents(StartCause::Poll),
                        &self.window_target,
                        &mut control_flow,
                    );
                }
                ControlFlow::Wait => {
                    if !instant_wakeup {
                        self.event_loop.dispatch(None, &mut get_target(&self.window_target));
                    }

                    callback(
                        Event::NewEvents(StartCause::WaitCancelled {
                            start: Instant::now(),
                            requested_resume: None,
                        }),
                        &self.window_target,
                        &mut control_flow,
                    );
                }
                ControlFlow::WaitUntil(deadline) => {
                    let start = Instant::now();
                    // compute the blocking duration
                    let duration = if deadline > start && !instant_wakeup {
                        deadline - start
                    } else {
                        Duration::from_millis(0)
                    };
                    self.event_loop.dispatch(Some(duration), &mut get_target(&self.window_target));

                    let now = Instant::now();
                    if now < deadline {
                        callback(
                            Event::NewEvents(StartCause::WaitCancelled {
                                start,
                                requested_resume: Some(deadline),
                            }),
                            &self.window_target,
                            &mut control_flow,
                        );
                    } else {
                        callback(
                            Event::NewEvents(StartCause::ResumeTimeReached {
                                start,
                                requested_resume: deadline,
                            }),
                            &self.window_target,
                            &mut control_flow,
                        );
                    }
                }
            }
        }

        callback(Event::LoopDestroyed, &self.window_target, &mut control_flow);
    }

    pub fn primary_monitor(&self) -> MonitorHandle {
        primary_monitor(&self.env.lock().unwrap())
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        available_monitors(&self.env.lock().unwrap())
    }

    pub fn window_target(&self) -> &RootELW<T> {
        &self.window_target
    }
}

impl<T> EventLoopWindowTarget<T> {
    pub fn display(&self) -> &Display {
        &*self.display
    }
}

/*
 * Private EventLoop Internals
 */

impl<T> EventLoop<T> {
    fn redraw_triggers<F>(&mut self, mut callback: F)
    where
        F: FnMut(WindowId, &RootELW<T>),
    {
        let window_target = match self.window_target.p {
            crate::platform_impl::EventLoopWindowTarget::Wayland(ref data) => data,
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        };
        window_target.store.lock().unwrap().for_each_redraw_trigger(
            |refresh, frame_refresh, wid, frame| {
                if let Some(frame) = frame {
                    if frame_refresh {
                        frame.refresh();
                        if !refresh {
                            frame.surface().commit()
                        }
                    }
                }
                if refresh {
                    callback(wid, &self.window_target);
                }
            },
        )
    }

    fn post_dispatch_triggers<F>(&mut self, mut callback: F, control_flow: &mut ControlFlow)
    where
        F: FnMut(Event<'_, T>, &RootELW<T>, &mut ControlFlow),
    {
        let window_target = match self.window_target.p {
            crate::platform_impl::EventLoopWindowTarget::Wayland(ref wt) => wt,
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        };

        let mut callback = |event: Event<'_, T>| {
            callback_wrapped(event, &self.window_target, control_flow, &mut callback);
        };

        // prune possible dead windows
        {
            let mut cleanup_needed = window_target.cleanup_needed.lock().unwrap();
            if *cleanup_needed {
                let pruned = window_target.store.lock().unwrap().cleanup();
                *cleanup_needed = false;
                for wid in pruned {
                    callback(Event::WindowEvent {
                        window_id: crate::window::WindowId(
                            crate::platform_impl::WindowId::Wayland(wid),
                        ),
                        event: WindowEvent::Destroyed,
                    });
                }
            }
        }
        // process pending resize/refresh
        window_target.store.lock().unwrap().for_each(|window| {
            let window_id =
                crate::window::WindowId(crate::platform_impl::WindowId::Wayland(window.wid));

            // Update window logical .size field (for callbacks using .inner_size)
            let (old_logical_size, mut logical_size) = {
                let mut window_size = window.size.lock().unwrap();
                let old_logical_size = *window_size;
                *window_size = window.new_size.unwrap_or(old_logical_size);
                (old_logical_size, *window_size)
            };

            if let Some(scale_factor) = window.new_scale_factor {
                // Update cursor scale factor
                let new_logical_size = {
                    let scale_factor = scale_factor as f64;
                    let mut physical_size =
                        LogicalSize::<f64>::from(logical_size).to_physical(scale_factor);
                    callback(Event::WindowEvent {
                        window_id,
                        event: WindowEvent::ScaleFactorChanged {
                            scale_factor,
                            new_inner_size: &mut physical_size,
                        },
                    });
                    physical_size.to_logical::<u32>(scale_factor).into()
                };
                // Update size if changed by callback
                if new_logical_size != logical_size {
                    logical_size = new_logical_size;
                    *window.size.lock().unwrap() = logical_size.into();
                }
            }

            if window.new_size.is_some() || window.new_scale_factor.is_some() {
                if let Some(frame) = window.frame {
                    // Update decorations state
                    match window.decorations_action {
                        Some(DecorationsAction::Hide) => {
                            frame.set_decorate(window::Decorations::None)
                        }
                        Some(DecorationsAction::Show) => {
                            frame.set_decorate(window::Decorations::FollowServer)
                        }
                        None => (),
                    }

                    // mutter (GNOME Wayland) relies on `set_geometry` to reposition window in case
                    // it overlaps mutter's `bounding box`, so we can't avoid this resize call,
                    // which calls `set_geometry` under the hood, for now.
                    let (w, h) = logical_size;
                    frame.resize(w, h);
                    frame.refresh();
                }
                // Don't send resize event downstream if the new logical size and scale is identical to the
                // current one
                if logical_size != old_logical_size || window.new_scale_factor.is_some() {
                    let physical_size = LogicalSize::<f64>::from(logical_size).to_physical(
                        window.new_scale_factor.unwrap_or(window.prev_scale_factor) as f64,
                    );
                    callback(Event::WindowEvent {
                        window_id,
                        event: WindowEvent::Resized(physical_size),
                    });
                }
            }

            if window.closed {
                callback(Event::WindowEvent {
                    window_id,
                    event: WindowEvent::CloseRequested,
                });
            }

            // Update grab
            if let Some(grab_cursor) = window.grab_cursor {
                let surface = if grab_cursor {
                    Some(window.surface) // Grab
                } else {
                    None // Release
                };
                self.cursor_manager.lock().unwrap().grab_pointer(surface);
            }
        })
    }
}

fn get_target<T>(target: &RootELW<T>) -> &EventLoopWindowTarget<T> {
    match target.p {
        crate::platform_impl::EventLoopWindowTarget::Wayland(ref wt) => wt,
        #[allow(unreachable_patterns)]
        _ => unreachable!(),
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
