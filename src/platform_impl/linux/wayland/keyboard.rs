/*use std::sync::{Arc, Mutex};
use smithay_client_toolkit::{
    reexports::client::{
        protocol::{wl_keyboard, wl_seat},
        Attached,
    },
    seat::keyboard::{self, Event as KbEvent, /*RepeatEvent,*/ RepeatKind, RepeatSource},
};
use super::{event_loop::EventsSink, DeviceId};
use crate::event::{ElementState, KeyboardInput, ModifiersState, VirtualKeyCode, WindowEvent};*/

use std::{rc::Rc, cell::Cell, time::{Instant, Duration}};
//use futures::{future::FutureExt, stream::Stream};
pub use smithay_client_toolkit::seat::keyboard::{Event, KeyState};
use {super::Frame, crate::{event::{ElementState, ModifiersState, WindowEvent, KeyboardInput}}, super::conversion};

// Track modifiers and key repetition
#[derive(Default)] pub struct Keyboard {
    modifiers : ModifiersState,
    repeat : Option<Rc<Cell<Event<'static>>>>,
}

impl Keyboard {
    fn handle<T>(&mut self, Frame{sink, ..}: &mut Frame<T>, event: Event, is_synthetic: bool) {
        let Self{modifiers, repeat} = self;
        match event {
            Event::Enter { surface, .. } => {
                sink.send_window_event(WindowEvent::Focused(true), surface.id());
                /*if !modifiers.is_empty() ?*/ {
                    sink.send_window_event(WindowEvent::ModifiersChanged(modifiers), surface.id());
                }
            }
            Event::Leave { surface, .. } => {
                //*repeat = None, // will drop the timer on its next event (Weak::upgrade=None)
                /*if !modifiers.is_empty() {
                    sink.send_window_event(WindowEvent::ModifiersChanged(ModifiersState::empty()), wid);
                }*/
                sink.send_window_event(WindowEvent::Focused(false), surface.id());
            }
            key @ Event::Key{ surface, rawkey, state, utf8, .. } => {
                /*if state == KeyState::Pressed {
                    if let Some(repeat) = repeat { // Update existing repeat cell (also triggered by the actual repetition => noop)
                        repeat.set(event);
                        // Note: This keeps the same timer on key repeat change. No delay! Nice!
                    } else { // New repeat timer (registers in the reactor on first poll)
                        //assert!(!is_repeat);
                        let repeat = Rc::new(Cell::new(event));
                        use futures::stream;
                        streams.get_mut().push(
                            stream::unfold(Instant::now()+Duration::from_millis(300), {
                                let repeat = Rc::downgrade(&repeat);
                                |last| {
                                    let next = last+Duration::from_millis(100);
                                    smol::Timer::at(next).map(move |_| { repeat.upgrade().map(|x| x.clone().into_inner() ) }) // Option<Key> (None stops the stream, autodrops from streams)
                                }
                            })
                            .map(|(item, _t)| item)
                        );
                        repeat = Some(Cell::new(event));
                    }
                } else {
                    if repeat.filter(|r| r.get()==event).is_some() { repeat = None }
                }*/
                sink.send_window_event(
                    #[allow(deprecated)]
                    WindowEvent::KeyboardInput {
                        device_id: crate::platform_impl::DeviceId::Wayland(super::DeviceId),
                        input: KeyboardInput {
                            state,
                            scancode: rawkey,
                            virtual_keycode: conversion::key(key),
                            modifiers,
                        },
                        is_synthetic,
                    },
                    surface.id(),
                );
                if let Some(txt) = utf8 {
                    for char in txt.chars() {
                        sink.send_window_event(WindowEvent::ReceivedCharacter(char), surface.id());
                    }
                }
            }
            Event::Modifiers { surface, modifiers: new_modifiers, .. } => {
                *modifiers = conversion::modifiers(new_modifiers);
                sink.send_window_event(WindowEvent::ModifiersChanged(modifiers), surface.id());
            }
        }
    }
}

