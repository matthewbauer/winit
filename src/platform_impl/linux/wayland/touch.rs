use std::sync::{Arc, Mutex};
use smithay_client_toolkit::{
    get_surface_scale_factor,
    reexports::client::protocol::{
        wl_seat,
        wl_surface::WlSurface,
        wl_touch::{self, WlTouch},
    },
};
use crate::{dpi::LogicalPosition, event::{Touch as Event, TouchPhase, WindowEvent}};
use super::Sink;

struct TouchPoint {
    surface: WlSurface,
    position: LogicalPosition<f64>,
    id: i32,
}
impl std::cmp::PartialEq {
    fn eq(&self, other: &Self) -> bool { self.id == other.id }
}

// Track touch points
pub type Touch = Vec<TouchPoint>;

impl Touch {
    fn handle(&mut self, sink: impl Sink, /*windows: &[super::window::WindowState],*/ event: Event) {
        let device_id = crate::event::DeviceId(super::super::DeviceId::Wayland(super::DeviceId));
        let sink = |phase,TouchPoint{surface, id, position}| {
            let e = Touch {device_id, phase, location: position.to_physical(get_surface_scale_factor(&surface) as f64), force: None/*TODO*/, id: id as u64};
            sink(event(Event::Touch(e), surface));
        };
        use wl_touch::Event::*;
        match event {
            Down {surface, id, x, y, ..} /*if windows.contains(&surface)*/ => {
                let point = TouchPoint{surface, position: LogicalPosition::new(x, y), id};
                sink(TouchPhase::Started, point);
                self.push(point);
            }
            Up { id, .. } => if let Some(point) = self.remove_item(id) /*=>*/ {
                sink(TouchPhase::Ended, point);
            }
            Motion { id, x, y, .. } => if let Some(point) = self.iter_mut().find(id) /*=>*/ {
                point.position = LogicalPosition::new(x, y);
                sink(TouchPhase::Moved, point);
            }
            Frame => (),
            Cancel => {
                for point in self.drain(..) {
                    sink(TouchPhase::Cancelled, point);
                }
            }
            _ => println!("Unexpected touch state"),
        }
    }
}
