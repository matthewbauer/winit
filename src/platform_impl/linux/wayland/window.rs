use std::{collections::VecDeque, sync::{Arc, Mutex}};
use raw_window_handle::unix::WaylandHandle;
use smithay_client_toolkit::{
    environment::Environment,
    reexports::client::{
        Display,
        protocol::wl_surface::WlSurface,
    },
    get_surface_outputs,
    get_surface_scale_factor,
    window::{ConceptFrame, Decorations, State as WState},
};
use crate::{
    event::Event,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize, Position, Size},
    error::{ExternalError, NotSupportedError, OsError as RootOsError},
    monitor::MonitorHandle as RootMonitorHandle,
    platform_impl::{
        platform::wayland::event_loop::{available_monitors, primary_monitor},
        MonitorHandle as PlatformMonitorHandle,
        PlatformSpecificWindowBuilderAttributes as AttributesExt,
    },
    window::{CursorIcon, Fullscreen, WindowAttributes},
};
use super::{Update, event_loop::{DispatchData, State, Env}, EventLoopWindowTarget, MonitorHandle, WindowId};

pub fn window_id(surface: &WlSurface) {
    crate::window::WindowId(super::super::WindowId::Wayland(surface.id()))
}

pub fn event<'t, T>(event: crate::event::WindowEvent<'t>, surface: &'t WlSurface) -> crate::event::Event<'t, T> {
    crate::event::Event::WindowEvent{event, window_id: window_id(surface) }
}

pub struct WindowState {
    surface: WlSurface,
    size: (u32, u32),
    current_cursor: &'static str,
    drop: bool,
    title: String,
    fullscreen: bool,
    decorated: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
}

impl PartialEq for WindowState { fn eq(&self, other: &Self) -> bool { self == other.surface } }
impl PartialEq<WlSurface> for WindowState { fn eq(&self, surface: &WlSurface) -> bool { self.surface.id() == surface.id() } }

pub struct WindowHandle {
    display: *mut Display,
    env: Environment<Env>,
    // Arc<Mutex> shared with EventLoop::windows so editions in Configure are reflected back on the handle state for size changes
    windows: Arc<Mutex<Vec<super::event_loop::Window>>>,
    state: WindowState, // Asynchronous state on the handle, sent on the update channel by all setters
    update: smithay_client_toolkit::reexports::calloop::channel::Sender<WindowState>, // Sends user settings changes to EventLoop (allows foreign thread setters)
    // todo: futures::channel ^
}

impl WindowHandle {
    pub fn new<T>(
        state: &EventLoopWindowTarget<T>,
        attributes: WindowAttributes,
        attributes_ext: AttributesExt,
    ) -> Result<Self, RootOsError> {
        let surface = state.env.create_surface_with_scale_callback(
            |scale_factor, surface, mut data| {
                let DispatchData{update: Update{sink}, state: State{windows}} = data.get().unwrap();
                surface.set_buffer_scale(scale_factor);
                for window in windows.find_item(surface).iter_mut() {
                    window.scale_factor = scale_factor;
                    let size = LogicalSize::<f64>::from(window.size).to_physical(scale_factor);
                    sink(event(Event::ScaleFactorChanged{scale_factor, new_inner_size: &mut size}, &surface));
                    window.size = size.to_logical(scale_factor).into();
                }
            }
        );

        let identity_scale_factor = get_surface_scale_factor(&surface); // Always 1.
        let size = attributes.inner_size.map(|size| size.to_logical::<f64>(identity_scale_factor as f64).into()).unwrap_or((800, 600));
        let fullscreen = false;
        let decorated = attributes.decorations;
        let mut window = state.env.create_window::<ConceptFrame, _>(surface.clone(), size, {
            let surface = surface.clone();
            move |event, data| {
                let DispatchData{update: Update{sink}, state} = data.get().unwrap();
                match event {
                    Event::Configure { new_size, states } => {
                        for window in state.windows.find(surface).iter_mut() {
                            window.size = new_size;
                            window.fullscreen = states.contains(&WState::Fullscreen);
                        }
                        sink(event(Event::Resized(LogicalSize::from(new_size).to_physical(get_surface_scale_factor(surface))), &surface));
                    }
                    Event::Refresh => { for window in state.windows.find(surface).iter_mut() { window.refresh(); } }
                    Event::Close => sink(event(Event::CloseRequested, &surface)),
                }
            }
        }).unwrap();

        if let Some(app_id) = attributes_ext.app_id { window.set_app_id(app_id); }
        match attributes.fullscreen {
            Some(Fullscreen::Exclusive(_)) => panic!("Wayland doesn't support exclusive fullscreen"),
            Some(Fullscreen::Borderless(RootMonitorHandle {
                inner: PlatformMonitorHandle::Wayland(ref monitor_id),
            })) => window.set_fullscreen(Some(&monitor_id.0)),
            #[allow(unreachable_patterns)]
            Some(Fullscreen::Borderless(_)) => unreachable!(),
            None => { if attributes.maximized { window.set_maximized(); } }
        }
        window.set_resizable(attributes.resizable);
        window.set_decorate( if attributes.decorations { Decorations::FollowServer } else { Decorations::None });
        window.set_title(attributes.title);
        window.set_min_size(attributes.min_inner_size.map(|size| size.to_logical::<f64>(identity_scale_factor as f64).into()));
        window.set_max_size(attributes.max_inner_size.map(|size| size.to_logical::<f64>(identity_scale_factor as f64).into()));
        state.windows.push(window);
        Ok(Self{surface, display: state.display, size})
    }
    pub fn id(&self) -> WindowId { window_id(self.surface) }
    pub fn set_title(&self, title: &str) {
        self.title = title.into();
        self.update.send(self);
    }
    pub fn set_visible(&self, _visible: bool) { /*todo*/ }
    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>, NotSupportedError> { Err(NotSupportedError::new()) }
    pub fn inner_position(&self) -> Result<PhysicalPosition<i32>, NotSupportedError> { Err(NotSupportedError::new()) }
    pub fn set_outer_position(&self, _pos: Position) { /*todo*/ }
    pub fn inner_size(&self) -> PhysicalSize<u32> { LogicalSize::<f64>::from(self.size).to_physical(self.scale_factor as f64) }
    pub fn request_redraw(&self) { self.need_refresh = true; self.update.send(self); }
    pub fn outer_size(&self) -> PhysicalSize<u32> {
        self.inner_size(); // fixme
    }
    // NOTE: This will only resize the borders, the contents must be updated by the user
    pub fn set_inner_size(&self, size: Size) {
        let scale_factor = self.scale_factor() as f64;
        self.size = size.to_logical::<u32>(scale_factor).into();
    }
    pub fn set_min_inner_size(&self, dimensions: Option<Size>) {
        let scale_factor = self.scale_factor() as f64;
        self.min_size = dimensions.map(|dim| dim.to_logical::<f64>(scale_factor).into());
        self.update.send(self);
    }
    pub fn set_max_inner_size(&self, dimensions: Option<Size>) {
        let scale_factor = self.scale_factor() as f64;
        self.max_size = dimensions.map(|dim| dim.to_logical::<f64>(scale_factor).into());
        self.update.send(self);
    }
    pub fn set_resizable(&self, resizable: bool) { self.resizable = resizable; self.update.send(self); }
    pub fn scale_factor(&self) -> i32 { get_surface_scale_factor(&self.surface) }
    pub fn set_decorations(&self, decorate: bool) { self.decorated= decorate; self.update.send(self); }
    pub fn set_minimized(&self, minimized: bool) { self.minimized = true; self.update.send(self); }
    pub fn set_maximized(&self, maximized: bool) { self.maximized = maximized; self.update.send(self); }
    pub fn fullscreen(&self) -> Option<Fullscreen> {
        if self.fullscreen { Some(Fullscreen::Borderless(RootMonitorHandle {inner: PlatformMonitorHandle::Wayland(self.current_monitor())}))
        } else { None }
    }
    pub fn set_fullscreen(&self, fullscreen: Option<Fullscreen>) {
        match fullscreen {
            Some(Fullscreen::Exclusive(_)) => panic!("Wayland doesn't support exclusive fullscreen"),
            Some(Fullscreen::Borderless(RootMonitorHandle{
                inner: PlatformMonitorHandle::Wayland(ref monitor_id),
            })) => {
                self.fullscreen = monitor_id.0;
            }
            #[allow(unreachable_patterns)]
            Some(Fullscreen::Borderless(_)) => unreachable!(),
            None => self.fullscreen = None,
        }
        self.update.send(self);
    }
    pub fn set_cursor_icon(&self, cursor: CursorIcon) { self.current_cursor = cursor; self.update.send(self); }
    pub fn set_cursor_visible(&self, visible: bool) { self.cursor_visible = visible; self.update.send(self); }
    pub fn set_cursor_grab(&self, grab: bool) -> Result<(), ExternalError> { self.cursor_grab = grab; self.update.send(self); Ok(()) }
    pub fn set_cursor_position(&self, _pos: Position) -> Result<(), ExternalError> { Err(ExternalError::NotSupported(NotSupportedError::new())) }
    pub fn display(&self) -> *mut Display { self.display }
    pub fn surface(&self) -> &WlSurface { &self.surface }
    pub fn current_monitor(&self) -> MonitorHandle { MonitorHandle(get_surface_outputs(&self.surface).last().unwrap().clone()) }
    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> { available_monitors(&self.env) }
    pub fn primary_monitor(&self) -> MonitorHandle { primary_monitor(&self.env) }

    pub fn raw_window_handle(&self) -> WaylandHandle {
        WaylandHandle {
            surface: self.surface().as_ref().c_ptr() as *mut _,
            display: self.display() as *mut _,
            ..WaylandHandle::empty()
        }
    }
}

impl Drop for WindowHandle {
    fn drop(&mut self) {
        self.state.drop = true;
        self.update.send(self);
    }
}
