use std::{rc::Rc, cell::Cell, time::{Instant, Duration}};
pub use smithay_client_toolkit::seat::keyboard::{Event, KeyState};
use {super::Update, crate::{event::{ElementState, ModifiersState, WindowEvent, KeyboardInput}}, super::conversion};

// Track modifiers and key repetition
#[derive(Default)] pub struct Keyboard {
    modifiers : ModifiersState,
    repeat : Option<Rc<Cell<Event<'static>>>>,
}

impl Keyboard {
    fn handle<T>(&mut self, Update{sink, ..}: &mut Update<T>, event: Event, is_synthetic: bool) {
        let Self{modifiers, repeat} = self;
        let event = |e,s| sink(event(e), s);
        match event {
            Event::Enter { surface, .. } => {
                event(Event::Focused(true), surface);
                /*if !modifiers.is_empty() ?*/ {
                    event(Event::ModifiersChanged(modifiers), surface);
                }
            }
            Event::Leave { surface, .. } => {
                //*repeat = None, // will drop the timer on its next event (Weak::upgrade=None)
                /*if !modifiers.is_empty() {
                    sink.send_window_event(WindowEvent::ModifiersChanged(ModifiersState::empty()), wid);
                }*/
                event(Event::Focused(false), surface);
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
                event(
                    #[allow(deprecated)]
                    Event::KeyboardInput {
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
                        event(Event::ReceivedCharacter(char), surface);
                    }
                }
            }
            Event::Modifiers { surface, modifiers: new_modifiers, .. } => {
                *modifiers = conversion::modifiers(new_modifiers);
                event(Event::ModifiersChanged(modifiers), surface);
            }
        }
    }
}
