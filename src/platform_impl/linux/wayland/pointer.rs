use smithay_client_toolkit::{
    reexports::{
        client::protocol::{wl_pointer::{self, ButtonState}, wl_surface::WlSurface},
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
use crate::{dpi::LogicalPosition, event::{ElementState, MouseButton, WindowEvent as Event, TouchPhase, MouseScrollDelta}};
use super::{Update, DeviceId, window::WindowState};

// Track focus and reconstruct scroll events
#[derive(Default)] pub struct Pointer {
    focus : Option<WlSurface>,
    axis_buffer: Option<(f32, f32)>,
    axis_discrete_buffer: Option<(i32, i32)>,
    phase: TouchPhase,
}

impl Pointer {
    fn handle(&mut self, event : Event, pointer: ThemedPointer, Update{sink}: &mut Update, windows: &[WindowState], current_cursor: &'static str) {
        let Self{focus, axis_buffer, axis_discrete_buffer, phase} = self;
        let event = |e,s| sink(event(e), s);
        let device_id = crate::event::DeviceId(super::super::DeviceId::Wayland(super::DeviceId));
        let position = |surface,x,y| LogicalPosition::new(x, y).to_physical(get_surface_scale_factor(&surface) as f64);
        use wl_pointer::Event::*;
        match event {
            Enter { surface, surface_x:x,surface_y:y, .. } => if let Some(window) = windows.iter().find(&surface) /*=>*/ {
                focus = Some(surface);

                // Reload cursor style only when we enter winit's surface.
                // FIXME: Might interfere with CSD
                pointer.set_cursor(window.current_cursor, None).expect("Unknown cursor");

                event(Event::CursorEntered {device_id}, surface);
                event(Event::CursorMoved {device_id, position: position(surface, x, y)}, surface);
            }
            Leave { surface, .. } => {
                focus = None;
                if windows.contains(&surface) {
                    event(Event::CursorLeft {device_id}, surface);
                }
            }
            Motion { surface_x:x, surface_y:y, .. } => if let Some(surface) = focus /*=>*/ {
                event(Event::CursorMoved {device_id, position: position(surface, x, y)}, surface);
            }
            Button { button, state, .. } => if let Some(surface) = focus /*=>*/ {
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
                event(Event::MouseInput {device_id, state, button}, surface);
            }
            Axis { axis, value, .. } => if let Some(surface) = focus /*=>*/ {
                let (mut x, mut y) = axis_buffer.unwrap_or((0.0, 0.0));
                use wl_pointer::Axis::*;
                match axis {
                    // wayland vertical sign convention is the inverse of winit
                    VerticalScroll => y -= value,
                    HorizontalScroll => x += value,
                    _ => unreachable!(),
                }
                axis_buffer = Some((x, y));
                phase = match phase {
                    TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
                    _ => TouchPhase::Started,
                }
            }
            Frame => {
                let delta =
                    if let Some((x,y)) = axis_buffer.take() { MouseScrollDelta::PixelDelta(x as f64,y as f64) }
                    else if let Some((x,y)) = axis_discrete_buffer.take() { MouseScrollDelta::LineDelta(x as f32,y as f32) }
                    else { debug_assert!(false); MouseScrollDelta::PixelDelta(0,0) };
                if let Some(surface) = focus {
                    event(Event::MouseWheel {device_id, delta, phase}, surface);
                }
            }
            AxisSource { .. } => (),
            AxisStop { .. } => phase = TouchPhase::Ended,
            AxisDiscrete { axis, discrete } => {
                let (mut x, mut y) = axis_discrete_buffer.unwrap_or((0, 0));
                use wl_pointer::Axis::*;
                match axis {
                    // wayland vertical sign convention is the inverse of winit
                    VerticalScroll => y -= discrete,
                    HorizontalScroll => x += discrete,
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
