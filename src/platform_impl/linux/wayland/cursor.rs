use smithay_client_toolkit::{
    environment::Environment,
    reexports::{
        client::{
            Attached,
            protocol::{
                wl_surface::WlSurface,
                wl_pointer::WlPointer,
            },
        },
        protocols::unstable::{
            pointer_constraints::v1::client::{
                zwp_locked_pointer_v1::ZwpLockedPointerV1, zwp_pointer_constraints_v1::Lifetime,
                zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            },
        },
    },
    seat::pointer::{ThemeManager, ThemeSpec::System, ThemedPointer},
};
use crate::window::CursorIcon;

pub struct CursorManager {
    constraints: Option<Attached<ZwpPointerConstraintsV1>>,
    theme_manager: Option<ThemeManager>,
    pointers: Vec<ThemedPointer>,
    locked_pointers: Vec<ZwpLockedPointerV1>,
    cursor_visible: bool,
    current_cursor: CursorIcon,
}

impl CursorManager {
    fn new<E>(env: Environment<E>, constraints: Option<Attached<ZwpPointerConstraintsV1>>) -> CursorManager {
        CursorManager {
            constraints,
            theme_manager: ThemeManager::init(System, env.require_global(), env.require_global()),
            pointers: Vec::new(),
            locked_pointers: Vec::new(),
            cursor_visible: true,
            current_cursor: CursorIcon::default(),
        }
    }

    fn register_pointer(&mut self, pointer: WlPointer) {
        let auto_themer = self
            .auto_themer
            .as_ref()
            .expect("AutoThemer not initialized. Server did not advertise shm or compositor?");
        self.pointers.push(auto_themer.theme_pointer(pointer));
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        if !visible {
            for pointer in self.pointers.iter() {
                (**pointer).set_cursor(0, None, 0, 0);
            }
        } else {
            self.set_cursor_icon_impl(self.current_cursor);
        }
        self.cursor_visible = visible;
    }

    /// A helper function to restore cursor styles on PtrEvent::Enter.
    pub fn reload_cursor_style(&mut self) {
        if !self.cursor_visible {
            self.set_cursor_visible(false);
        } else {
            self.set_cursor_icon_impl(self.current_cursor);
        }
    }

    pub fn set_cursor_icon(&mut self, cursor: CursorIcon) {
        if cursor != self.current_cursor {
            self.current_cursor = cursor;
            if self.cursor_visible {
                self.set_cursor_icon_impl(cursor);
            }
        }
    }

    pub fn update_scale_factor(&mut self) {
        self.reload_cursor_style();
    }

    fn set_cursor_icon_impl(&mut self, cursor: CursorIcon) {
        let cursor = match cursor {
            CursorIcon::Alias => "link",
            CursorIcon::Arrow => "arrow",
            CursorIcon::Cell => "plus",
            CursorIcon::Copy => "copy",
            CursorIcon::Crosshair => "crosshair",
            CursorIcon::Default => "left_ptr",
            CursorIcon::Hand => "hand",
            CursorIcon::Help => "question_arrow",
            CursorIcon::Move => "move",
            CursorIcon::Grab => "grab",
            CursorIcon::Grabbing => "grabbing",
            CursorIcon::Progress => "progress",
            CursorIcon::AllScroll => "all-scroll",
            CursorIcon::ContextMenu => "context-menu",

            CursorIcon::NoDrop => "no-drop",
            CursorIcon::NotAllowed => "crossed_circle",

            // Resize cursors
            CursorIcon::EResize => "right_side",
            CursorIcon::NResize => "top_side",
            CursorIcon::NeResize => "top_right_corner",
            CursorIcon::NwResize => "top_left_corner",
            CursorIcon::SResize => "bottom_side",
            CursorIcon::SeResize => "bottom_right_corner",
            CursorIcon::SwResize => "bottom_left_corner",
            CursorIcon::WResize => "left_side",
            CursorIcon::EwResize => "h_double_arrow",
            CursorIcon::NsResize => "v_double_arrow",
            CursorIcon::NwseResize => "bd_double_arrow",
            CursorIcon::NeswResize => "fd_double_arrow",
            CursorIcon::ColResize => "h_double_arrow",
            CursorIcon::RowResize => "v_double_arrow",

            CursorIcon::Text => "text",
            CursorIcon::VerticalText => "vertical-text",

            CursorIcon::Wait => "watch",

            CursorIcon::ZoomIn => "zoom-in",
            CursorIcon::ZoomOut => "zoom-out",
        };

        for pointer in self.pointers.iter() {
            // Ignore erros, since we don't want to fail hard in case we can't find a proper cursor
            // in a given theme.
            let _ = pointer.set_cursor/*_with_scale*/(cursor, /*self.scale_factor,*/ None);
        }
    }

    // This function can only be called from a thread on which `pointer_constraints_proxy` event
    // queue is located, so calling it directly from a Window doesn't work well, in case
    // you've sent your window to another thread, so we need to pass cursor grab updates to
    // the event loop and call this function from there.
    fn grab_pointer(&mut self, surface: Option<&WlSurface>) {
        for locked_pointer in self.locked_pointers.drain(..) {
            locked_pointer.destroy();
        }

        if let Some(surface) = surface {
            for pointer in self.pointers.iter() {
                let locked_pointer = self.constraints.as_ref().map(|constraints| {
                    constraints
                        .lock_pointer(surface, pointer, None, Lifetime::Persistent.to_raw())
                        .detach()
                });
                if let Some(locked_pointer) = locked_pointer {
                    self.locked_pointers.push(locked_pointer);
                }
            }
        }
    }
}
