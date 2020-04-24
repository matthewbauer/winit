use smithay_client_toolkit::{
    reexports::{
        client::protocol::{wl_pointer::{ButtonState, Event, Axis}, wl_surface::WlSurface},
        protocols::unstable::relative_pointer::v1::client::{
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
        },
    },
    seat::pointer::ThemedPointer,
    window
};
type SCTKWindow = window::Window<window::ConceptFrame>;
use crate::{dpi::LogicalPosition, event::{ElementState, MouseButton, WindowEvent, TouchPhase, MouseScrollDelta}};
use super::{
    DeviceId,
    Sink,
    cursor::CursorManager
};

// Track focus and reconstruct scroll events
#[derive(Default)] pub struct Pointer {
    focus : Option<WlSurface>,
    axis_buffer: Option<(f32, f32)>,
    axis_discrete_buffer: Option<(i32, i32)>,
}

impl Pointer {
    fn handle(&mut self, event : Event, pointer: ThemedPointer, sink: &mut super::Sink, windows: &Windows, current_cursor: &'static str) {
        let Self{focus, axis_buffer, axis_discrete_buffer} = self;
        match event {
            Event::Enter { surface, surface_x:x,surface_y:y, .. } if let Some(wid) = store.find_wid(&surface) => {
                focus = Some(surface);

                // TODO: Reload cursor style only when we enter winit's surface. Calling
                // this function every time on `PtrEvent::Enter` could interfere with
                // SCTK CSD handling, since it changes cursor icons when you hover
                // cursor over the window borders.
                pointer.set_cursor(current_cursor, None).expect("Unknown cursor");

                sink.send_window_event(
                    WindowEvent::CursorEntered {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                    },
                    wid,
                );


                sink.send_window_event(
                    WindowEvent::CursorMoved {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        position: LogicalPosition::new(surface_x, surface_y).to_physical(get_surface_scale_factor(&surface) as f64),
                    },
                    wid,
                );
            }
            Event::Leave { surface, .. } => {
                mouse_focus = None;
                let wid = store.find_wid(&surface);
                if let Some(wid) = wid {
                    sink.send_window_event(
                        WindowEvent::CursorLeft {
                            device_id: crate::event::DeviceId(
                                crate::platform_impl::DeviceId::Wayland(DeviceId),
                            ),
                        },
                        wid,
                    );
                }
            }
            Event::Motion {
                surface_x,
                surface_y,
                ..
            } if let Some(surface) = mouse_focus => {
                let wid = make_wid(surface);

                let scale_factor = get_surface_scale_factor(&surface);
                let position =
                    LogicalPosition::new(surface_x, surface_y).to_physical(scale_factor as f64);

                sink.send_window_event(
                    WindowEvent::CursorMoved {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        position,
                        modifiers: modifiers_tracker.lock().unwrap().clone(),
                    },
                    wid,
                );
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
                sink.send_window_event(
                    WindowEvent::MouseInput {
                        device_id: crate::event::DeviceId(
                            crate::platform_impl::DeviceId::Wayland(DeviceId),
                        ),
                        state,
                        button,
                        modifiers: modifiers_tracker.lock().unwrap().clone(),
                    },
                    make_wid(surface),
                );
            }
            Event::Axis { axis, value, .. } if let Some(surface) = focus => {
                let wid = make_wid(surface);
                let (mut x, mut y) = axis_buffer.unwrap_or((0.0, 0.0));
                match axis {
                    // wayland vertical sign convention is the inverse of winit
                    Axis::VerticalScroll => y -= value as f32,
                    Axis::HorizontalScroll => x += value as f32,
                    _ => unreachable!(),
                }
                axis_buffer = Some((x, y));
                axis_state = match axis_state {
                    TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
                    _ => TouchPhase::Started,
                }
            }
            Event::Frame => {
                let axis_buffer = axis_buffer.take();
                let axis_discrete_buffer = axis_discrete_buffer.take();
                if let Some(surface) = mouse_focus {
                    let wid = make_wid(surface);
                    if let Some((x, y)) = axis_discrete_buffer {
                        sink.send_window_event(
                            WindowEvent::MouseWheel {
                                device_id: crate::event::DeviceId(
                                    crate::platform_impl::DeviceId::Wayland(DeviceId),
                                ),
                                delta: MouseScrollDelta::LineDelta(x as f32, y as f32),
                                phase: axis_state,
                                modifiers: modifiers_tracker.lock().unwrap().clone(),
                            },
                            wid,
                        );
                    } else if let Some((x, y)) = axis_buffer {
                        sink.send_window_event(
                            WindowEvent::MouseWheel {
                                device_id: crate::event::DeviceId(
                                    crate::platform_impl::DeviceId::Wayland(DeviceId),
                                ),
                                delta: MouseScrollDelta::PixelDelta((x as f64, y as f64).into()),
                                phase: axis_state,
                                modifiers: modifiers_tracker.lock().unwrap().clone(),
                            },
                            wid,
                        );
                    }
                }
            }
            Event::AxisSource { .. } => (),
            Event::AxisStop { .. } => {
                axis_state = TouchPhase::Ended;
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
                axis_state = match axis_state {
                    TouchPhase::Started | TouchPhase::Moved => TouchPhase::Moved,
                    _ => TouchPhase::Started,
                }
            }
            _ => unreachable!(),
        }
    }
}
