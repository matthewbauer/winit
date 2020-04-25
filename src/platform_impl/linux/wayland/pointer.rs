use smithay_client_toolkit::{
    reexports::{
        client::protocol::{wl_pointer::{ButtonState, Event, Axis}, wl_surface::WlSurface},
        protocols::unstable::relative_pointer::v1::client::{
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
        },
    },
    get_surface_scale_factor,
    seat::pointer::ThemedPointer,
    window
};
type SCTKWindow = window::Window<window::ConceptFrame>;
use crate::{dpi::LogicalPosition, event::{ElementState, MouseButton, WindowEvent, TouchPhase, MouseScrollDelta}};
use super::{Frame, DeviceId, Window};

// Track focus and reconstruct scroll events
#[derive(Default)] pub struct Pointer {
    focus : Option<WlSurface>,
    axis_buffer: Option<(f32, f32)>,
    axis_discrete_buffer: Option<(i32, i32)>,
}

impl Pointer {
    fn handle(&mut self, event : Event, pointer: ThemedPointer, Frame{sink}: &mut Frame, windows: &[Window], current_cursor: &'static str) {
        let Self{focus, axis_buffer, axis_discrete_buffer} = self;
        match event {
            Event::Enter { surface, surface_x:x,surface_y:y, .. } if let Some(window) = windows.find(&surface) => {
                focus = Some(surface);

                // Reload cursor style only when we enter winit's surface.
                // FIXME: Might interfere with CSD
                pointer.set_cursor(window.current_cursor, None).expect("Unknown cursor");

                sink(window_event(
                    WindowEvent::CursorEntered {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                    },
                    surface,
                ));

                sink(window_event(
                    WindowEvent::CursorMoved {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        position: LogicalPosition::new(x, y).to_physical(get_surface_scale_factor(&surface) as f64),
                    },
                    surface
                ));
            }
            Event::Leave { surface, .. } => {
                focus = None;
                if windows.contains(&surface) {
                    sink(window_event(
                        WindowEvent::CursorLeft {
                            device_id: crate::event::DeviceId(
                                crate::platform_impl::DeviceId::Wayland(DeviceId),
                            ),
                        },
                        surface
                    ));
                }
            }
            Event::Motion { surface_x:x, surface_y:y, .. } if let Some(surface) = focus => {
                sink(window_event(
                   WindowEvent::CursorMoved {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        position: LogicalPosition::new(surface_x, surface_y).to_physical(get_surface_scale_factor(&surface) as f64),
                        modifiers,
                    },
                   surface
                ));
            }
            Event::Button { button, state, .. } if let Some(surface) = mouse_focus.as_ref() => {
                state = if let ButtonState::Pressed = state
                    { ElementState::Pressed } else
                    { ElementState::Released};
                // input-event-codes
                let button = match button {
                    0x110 => MouseButton::Left,
                    0x111 => MouseButton::Right,
                    0x112 => MouseButton::Middle,
                    other => MouseButton::Other(other),
                };
                sink(window_event(
                    WindowEvent::MouseInput {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        state,
                        button,
                        modifiers: modifiers_tracker.lock().unwrap().clone(),
                    },
                    surface
                ));
            }
            Event::Axis { axis, value, .. } if let Some(surface) = focus => {
                let wid = make_wid(surface);
                let (mut x, mut y) = axis_buffer.unwrap_or((0.0, 0.0));
                match axis {
                    // wayland vertical sign convention is the inverse of winit
                    Axis::VerticalScroll => y -= value,
                    Axis::HorizontalScroll => x += value,
                    _ => unreachable!(),
                }
                axis_buffer = Some((x, y));
                phase = match phase {
                    TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
                    _ => TouchPhase::Started,
                }
            }
            Event::Frame => {
                let delta =
                    if Some(x,y) = axis_buffer.take() { MouseScrollDelta::PixelDelta(x as f64,y as f64) }
                    else if Some(x,y) = axis_discrete_buffer.take() { MouseScrollDelta::LineDelta(x as f32,y as f32) }
                    else { debug_assert!(false); MouseScrollDelta::PixelDelta(0,0) };
                if let Some(surface) = focus {
                    sink(window_event(
                        WindowEvent::MouseWheel {
                            device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId)),
                            delta, phase, modifiers,
                        },
                        surface
                    ));
                }
            }
            Event::AxisSource { .. } => (),
            Event::AxisStop { .. } => {
                phase = TouchPhase::Ended;
            }
            Event::AxisDiscrete { axis, discrete } => {
                let (mut x, mut y) = axis_discrete_buffer.unwrap_or((0, 0));
                match axis {
                    // wayland vertical sign convention is the inverse of winit
                    Axis::VerticalScroll => y -= discrete,
                    Axis::HorizontalScroll => x += discrete,
                    _ => unreachable!(),
                }
                axis_discrete_buffer = Some((x, y));
                phase = match phase {
                    TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
                    _ => TouchPhase::Started,
                }
            }
            _ => unreachable!(),
        }
    }
}
