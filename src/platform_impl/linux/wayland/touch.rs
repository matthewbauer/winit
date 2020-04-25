use std::sync::{Arc, Mutex};
use smithay_client_toolkit::{
    get_surface_scale_factor,
    reexports::client::protocol::{
        wl_seat,
        wl_surface::WlSurface,
        wl_touch::{Event, WlTouch},
    },
};
use crate::{dpi::LogicalPosition, event::{TouchPhase, WindowEvent}};
use super::{DeviceId, Frame};

struct TouchPoint {
    surface: WlSurface,
    position: LogicalPosition<f64>,
    id: i32,
}
impl std::cmp::PartialEq {
    fn eq(&self, other: &Self) -> bool { self.id == other.id }
}

type Touch = Vec<TouchPoint>;

impl Touch {
    fn handle(&mut self, event: Event, Frame{windows, ..}: &mut Frame) {
        match event {
            Event::Down {surface, id, x, y, ..} if let Some(wid) = store.find_wid(&surface) => {
                let position = LogicalPosition::new(x, y);
                sink.send_window_event(
                    WindowEvent::Touch(crate::event::Touch {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        phase: TouchPhase::Started,
                        location: position.to_physical(get_surface_scale_factor(&surface) as f64),
                        force: None, // TODO
                        id: id as u64,
                    }),
                    wid,
                );
                self.push(TouchPoint{
                    surface,
                    position,
                    id,
                });
            }
            Event::Up { id, .. } if let Some(point) = self.remove_item(|p| p.id == id) => {
                sink.send_window_event(
                    WindowEvent::Touch(crate::event::Touch {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        phase: TouchPhase::Ended,
                        location: point.position.to_physical(get_surface_scale_factor(&point.surface) as f64),
                        force: None, // TODO
                        id: id as u64,
                    }),
                    make_wid(&point.surface),
                );
            }
            Event::Motion { id, x, y, .. } if let Some(point) = self.iter_mut().find(id) => {
                point.position = LogicalPosition::new(x, y);
                sink.send_window_event(
                    WindowEvent::Touch(crate::event::Touch {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        phase: TouchPhase::Moved,
                        location: point.position.to_physical(get_surface_scale_factor(&point.surface) as f64),
                        force: None, // TODO
                        id: id as u64,
                    }),
                    make_wid(&point.surface),
                );
            }
            Event::Frame => (),
            Event::Cancel => {
                for point in self.drain(..) {
                    sink.send_window_event(
                        WindowEvent::Touch(crate::event::Touch {
                            device_id: crate::event::DeviceId(
                                crate::platform_impl::DeviceId::Wayland(DeviceId),
                            ),
                            phase: TouchPhase::Cancelled,
                            location: point.position.to_physical(get_surface_scale_factor(&point.surface) as f64),
                            force: None, // TODO
                            id: point.id as u64,
                        }),
                        make_wid(&point.surface),
                    );
                }
            }
            _ => println!("Unexpected touch state"),
        }
    }
}
