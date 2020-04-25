use std::{
    collections::VecDeque,
    mem::replace,
    sync::{Arc, Mutex, Weak},
};
use raw_window_handle::unix::WaylandHandle;
use smithay_client_toolkit::{
    environment::Environment,
    reexports::client::{
        Display,
        protocol::wl_surface::WlSurface,
    },
    get_surface_outputs,
    get_surface_scale_factor,
    window::{
        ConceptFrame, Decorations, Event as WEvent, State as WState,
        Window as SCTKWindow,
    },
};
use crate::{
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
use super::{
    event_loop::Env,
    EventLoopWindowTarget, MonitorHandle, WindowId,
};

pub struct WindowState {
    surface: WlSurface,
    size: (u32, u32),
    title: String
    current_cursor: &'static str,
    scale_factor: u32,
    resized: bool,
    need_refresh: bool,
    fullscreen: bool,
    decorated: bool,
    min_size: (u32, u32),
    max_size: (u32, u32),
}

impl PartialEq<WlSurface> for WindowState {
    fn eq(&self, surface: &WlSurface) -> bool {
        self.surface.id() == surface.id()()
    }
}
impl PartialEq for WindowState {
    fn eq(&self, other: &Self) -> bool {
        self == other.surface
    }
}

pub struct WindowHandle {
    display: *Display,
    env: Environment<Env>,
    state: Arc<Mutex<WindowState>>, // Arc<Mutex> so EventLoop::window_states editions reflect back on the handle
    update: Sender<u32>, // Wakes up EventLoop if configuring Window from another thread
}
type Window = WindowHandle;

#[derive(Clone, Copy, Debug)]
pub enum DecorationsAction {
    Hide,
    Show,
}

impl Window {
    pub fn new<T>(
        state: &EventLoopWindowTarget<T>,
        attributes: WindowAttributes,
        attributes_ext: AttributesExt,
    ) -> Result<Self, RootOsError> {
        let surface = state.env.create_surface_with_scale_callback(
            |scale, surface, mut data| {
                let DispatchData{frame:Frame{sink}, state:State{window_states}} = data.get().unwrap();
                surface.set_buffer_scale(scale);
                for window in states.find_item(window).iter_mut() {
                    window.scale_factor = scale;
                    let size = LogicalSize::<f64>::from(window.size).to_physical(scale);
                    sink(Event::WindowEvent {
                        window_id: wid(&surface),
                        event: WindowEvent::ScaleFactorChanged {
                            scale_factor,
                            new_inner_size: &mut size,
                        },
                    });
                    window.size = size.to_logical(scale).into();
                }
            }
        );

        let scale_factor = get_surface_scale_factor(&surface); // Always 1.
        let size = attributes
            .inner_size
            .map(|size| size.to_logical::<f64>(scale_factor as f64).into())
            .unwrap_or((800, 600));
        let fullscreen = false;
        let decorated = attributes.decorations;
        let mut window = state.env.create_window::<ConceptFrame, _>(surface.clone(), size, {
            let surface = surface.clone();
            move |event, data| {
                let DispatchData{frame: Frame{sink}, state} = data.get().unwrap();
                match event {
                    Event::Configure { new_size, states } => {
                        sink(Event::WindowEvent { window_id: wid(surface), event: WindowEvent::Resized(LogicalSize::from(new_size).to_physical(scale))});
                        for window in state.window_states.find(surface).iter_mut() {
                            window.size = new_size;
                            window.fullscreen = states.contains(&WState::Fullscreen);
                            window.need_refresh = true;
                            state.update.send(wid(surface));
                        }
                    }
                    Event::Refresh => {
                        for window in state.window_states.find(surface).iter_mut() {
                            window.need_refresh = true;
                            state.update.send(wid(surface));
                        }
                    }
                    Event::Close => {
                        sink(Event::WindowEvent{ window_id: wid(surface), event: WindowEvent::CloseRequested });
                    }
                }
            }
        )
        .unwrap();

        if let Some(app_id) = attributes_ext.app_id {
            window.set_app_id(app_id);
        }

        // Check for fullscreen requirements
        match attributes.fullscreen {
            Some(Fullscreen::Exclusive(_)) => {
                panic!("Wayland doesn't support exclusive fullscreen")
            }
            Some(Fullscreen::Borderless(RootMonitorHandle {
                inner: PlatformMonitorHandle::Wayland(ref monitor_id),
            })) => window.set_fullscreen(Some(&monitor_id.0)),
            #[allow(unreachable_patterns)]
            Some(Fullscreen::Borderless(_)) => unreachable!(),
            None => {
                if attributes.maximized {
                    window.set_maximized();
                }
            }
        }

        window.set_resizable(attributes.resizable);

        // set decorations
        window.set_decorate(if attributes.decorations {
            Decorations::FollowServer
        } else {
            Decorations::None
        });

        // set title
        window.set_title(attributes.title);

        // min-max dimensions
        window.set_min_size(
            attributes
                .min_inner_size
                .map(|size| size.to_logical::<f64>(scale_factor as f64).into()),
        );
        window.set_max_size(
            attributes
                .max_inner_size
                .map(|size| size.to_logical::<f64>(scale_factor as f64).into()),
        );

        state.windows.push(window);
        Ok(Window{display, id, size})
    }

    #[inline]
    pub fn id(&self) -> WindowId {
        id
    }

    pub fn set_title(&self, title: &str) {
        self.title = title.into();
        self.update.send(self);
    }

    pub fn set_visible(&self, _visible: bool) {
        // TODO
    }

    #[inline]
    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>, NotSupportedError> {
        Err(NotSupportedError::new())
    }

    #[inline]
    pub fn inner_position(&self) -> Result<PhysicalPosition<i32>, NotSupportedError> {
        Err(NotSupportedError::new())
    }

    #[inline]
    pub fn set_outer_position(&self, _pos: Position) {
        // Not possible with wayland
    }

    pub fn inner_size(&self) -> PhysicalSize<u32> {
        let scale_factor = self.scale_factor as f64;
        let size = LogicalSize::<f64>::from(self.size);
        size.to_physical(scale_factor)
    }

    pub fn request_redraw(&self) {
        self.need_refresh = true;
        self.update.send(self);
    }

    #[inline]
    pub fn outer_size(&self) -> PhysicalSize<u32> {
        self.inner_size(); // fixme
    }

    #[inline]
    // NOTE: This will only resize the borders, the contents must be updated by the user
    pub fn set_inner_size(&self, size: Size) {
        let scale_factor = self.scale_factor() as f64;
        self.size = size.to_logical::<u32>(scale_factor).into();
    }

    #[inline]
    pub fn set_min_inner_size(&self, dimensions: Option<Size>) {
        let scale_factor = self.scale_factor() as f64;
        self.min_size = dimensions.map(|dim| dim.to_logical::<f64>(scale_factor).into()));
        self.update.send(self);
    }

    #[inline]
    pub fn set_max_inner_size(&self, dimensions: Option<Size>) {
        let scale_factor = self.scale_factor() as f64;
        self.max_size = dimensions.map(|dim| dim.to_logical::<f64>(scale_factor).into()));
        self.update.send(self);
    }

    #[inline]
    pub fn set_resizable(&self, resizable: bool) {
        self.resizable = resizable;
        self.update.send(self);
    }

    #[inline]
    pub fn scale_factor(&self) -> i32 {
        get_surface_scale_factor(&self.surface)
    }

    pub fn set_decorations(&self, decorate: bool) {
        *self.decorated= decorate;
        self.update.send(self);
    }

    pub fn set_minimized(&self, minimized: bool) {
        self.minimized = true;
        self.update.send(self);
    }

    pub fn set_maximized(&self, maximized: bool) {
        self.maximized = maxmized;
        self.update.send(self);
    }

    pub fn fullscreen(&self) -> Option<Fullscreen> {
        if self.fullscreen {
            Some(Fullscreen::Borderless(RootMonitorHandle {
                inner: PlatformMonitorHandle::Wayland(self.current_monitor()),
            }))
        } else {
            None
        }
    }

    pub fn set_fullscreen(&self, fullscreen: Option<Fullscreen>) {
        match fullscreen {
            Some(Fullscreen::Exclusive(_)) => {
                panic!("Wayland doesn't support exclusive fullscreen")
            }
            Some(Fullscreen::Borderless(RootMonitorHandle {
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

    #[inline]
    pub fn set_cursor_icon(&self, cursor: CursorIcon) {
        self.current_cursor = cursor
        self.update.send(self);
    }

    #[inline]
    pub fn set_cursor_visible(&self, visible: bool) {
        self.cursor_visible = visible
        self.update.send(self);
    }

    #[inline]
    pub fn set_cursor_grab(&self, grab: bool) -> Result<(), ExternalError> {
        self.cursor_grab = grab;
        self.update.send(self);
        Ok(())
    }

    #[inline]
    pub fn set_cursor_position(&self, _pos: Position) -> Result<(), ExternalError> {
        Err(ExternalError::NotSupported(NotSupportedError::new()))
    }

    pub fn display(&self) -> *Display {
        self.display
    }

    pub fn surface(&self) -> &wl_surface::WlSurface {
        &self.surface
    }

    pub fn current_monitor(&self) -> MonitorHandle {
        MonitorHandle(get_surface_outputs(&self.surface).last().unwrap().clone())
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        available_monitors(&self.env)
    }

    pub fn primary_monitor(&self) -> MonitorHandle {
        primary_monitor(&self.env)
    }

    pub fn raw_window_handle(&self) -> WaylandHandle {
        WaylandHandle {
            surface: self.surface().as_ref().c_ptr() as *mut _,
            display: self.display() as *mut _,
            ..WaylandHandle::empty()
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.size = (0, 0);
        self.update.send(self);
    }
}
