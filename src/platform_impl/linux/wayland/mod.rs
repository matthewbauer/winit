#![cfg(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd",
           target_os = "netbsd", target_os = "openbsd"))]

pub use self::{
    event_loop::{EventLoop, EventLoopProxy, EventLoopWindowTarget, MonitorHandle, VideoMode},
    window::Window,
};

trait Sink<T> = FnMut(crate::event::Event<T>, &crate::event_loop::EventLoopWindowTarget<T>, &mut crate::event_loop::ControlFlow)+'static;

mod event_loop;
mod keyboard;
mod pointer;
mod touch;
mod window;
mod cursor;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId;

impl DeviceId {
    pub unsafe fn dummy() -> Self {
        DeviceId
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u32);

impl WindowId {
    pub unsafe fn dummy() -> Self {
        WindowId(0)
    }
}
