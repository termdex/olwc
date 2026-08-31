// olshell - the unprivileged half of olwc. An ordinary wlr-layer-shell
// Wayland client: no compositor privileges, talks to olcore purely over
// standard and (eventually) custom Wayland protocol extensions. This
// skeleton binds the core globals and paints a single anchored panel
// surface, standing in for the eventual root menu / workspace strip.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_subcompositor,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    backend::ObjectId,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};

// Linux input event codes (linux/input-event-codes.h), as reported in
// wl_pointer button events. OPEN LOOK's MENU button is the right button;
// SELECT is the left one; ADJUST is the middle one.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

// openlook-workspaces isn't published anywhere -- it's olwc's own protocol,
// generated straight from the XML in the repo the same way wlr-protocols
// generates its own bindings (see wayland-scanner's docs). Cross-references
// to zwlr_foreign_toplevel_handle_v1 resolve via the wlr crate's interfaces;
// get_output_workspaces's wl_output argument needs wayland-client's own
// core interfaces in scope too, same reasoning as openlook_decoration below.
mod openlook_workspaces {
    pub mod v1 {
        pub mod client {
            use wayland_client;
            use wayland_client::protocol::*;
            use wayland_protocols_wlr::foreign_toplevel::v1::client::*;

            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                use wayland_protocols_wlr::foreign_toplevel::v1::client::__interfaces::*;
                wayland_scanner::generate_interfaces!(
                    "../protocol/openlook-workspaces-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!(
                "../protocol/openlook-workspaces-unstable-v1.xml"
            );
        }
    }
}

use openlook_workspaces::v1::client::{
    zopenlook_workspaces_manager_v1::{self, ZopenlookWorkspacesManagerV1},
    zopenlook_workspaces_output_v1::{self, ZopenlookWorkspacesOutputV1},
};

// Same approach as openlook_workspaces above, but this protocol's
// get_decoration request also takes a plain wl_surface argument, so its
// __interfaces module needs wayland-client's own core interfaces in scope
// too, not just the wlr crate's.
mod openlook_decoration {
    pub mod v1 {
        pub mod client {
            use wayland_client;
            use wayland_client::protocol::*;
            use wayland_protocols_wlr::foreign_toplevel::v1::client::*;

            pub mod __interfaces {
                use wayland_client::protocol::__interfaces::*;
                use wayland_protocols_wlr::foreign_toplevel::v1::client::__interfaces::*;
                wayland_scanner::generate_interfaces!(
                    "../protocol/openlook-decoration-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!(
                "../protocol/openlook-decoration-unstable-v1.xml"
            );
        }
    }
}

use openlook_decoration::v1::client::{
    zopenlook_decoration_manager_v1::{self, ZopenlookDecorationManagerV1},
    zopenlook_decoration_v1::{self, ZopenlookDecorationV1},
};

// openlook-session references no foreign objects at all (just a bare
// exit request), so unlike openlook_decoration/openlook_workspaces above
// its generated code needs nothing from any other protocol module.
mod openlook_session {
    pub mod v1 {
        pub mod client {
            use wayland_client;

            pub mod __interfaces {
                wayland_scanner::generate_interfaces!(
                    "../protocol/openlook-session-unstable-v1.xml"
                );
            }
            use self::__interfaces::*;

            wayland_scanner::generate_client_code!(
                "../protocol/openlook-session-unstable-v1.xml"
            );
        }
    }
}

use openlook_session::v1::client::zopenlook_session_manager_v1::{self, ZopenlookSessionManagerV1};

mod menu;
use menu::{Menu, MenuNode};

const PANEL_HEIGHT: u32 = 28;
const PANEL_FONT_SIZE: f32 = 20.0;
const PANEL_TEXT_COLOR: (u8, u8, u8) = (0x20, 0x20, 0x20);
const PANEL_BG_COLOR: (u8, u8, u8) = (0xBE, 0xBE, 0xBE);

const BACKGROUND_COLOR: (u8, u8, u8) = (0x5A, 0x76, 0x8C);

// Workspace switcher strip, drawn at the panel's left edge. No OPEN LOOK
// reference exists for this specifically -- docs/DESIGN.md's non-goals
// deliberately replace olvwm's pannable Virtual Desktop Manager with
// discrete, linear workspaces instead of reproducing it, so there's no
// authentic look to match here, just a plain numbered segmented strip.
// The active segment is filled with BACKGROUND_COLOR -- the same color as
// the desktop it represents, a deliberate visual tie-in rather than a new
// color pick.
const WORKSPACE_SEGMENT_WIDTH: i32 = 32;
const WORKSPACE_STRIP_MARGIN: i32 = 6;
const WORKSPACE_SEGMENT_GAP: i32 = 2;
const WORKSPACE_ACTIVE_TEXT_COLOR: (u8, u8, u8) = (0xF0, 0xF0, 0xF0);

// Icon tray: a minimized ("Close" in the window menu -- see
// WindowMenuAction::Minimize's doc comment) toplevel's icon, drawn
// directly into its own output's background rather than as a separate
// surface, the same way a real OPEN LOOK icon sits right on the desktop
// (root window) rather than in a dock or taskbar container. No reference
// exists for the icon's own look (screenshots/sunos551-ow1-scr-*.png show
// *that* icons sit there and can be app-specific/live-content, per
// docs/DESIGN.md's icon tray entry, but not at a resolution that shows
// real proportions or chrome) -- plain and functional like the workspace
// strip, not an attempt at OPEN LOOK's actual icon chrome.
const ICON_SIZE: i32 = 48;
const ICON_GAP: i32 = 14;
const ICON_MARGIN_BOTTOM: i32 = 14;
const ICON_LABEL_HEIGHT: i32 = 16;
const ICON_FONT_SIZE: f32 = 13.0;
const ICON_GLYPH_FONT_SIZE: f32 = 22.0;
const ICON_BG_COLOR: (u8, u8, u8) = DECORATION_BG_COLOR;
const ICON_BORDER_COLOR: (u8, u8, u8) = DECORATION_BORDER_COLOR;
const ICON_TEXT_COLOR: (u8, u8, u8) = WORKSPACE_ACTIVE_TEXT_COLOR;
// Distinct from ICON_BG_COLOR/MENU_HOVER_COLOR so selected, hovered, and
// plain icons are all visually distinguishable at once (e.g. hovering a
// different icon than the one currently selected).
const ICON_SELECTED_COLOR: (u8, u8, u8) = DECORATION_FOCUSED_BG_COLOR;
// SELECT-click on an icon only selects/highlights it, matching authentic
// OPEN LOOK -- restoring takes a second SELECT-click within this window,
// the conventional double-click gesture. Timestamps come from the Wayland
// pointer Press event (server clock, milliseconds), not wall-clock time.
const ICON_DOUBLE_CLICK_MS: u32 = 400;
// How far the pointer has to move (surface-local pixels) while SELECT is
// held on an icon before it counts as a drag rather than a click -- below
// this, a press-then-release is a click (select or double-click-restore);
// at or past it, the icon follows the pointer instead.
const ICON_DRAG_THRESHOLD: f64 = 4.0;

const MENU_FONT_SIZE: f32 = 18.0;
const MENU_ROW_HEIGHT: i32 = 26;
const MENU_H_PADDING: i32 = 12;
const MENU_BG_COLOR: (u8, u8, u8) = (0xD8, 0xD8, 0xD0);
const MENU_HOVER_COLOR: (u8, u8, u8) = (0x8A, 0x9E, 0xB0);
const MENU_TITLE_COLOR: (u8, u8, u8) = (0x40, 0x40, 0x38);
const MENU_TEXT_COLOR: (u8, u8, u8) = (0x18, 0x18, 0x18);

// Two separate box shapes rather than one square PUSHPIN_SIZE: the
// decoration's sticky indicator only ever shows the pinned state (a
// compact, roughly-square glyph -- see PUSHPIN_GLYPH_PINNED), but a popup
// toggles between that and the unpinned state, which is nearly 2:1 wide
// (PUSHPIN_GLYPH_UNPINNED) -- a single square box forced whichever glyph
// was further from square to be scaled down hard to fit, which is
// what made the unpinned glyph look compressed. Sized close to each
// glyph's own native pixel dimensions so draw_glyph_bitmap needs little
// or no shrinking for either.
const STICKY_PUSHPIN_SIZE: i32 = 15;
const POPUP_PUSHPIN_WIDTH: i32 = 26;
const POPUP_PUSHPIN_HEIGHT: i32 = 14;
const PUSHPIN_UNPINNED_COLOR: (u8, u8, u8) = MENU_TITLE_COLOR;
const PUSHPIN_PINNED_COLOR: (u8, u8, u8) = (0xA8, 0x30, 0x28);

// Rightward wedge marking a window-menu item that opens a submenu
// (currently just Move to Workspace) rather than acting immediately.
const SUBMENU_ARROW_SIZE: i32 = 8;

// Window decoration (header/title bar). See docs/OPENLOOK-REFERENCE.md's
// "Window menu" section -- this is the title bar these constants describe;
// the window-menu popup that its button is meant to open is follow-up work
// (see the button-click handler in PointerHandler::pointer_frame below).
const DECORATION_HEIGHT: u32 = 22;
// Deliberately a shade darker than the panel's (0xBE,0xBE,0xBE) -- same
// palette family, but visually distinct chrome, matching the reference's
// "clean thin border separating it from the content area" cue rather than
// blending into the desktop panel above it.
const DECORATION_BG_COLOR: (u8, u8, u8) = (0xA8, 0xA8, 0xA8);
// The focused header's fill and bevel direction, per the reference
// screenshots: the active window's title bar is a darkened, recessed
// rectangle where unfocused ones are the uniform light gray above --
// the same "inset = pressed" bevel language OPENLOOK-REFERENCE.md
// already calls out for buttons, applied here to focus instead.
const DECORATION_FOCUSED_BG_COLOR: (u8, u8, u8) = (0x80, 0x80, 0x80);
const DECORATION_BEVEL_LIGHT: (u8, u8, u8) = (0xE8, 0xE8, 0xE8);
const DECORATION_BEVEL_DARK: (u8, u8, u8) = (0x70, 0x70, 0x70);
const DECORATION_TEXT_COLOR: (u8, u8, u8) = (0x18, 0x18, 0x18);
const DECORATION_BUTTON_SIZE: i32 = 14;
const DECORATION_BUTTON_MARGIN: i32 = 4;
const DECORATION_BUTTON_HOVER_COLOR: (u8, u8, u8) = MENU_HOVER_COLOR;
const DECORATION_FONT_SIZE: f32 = 15.0;

// Resize-corner handles: right-angle brackets per
// docs/OPENLOOK-REFERENCE.md, confirmed against
// screenshots/sunos551-ow1-scr-01.png (see draw_corner_handle).
const CORNER_HANDLE_SIZE: i32 = 14;

// The plain black frame running around the rest of the window's straight
// edges -- confirmed against the same screenshot: 3px thick on all four
// sides, present everywhere except where a corner bracket sits instead.
const DECORATION_BORDER_COLOR: (u8, u8, u8) = (0, 0, 0);
const DECORATION_BORDER_WIDTH: i32 = 3;

// Window menu: the popup the decoration header's button opens. Reuses the
// root menu's palette (MENU_BG_COLOR etc.) -- both are menus and should
// look the same per OPEN LOOK's visual language -- plus one addition for
// the "Properties" item, which the reference screenshot shows grayed out.
const WINDOW_MENU_DISABLED_COLOR: (u8, u8, u8) = (0x90, 0x90, 0x88);

// Edge bitmask matching wlroots' WLR_EDGE_* / xdg_toplevel's resize_edge
// encoding, which openlook-decoration's resize request also uses (see its
// doc comment in the protocol XML). Only bottom|right is used for now --
// there's no resize-corner chrome yet to pick a different corner from.
#[allow(dead_code)]
const EDGE_TOP: u32 = 1;
const EDGE_BOTTOM: u32 = 2;
#[allow(dead_code)]
const EDGE_LEFT: u32 = 4;
const EDGE_RIGHT: u32 = 8;

enum WindowMenuAction {
    /// Labeled "Close" (see WINDOW_MENU_ITEMS) but iconifies rather than
    /// terminating -- authentic OPEN LOOK terminology, not a typo. See
    /// WINDOW_MENU_ITEMS' doc comment.
    Minimize,
    ToggleMaximize,
    Move,
    Resize,
    Lower,
    Quit,
    ToggleSticky,
    /// Opens the "Move to Workspace" submenu (WorkspaceSubmenu) instead
    /// of acting immediately -- see its handling in pointer_frame.
    MoveToWorkspace,
    /// Not wired up yet -- logs a placeholder, same as the root menu's
    /// non-interactive submenus.
    Unimplemented,
}

struct WindowMenuItem {
    label: &'static str,
    action: WindowMenuAction,
    disabled: bool,
    /// Keyboard-accelerator key drawn right-aligned on this row, preceded
    /// by a small diamond mark (see draw_window_menu and
    /// DIAMOND_MARK_GLYPH), or None for an item with no binding. The
    /// binding itself lives in olcore (`handle_keybinding`, `core/
    /// main.c`) -- olcore intercepts Super+<key> globally before routing
    /// input to the focused client, since a normal application window
    /// holds keyboard focus while in use and olshell alone has no way to
    /// intercept a global shortcut.
    ///
    /// The diamond is not decoration: real olwm accelerators used exactly
    /// one modifier, the physical "Meta" key Sun keyboards marked with a
    /// diamond glyph, confirmed from source -- `evbind.c`'s actual
    /// default bindings are plain `w+Meta`/`q+Meta`, and
    /// `menu.c`/`ol_button.c`'s `olgx_draw_diamond_mark` draws that
    /// diamond next to the accelerator letter whenever a binding includes
    /// Meta. What first looked like a screenshot showing `W` for Close
    /// but a Shift-arrow-prefixed `Q` for Quit was that same small
    /// diamond next to the Q, not a second modifier -- Super plays Meta's
    /// role here, uniformly, for every accelerator, matching that real
    /// convention rather than the mistaken shift-key reading. Close and
    /// Quit reuse the exact letters the reference screenshots show;
    /// Full Size/Back/Stick have no screenshot precedent, so are this
    /// project's own mnemonic picks.
    accel_key: Option<char>,
}

// Reference list from docs/OPENLOOK-REFERENCE.md's window menu section,
// minus Refresh: that item exists to force a repaint of a stale X11
// window, a class of bug Wayland's damage-tracking model makes
// structurally impossible, so there's nothing for it to do here -- dropped
// rather than kept as a dead placeholder. Close, Full Size, Move, Resize,
// Back, and Quit are wired to real actions; Move/Resize reuse the same
// interactive grabs the header drag gesture and (eventually) resize-corner
// handles trigger, Back reuses the same "lower to bottom" olcore exposes.
//
// Close and Quit are NOT the close-window/quit-application split most
// desktop environments now use -- that reading predated having an icon
// tray to make sense of the alternative (see docs/DESIGN.md's icon tray
// entry). Authentic OPEN LOOK terminology has Close *iconify* the window
// (the app keeps running, represented by an icon on its output's
// desktop -- see draw_background) and Quit is the only real terminate,
// sent to every toplevel sharing this one's client connection (see the
// decoration protocol's quit request) rather than just this window.
// Properties is disabled to match the reference screenshot (shown grayed
// out there too).
//
// Move to Workspace has no reference precedent -- see the workspace strip's
// ADJUST-click, which it complements now that submenus exist: this is the
// discoverable menu path for the same action, grouped next to Stick since
// both are about a window's relationship to workspaces.
const WINDOW_MENU_ITEMS: &[WindowMenuItem] = &[
    WindowMenuItem { label: "Close", action: WindowMenuAction::Minimize, disabled: false, accel_key: Some('W') },
    WindowMenuItem { label: "Full Size", action: WindowMenuAction::ToggleMaximize, disabled: false, accel_key: Some('F') },
    WindowMenuItem { label: "Move", action: WindowMenuAction::Move, disabled: false, accel_key: None },
    WindowMenuItem { label: "Resize", action: WindowMenuAction::Resize, disabled: false, accel_key: None },
    WindowMenuItem { label: "Properties", action: WindowMenuAction::Unimplemented, disabled: true, accel_key: None },
    WindowMenuItem { label: "Back", action: WindowMenuAction::Lower, disabled: false, accel_key: Some('B') },
    WindowMenuItem { label: "Stick", action: WindowMenuAction::ToggleSticky, disabled: false, accel_key: Some('S') },
    WindowMenuItem { label: "Move to Workspace", action: WindowMenuAction::MoveToWorkspace, disabled: false, accel_key: None },
    WindowMenuItem { label: "Quit", action: WindowMenuAction::Quit, disabled: false, accel_key: Some('Q') },
];

enum IconMenuAction {
    /// Restores the toplevel -- the same action a double-click on the
    /// icon triggers, offered here too since a real OPEN LOOK icon's own
    /// popup is the discoverable alternative to remembering the
    /// double-click (see the icon tray entry's own restore-gesture
    /// paragraph in docs/DESIGN.md).
    Open,
    /// Arms a click-to-follow move of the icon, ended by the next press
    /// anywhere -- see IconDrag's doc comment for why it's modeled as an
    /// IconDrag with `armed: true` rather than a new mechanism.
    Move,
    /// Not wired up yet -- logs a placeholder, same as the window menu's
    /// own Properties item.
    Unimplemented,
}

struct IconMenuItem {
    label: &'static str,
    action: IconMenuAction,
    disabled: bool,
}

// Deliberately just these three, matching docs/DESIGN.md's icon tray
// entry: a real OPEN LOOK icon's popup is Open/Move/Properties, plus
// Quit -- Quit is left out for now since nothing here has needed it yet
// (the icon tray's icons are always attached to a live, running
// application; wiring in a Quit path can wait until it does).
// Properties is disabled to match the window menu's own convention for
// the same not-yet-implemented item.
const ICON_MENU_ITEMS: &[IconMenuItem] = &[
    IconMenuItem { label: "Open", action: IconMenuAction::Open, disabled: false },
    IconMenuItem { label: "Move", action: IconMenuAction::Move, disabled: false },
    IconMenuItem { label: "Properties", action: IconMenuAction::Unimplemented, disabled: true },
];

// SIL Open Font License 1.1, see assets/fonts/OFL.txt.
static PANEL_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/VT323-Regular.ttf");

#[derive(Default)]
struct ToplevelInfo {
    title: String,
    app_id: String,
    states: Vec<u32>,
    // Kept so we can send it control requests later (close, maximize, ...)
    // and pass it to openlook-decoration's get_decoration. None only very
    // briefly, between insertion and the first event carrying it -- in
    // practice always Some by the time anything else reads this struct.
    handle: Option<ZwlrForeignToplevelHandleV1>,
    decoration: Option<Decoration>,
    // Kept in sync via the workspaces protocol's toplevel_workspace event.
    // Both None only until the first such event arrives. workspace_index
    // is used to gray out the window menu's own workspace in its Move to
    // Workspace submenu; output says which WorkspacePanel that submenu
    // should read/act through, now that workspaces are per-output.
    workspace_index: Option<u32>,
    output: Option<wl_output::WlOutput>,
    /// Top-left of this toplevel's icon box (background-surface-local, on
    /// whichever output it's currently minimized on) once the user has
    /// dragged it there -- None means "use the default packed left-to-
    /// right layout", the only kind olwc had before free positioning.
    /// Reset to None if the toplevel moves to a different output (see
    /// the ToplevelWorkspace handler below), since a stored position
    /// only makes sense relative to the output it was set on.
    icon_position: Option<(i32, i32)>,
}

/// The header (title bar) chrome olshell draws for one toplevel it doesn't
/// own, via openlook-decoration. Rendering only; the window-menu popup its
/// button is meant to open is follow-up work (see pointer_frame below).
struct Decoration {
    surface: wl_surface::WlSurface,
    object: ZopenlookDecorationV1,
    width: u32,
    height: u32,
    /// Integer buffer_scale for the header and (see draw_decoration) all
    /// seven of its child chrome subsurfaces -- footer/corners/borders
    /// deliberately don't track their own, since they're always positioned
    /// within this header's own bounds and so are on the same output/scale
    /// in every realistic case. Updated by scale_factor_changed.
    scale: i32,
    /// The decorated toplevel's own current content height -- not this
    /// header's, which is always `height`. Needed to position the bottom
    /// resize handles, which have to reach the toplevel's bottom edge;
    /// see the protocol's configure event doc comment for why olcore has
    /// to tell us this rather than us computing it some other way.
    toplevel_height: u32,
    button_hovered: bool,
    /// Mirrors olcore's state, kept in sync via the sticky_changed event --
    /// olshell never decides this itself, only asks to toggle it.
    sticky: bool,
    top_left: ResizeHandle,
    top_right: ResizeHandle,
    bottom_left: ResizeHandle,
    bottom_right: ResizeHandle,
    footer: ResizeHandle,
    left_border: BorderStrip,
    right_border: BorderStrip,
}

/// A resize handle: a small subsurface of the header (same trick as the
/// window menu -- both are olshell-owned surfaces, so no protocol
/// extension needed) positioned at one edge or corner of the toplevel.
/// Corners are drawn as right-angle brackets (draw_corner_handle); the
/// footer, a wide strip spanning the two bottom corners, is drawn as a
/// thin bar (draw_footer) -- see ResizeRegion for which is which and
/// what edges each one resizes from.
struct ResizeHandle {
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    hovered: bool,
}

/// A plain black border strip along the window's left or right edge --
/// see Decoration::border_side_rect(). Unlike ResizeHandle, this isn't
/// interactive (no hover, not in ResizeRegion), so it doesn't need
/// ResizeHandle's extra state: it's the same corner-to-corner black
/// frame confirmed in `screenshots/sunos551-ow1-scr-01.png`, just drawn
/// as its own subsurface since it spans both the header and the
/// toplevel's own content below it.
struct BorderStrip {
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
}

#[derive(Clone, Copy, PartialEq)]
enum ResizeRegion {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Footer,
}

impl ResizeRegion {
    const ALL: [ResizeRegion; 5] = [
        ResizeRegion::TopLeft,
        ResizeRegion::TopRight,
        ResizeRegion::BottomLeft,
        ResizeRegion::BottomRight,
        ResizeRegion::Footer,
    ];

    /// Edge bitmask to pass to the decoration protocol's resize request.
    fn edges(self) -> u32 {
        match self {
            ResizeRegion::TopLeft => EDGE_TOP | EDGE_LEFT,
            ResizeRegion::TopRight => EDGE_TOP | EDGE_RIGHT,
            ResizeRegion::BottomLeft => EDGE_BOTTOM | EDGE_LEFT,
            ResizeRegion::BottomRight => EDGE_BOTTOM | EDGE_RIGHT,
            ResizeRegion::Footer => EDGE_BOTTOM,
        }
    }

    /// Which edge of the handle's own square the bracket's elbow sits
    /// against -- (flip_x, flip_y) true means the elbow is on the
    /// left/top rather than the right/bottom. Only meaningful for the
    /// four corner variants; draw_corner_handle is never called for
    /// Footer.
    fn corner_flip(self) -> (bool, bool) {
        match self {
            ResizeRegion::TopLeft => (true, true),
            ResizeRegion::TopRight => (false, true),
            ResizeRegion::BottomLeft => (true, false),
            ResizeRegion::BottomRight => (false, false),
            ResizeRegion::Footer => unreachable!("footer has no corner glyph"),
        }
    }
}

impl Decoration {
    fn resize_handle(&self, region: ResizeRegion) -> &ResizeHandle {
        match region {
            ResizeRegion::TopLeft => &self.top_left,
            ResizeRegion::TopRight => &self.top_right,
            ResizeRegion::BottomLeft => &self.bottom_left,
            ResizeRegion::BottomRight => &self.bottom_right,
            ResizeRegion::Footer => &self.footer,
        }
    }

    fn resize_handle_mut(&mut self, region: ResizeRegion) -> &mut ResizeHandle {
        match region {
            ResizeRegion::TopLeft => &mut self.top_left,
            ResizeRegion::TopRight => &mut self.top_right,
            ResizeRegion::BottomLeft => &mut self.bottom_left,
            ResizeRegion::BottomRight => &mut self.bottom_right,
            ResizeRegion::Footer => &mut self.footer,
        }
    }

    // Top corner handles need room made for them at the header's far left
    // and right edges, so the button and sticky indicator both shift in
    // by CORNER_HANDLE_SIZE from where they'd otherwise sit.
    fn button_rect(&self) -> (i32, i32, i32, i32) {
        let x0 = CORNER_HANDLE_SIZE + DECORATION_BUTTON_MARGIN;
        let y0 = (self.height as i32 - DECORATION_BUTTON_SIZE) / 2;
        (x0, y0, x0 + DECORATION_BUTTON_SIZE, y0 + DECORATION_BUTTON_SIZE)
    }

    /// Sticky indicator, near the header's right edge -- purely a passive
    /// indicator (not clickable), reusing the same pushpin glyph the root
    /// menu's pin-to-persist gesture uses, since both mean "stays put".
    fn sticky_pushpin_rect(&self) -> (i32, i32, i32, i32) {
        let x1 = self.width as i32 - CORNER_HANDLE_SIZE - DECORATION_BUTTON_MARGIN;
        let x0 = x1 - STICKY_PUSHPIN_SIZE;
        let y0 = (self.height as i32 - STICKY_PUSHPIN_SIZE) / 2;
        (x0, y0, x1, y0 + STICKY_PUSHPIN_SIZE)
    }

    /// Header-local position for a corner handle -- header-local because
    /// all of these are subsurfaces of the header (see ResizeHandle's doc
    /// comment). Top corners sit at the header's own top edge, i.e. no Y
    /// offset; bottom corners are positioned far enough below the header
    /// (`self.height` + the toplevel's own content height) to reach the
    /// toplevel's actual bottom edge.
    fn corner_handle_position(&self, region: ResizeRegion) -> (i32, i32) {
        let right = matches!(region, ResizeRegion::TopRight | ResizeRegion::BottomRight);
        let bottom = matches!(region, ResizeRegion::BottomLeft | ResizeRegion::BottomRight);
        let x = if right { self.width as i32 - CORNER_HANDLE_SIZE } else { 0 };
        let y = if bottom { self.height as i32 + self.toplevel_height as i32 - CORNER_HANDLE_SIZE } else { 0 };
        (x, y)
    }

    /// Header-local (x, y, width) for the footer strip -- spans the full
    /// width minus the two bottom corner handles at its ends, at the same
    /// Y as they are. width can come out non-positive for a toplevel
    /// narrower than both corners combined; callers must check before
    /// drawing.
    fn footer_rect(&self) -> (i32, i32, i32) {
        let x0 = CORNER_HANDLE_SIZE;
        let y0 = self.height as i32 + self.toplevel_height as i32 - CORNER_HANDLE_SIZE;
        let width = self.width as i32 - 2 * CORNER_HANDLE_SIZE;
        (x0, y0, width)
    }

    /// Header-local (x, y0, y1) for the left (`right = false`) or right
    /// border strip -- runs the straight stretch of that side from just
    /// below the top corner handle to just above the bottom one, the
    /// same way footer_rect leaves room for the two bottom corners.
    /// Never drawn underneath a corner bracket's own transparent notch,
    /// matching the reference: the border simply stops where each
    /// bracket begins. y1 can come out non-positive before
    /// toplevel_height is known; callers must check before drawing.
    fn border_side_rect(&self, right: bool) -> (i32, i32, i32) {
        let x = if right { self.width as i32 - DECORATION_BORDER_WIDTH } else { 0 };
        let y0 = CORNER_HANDLE_SIZE;
        let y1 = self.height as i32 + self.toplevel_height as i32 - CORNER_HANDLE_SIZE;
        (x, y0, y1)
    }

    fn is_on_button(&self, x: f64, y: f64) -> bool {
        let (x0, y0, x1, y1) = self.button_rect();
        let (x, y) = (x as i32, y as i32);
        x >= x0 && x < x1 && y >= y0 && y < y1
    }
}

/// The window menu popup for one toplevel, opened by clicking its
/// decoration's button. A wl_subsurface of the decoration's own surface --
/// both are olshell-owned, so positioning it needs no protocol extension,
/// unlike the decoration itself -- positioned just below the header,
/// extending down over the window's content like the reference screenshots
/// show. No pushpin (the reference doesn't show one on window menus, only
/// on root menu-style pinnable menus).
///
/// Unlike a root-menu popup, a plain subsurface has no wlr-layer-shell
/// Exclusive-interactivity equivalent to ask for keyboard focus with, so
/// getting Escape-to-close working here (`open_window_menu`) did need a
/// protocol addition: openlook-decoration's grab_keyboard request. Click
/// elsewhere still closes it too, same as before.
struct WindowMenu {
    toplevel_id: ObjectId,
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    width: u32,
    height: u32,
    /// Integer buffer_scale, updated by scale_factor_changed -- also used
    /// to redraw the workspace_submenu below when it changes, which
    /// doesn't track its own (see BackgroundOutput/Decoration's own child-
    /// chrome fields for the same reasoning).
    scale: i32,
    hovered: Option<usize>,
    /// The "Move to Workspace" submenu, if currently open -- see
    /// WorkspaceSubmenu's doc comment.
    workspace_submenu: Option<WorkspaceSubmenu>,
}

impl WindowMenu {
    fn item_at(&self, y: f64) -> Option<usize> {
        let row = (y / MENU_ROW_HEIGHT as f64) as usize;
        (row < WINDOW_MENU_ITEMS.len()).then_some(row)
    }
}

/// A MENU-click (right-click) popup for one icon in the tray -- Open,
/// Move, Properties (see ICON_MENU_ITEMS). A subsurface of the icon's own
/// `BackgroundOutput` layer surface, positioned just below the icon
/// (`open_icon_menu`) -- the same "any of olshell's own surfaces can
/// parent a subsurface, no protocol extension needed" reasoning the
/// window menu already relies on for the decoration header, just with
/// the background as parent instead. Keyboard focus (openlook-
/// decoration's grab_keyboard) so Escape closes it, same as the window
/// menu and for the same reason -- a plain subsurface has no
/// wlr-layer-shell Exclusive-interactivity equivalent of its own.
struct IconMenu {
    /// The icon actually MENU-clicked to open this menu -- used to
    /// position it and to recognize a second click on the same icon's
    /// button as toggle-closed rather than reopen.
    toplevel_id: ObjectId,
    /// The icons Open/Move actually act on: just `toplevel_id` unless it
    /// was already part of a multi-selection
    /// (`BackgroundOutput::selected_icons`) when MENU-clicked, in which
    /// case the whole selection -- this is what makes ADJUST-click's
    /// selection mean something (see that field's doc comment).
    group: Vec<ObjectId>,
    /// Which BackgroundOutput this icon (and so this menu) belongs to --
    /// needed by the Move item to know which background's icon_position
    /// to update and which pointer events (keyed by background surface)
    /// apply.
    background_index: usize,
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    width: u32,
    height: u32,
    /// Integer buffer_scale, updated by scale_factor_changed.
    scale: i32,
    hovered: Option<usize>,
}

impl IconMenu {
    fn item_at(&self, y: f64) -> Option<usize> {
        let row = (y / MENU_ROW_HEIGHT as f64) as usize;
        (row < ICON_MENU_ITEMS.len()).then_some(row)
    }
}

/// One row of a WorkspaceSubmenu.
enum WorkspaceSubmenuRow {
    /// A non-interactive row naming the output the Workspace rows right
    /// below it belong to. Only present when there's more than one
    /// output to choose from at all -- a single-monitor setup keeps
    /// exactly the plain flat list this submenu always had, no headers.
    OutputHeader { name: String },
    /// A clickable "move to this workspace on this output" row.
    Workspace {
        output: wl_output::WlOutput,
        index: u32,
        /// Whether this is where the toplevel already is -- shown
        /// disabled, same convention as Properties, since moving a
        /// window to the workspace it's already on is a no-op.
        current: bool,
    },
}

/// The window menu's "Move to Workspace" submenu: lists every workspace on
/// every output directly rather than nesting further per output, so one
/// can still be picked in a single click -- see WorkspaceSubmenuRow for
/// how a multi-output list stays readable without a second level of
/// nesting. A subsurface of the window menu's own surface (itself a
/// subsurface of the header) -- Wayland subsurface trees nest arbitrarily
/// deep, and every surface in this chain is olshell's own, so this needed
/// no protocol extension either, same as the window menu itself.
/// Positioned to the window menu's right, top-aligned with the row that
/// opened it (see open_workspace_submenu). Opened and closed by clicking
/// that row, which toggles it exactly like the header button toggles the
/// window menu itself.
struct WorkspaceSubmenu {
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    width: u32,
    height: u32,
    hovered: Option<usize>,
    // Snapshotted at open time -- doesn't react to a workspace count or
    // output arrangement changing while open, a rare enough
    // reconfiguration that re-deriving row layout live isn't worth the
    // complexity; closing and reopening picks up the change like
    // everything else here already does.
    rows: Vec<WorkspaceSubmenuRow>,
}

impl WorkspaceSubmenu {
    /// Row index at surface-local `y`, whether or not that row is
    /// actually clickable (an OutputHeader, or the current-workspace
    /// Workspace row, isn't) -- callers already need to look the row up
    /// in `rows` to tell those apart, so this doesn't duplicate that.
    fn item_at(&self, y: f64) -> Option<usize> {
        let row = (y / MENU_ROW_HEIGHT as f64) as usize;
        (row < self.rows.len()).then_some(row)
    }

    /// Whether row `index` is a real, clickable target -- false for an
    /// OutputHeader (never actionable) and for the current-workspace row
    /// (a no-op, disabled the same way Properties is elsewhere).
    fn is_selectable(&self, index: usize) -> bool {
        matches!(self.rows.get(index), Some(WorkspaceSubmenuRow::Workspace { current: false, .. }))
    }
}

/// A root-menu popup: press MENU on a background to open one, drag to
/// highlight an item, release over it to run the item's command and
/// dismiss (release elsewhere just dismisses). Submenu entries are
/// rendered but not yet interactive -- opening a nested popup on hover is
/// follow-up work.
///
/// `Olshell::popups` holds any number of these at once, but
/// `open_menu` only ever lets one *unpinned* one exist -- opening a new
/// menu drops whatever unpinned one was already open, same as OPEN LOOK
/// only ever shows one transient menu at a time. A pinned one is exempt
/// from that: pinning (below) turns it into an independent persistent
/// palette that stays open regardless of what else olshell does,
/// including opening a menu on another output, so there can be one
/// pinned per output (or several on the same one) at once.
struct MenuPopup {
    layer: LayerSurface,
    /// Which output this popup was opened on -- needed by the Exit...
    /// item to know which output to center the confirmation Notice on
    /// (see open_notice), since a Notice has no click position of its own
    /// to anchor to the way this popup did.
    output: wl_output::WlOutput,
    items: Vec<MenuNode>,
    title: Option<String>,
    width: u32,
    height: u32,
    /// Integer buffer_scale, updated by scale_factor_changed.
    scale: i32,
    hovered: Option<usize>,
    // Pin-to-persist (OPEN LOOK's pushpin gesture): clicking the pushpin
    // in the header converts a transient popup into a persistent one that
    // survives a release and can be used repeatedly, until the pushpin is
    // clicked again or Escape is pressed. No drag-to-move yet -- it's not
    // truly a "floating palette" you can reposition, just one that stays
    // put and stays open.
    pinned: bool,
}

impl MenuPopup {
    /// The header row (pushpin + optional title text) is always present,
    /// regardless of whether this menu has a title -- the pushpin needs
    /// somewhere to live either way.
    fn header_rows(&self) -> i32 {
        1
    }

    /// Bounding box of the pushpin hit/paint target, within the header row.
    fn pushpin_rect(&self) -> (i32, i32, i32, i32) {
        // Top-left, per reference screenshots (screenshots/sunos551-ow1-scr-01.png
        // and -02.png): the pushpin sits before the title text, not after it.
        let x0 = MENU_H_PADDING;
        let x1 = x0 + POPUP_PUSHPIN_WIDTH;
        let y0 = (MENU_ROW_HEIGHT - POPUP_PUSHPIN_HEIGHT) / 2;
        (x0, y0, x1, y0 + POPUP_PUSHPIN_HEIGHT)
    }

    fn is_on_pushpin(&self, x: f64, y: f64) -> bool {
        let (x0, y0, x1, y1) = self.pushpin_rect();
        let (x, y) = (x as i32, y as i32);
        x >= x0 && x < x1 && y >= y0 && y < y1
    }

    /// Item index under `y` (surface-local), if any.
    fn item_at(&self, y: f64) -> Option<usize> {
        // Do the boundary check and division in f64 throughout -- mixing in
        // i32 here is a trap: integer division truncates toward zero, not
        // toward -inf, so a header-row y (which makes the numerator
        // negative) doesn't reliably come out negative after dividing.
        let header_h = (self.header_rows() * MENU_ROW_HEIGHT) as f64;
        if y < header_h {
            return None;
        }
        let row = ((y - header_h) / MENU_ROW_HEIGHT as f64) as usize;
        (row < self.items.len()).then_some(row)
    }
}

// Authentic OPEN LOOK, confirmed from source: real olwm's Exit... item
// (services.c's ExitFunc) shows exactly this message and these two
// buttons, Cancel as the safe default -- see Notice's own doc comment.
const NOTICE_MESSAGE: &str = "Please confirm exit from window system";
const NOTICE_BUTTONS: &[&str] = &["Exit", "Cancel"];
const NOTICE_DEFAULT_BUTTON: usize = 1;
/// Margin between the Notice's outer beveled frame and its content.
const NOTICE_PADDING: i32 = 16;
/// Thickness of the Notice's own outer bevel frame -- unlike the menu
/// popups (which have no frame at all), a Notice is meant to read as its
/// own distinct "boxed" element, matching real olwm's `olgx_draw_box`
/// frame (simplified here to one bevel layer rather than the original's
/// nested chiseled double-box).
const NOTICE_BORDER_WIDTH: i32 = 3;
const NOTICE_BUTTON_GAP: i32 = 16;
const NOTICE_BUTTON_VGAP: i32 = 20;
const NOTICE_BUTTON_HEIGHT: i32 = MENU_ROW_HEIGHT;
const NOTICE_BUTTON_H_PADDING: i32 = 16;

/// OPEN LOOK's "Notice" widget (see the widget vocabulary table: "used
/// specifically for short, must-acknowledge messages"), currently used
/// only for confirming the root menu's Exit... item -- see
/// NOTICE_MESSAGE's doc comment for the authentic source behind its
/// exact wording and buttons. A real wlr-layer-shell top-level surface
/// (`Layer::Overlay`), not a subsurface -- unlike the window/icon menus,
/// a Notice isn't tied to any particular decoration or icon to hang off
/// of. Deliberately given no anchor at all when created (see open_notice)
/// -- wlr-layer-shell centers an unanchored surface on its output for
/// free, which is exactly where a modal confirmation belongs, with none
/// of the pixel-precise "under the click" positioning math the root menu
/// itself needs.
///
/// Modal: pointer_frame swallows every pointer event that isn't on this
/// surface while one is open (see its own pre-check, mirroring the armed-
/// icon-drag swallow-check above it), and Escape/Return both dismiss it
/// without acting -- authentic, since Cancel (NOTICE_DEFAULT_BUTTON) is
/// the default button a real Notice's own Return-key handling
/// (`ACTION_EXEC_DEFAULT`) would trigger anyway.
struct Notice {
    layer: LayerSurface,
    width: u32,
    height: u32,
    scale: i32,
    /// SELECT is down on this button and the pointer is still over it --
    /// drawn in the recessed/invoked state until Release, matching
    /// olwm's own drawButton(OLGX_INVOKED) while a notice button is held.
    /// No separate hover-highlight state deliberately: real olwm's own
    /// noticeInterposer only ever changes a button's appearance on press/
    /// release (or keyboard focus, in Mouseless mode, which olshell
    /// doesn't implement), never on plain mouse-over.
    pressed: Option<usize>,
    /// (x0, y0, x1, y1) for each of NOTICE_BUTTONS, computed once in
    /// open_notice and shared by drawing and hit-testing (same pattern
    /// icon_layout/workspace_segment_x already use elsewhere) -- static
    /// once the Notice is sized, unlike an icon's position, so there's no
    /// need to recompute this on every draw or click the way those do.
    button_rects: Vec<(i32, i32, i32, i32)>,
}

impl Notice {
    fn button_at(&self, x: f64, y: f64) -> Option<usize> {
        let (x, y) = (x as i32, y as i32);
        self.button_rects.iter().position(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1)
    }
}

fn main() {
    env_logger::init();

    let conn = Connection::connect_to_env().expect("failed to connect to Wayland display");
    let (globals, mut event_queue) = registry_queue_init(&conn).expect("failed to init registry");
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr-layer-shell not available");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");
    // Used for the window menu popup: a subsurface of the decoration
    // header's surface, both olshell-owned, so no protocol extension
    // needed beyond this standard global.
    let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh)
        .expect("wl_subcompositor not available");

    // Panels and backgrounds are created per-output, one each, as outputs
    // are discovered via OutputHandler::new_output -- see WorkspacePanel's
    // and BackgroundOutput's doc comments. Nothing to do here at startup;
    // new_output fires for every output that already exists too, once the
    // event loop starts dispatching.

    let pool = SlotPool::new(4 * (PANEL_HEIGHT as usize) * 1920, &shm)
        .expect("failed to create shm pool");

    let menu = Menu::load_default();

    // Both optional: olshell should degrade gracefully against a compositor
    // that doesn't (yet) implement one or either of them.
    let foreign_toplevel_manager = globals
        .bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ())
        .ok();
    log::info!("wlr-foreign-toplevel-management: {}",
        if foreign_toplevel_manager.is_some() { "bound" } else { "not available" });
    let workspaces_manager = globals
        .bind::<ZopenlookWorkspacesManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    log::info!("openlook-workspaces: {}",
        if workspaces_manager.is_some() { "bound" } else { "not available" });
    let decoration_manager = globals
        .bind::<ZopenlookDecorationManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    log::info!("openlook-decoration: {}",
        if decoration_manager.is_some() { "bound" } else { "not available" });
    let session_manager = globals
        .bind::<ZopenlookSessionManagerV1, _, _>(&qh, 1..=1, ())
        .ok();
    log::info!("openlook-session: {}",
        if session_manager.is_some() { "bound" } else { "not available" });

    let font = fontdue::Font::from_bytes(PANEL_FONT_BYTES, fontdue::FontSettings::default())
        .expect("failed to parse embedded panel font");

    let mut state = Olshell {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        subcompositor,
        pool,
        panels: Vec::new(),
        backgrounds: Vec::new(),
        exit: false,
        foreign_toplevel_manager,
        workspaces_manager,
        decoration_manager,
        session_manager,
        toplevels: std::collections::HashMap::new(),
        font,
        menu,
        pointer: None,
        keyboard: None,
        keyboard_focus: None,
        popups: Vec::new(),
        window_menu: None,
        icon_menu: None,
        notice: None,
    };

    while !state.exit {
        event_queue.blocking_dispatch(&mut state).expect("event dispatch failed");
    }
}

/// One monitor's workspace switcher strip. Workspaces are per-output --
/// each monitor cycles its own independent sequence, following i3/Sway's
/// convention rather than one desktop-wide sequence, since it's the
/// closest architectural relative to olcore and avoids a secondary
/// reference monitor getting yanked to a different workspace just
/// because the primary switched. So each output olshell learns about
/// (via OutputHandler::new_output) gets its own panel, its own
/// zopenlook_workspaces_output_v1 object, and its own
/// workspace_count/active_workspace/hovered_workspace -- all the state
/// that used to be flat fields on Olshell itself back when there was
/// only ever one panel.
struct WorkspacePanel {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    workspaces: ZopenlookWorkspacesOutputV1,
    width: u32,
    height: u32,
    /// Integer buffer_scale, updated by scale_factor_changed.
    scale: i32,
    workspace_count: u32,
    active_workspace: u32,
    hovered_workspace: Option<u32>,
}

/// One monitor's desktop background, which doubles as the OPEN LOOK "root
/// window" -- MENU (right-button) clicks on it open the root menu. Per
/// output for the same reason WorkspacePanel is: a layer surface created
/// with `output: None` gets assigned to whichever one output the
/// compositor picks, so before this every monitor past the first got no
/// background (and so no right-click root menu) at all.
struct BackgroundOutput {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    width: u32,
    height: u32,
    /// Integer buffer_scale, updated by scale_factor_changed.
    scale: i32,
    /// Index (into the per-draw list draw_background/icon_at both
    /// recompute -- see minimized_toplevels_for_output) of the icon the
    /// pointer is currently over, if any.
    hovered_icon: Option<usize>,
    /// The current icon selection, keyed by the toplevel's ObjectId rather
    /// than tray position (the tray is sorted by title and re-sorts as
    /// windows are minimized/restored -- a position-based selection could
    /// silently end up highlighting a different icon after such a
    /// reshuffle). Authentic OPEN LOOK icons need a second click
    /// (double-click) to restore; a single SELECT-click just replaces this
    /// with a single-element selection (highlighting it), cleared entirely
    /// by clicking elsewhere on the background or by restoring an icon.
    /// ADJUST (middle-click) instead toggles one icon's membership in this
    /// set without disturbing the rest -- the icon-repositioning entry's
    /// deferred "what would ADJUST-click even do here" question in
    /// `docs/OPENLOOK-REFERENCE.md`, now with a real consumer: dragging, or
    /// the icon menu's Open/Move, act on the *whole* selection when the
    /// icon they're started from is a member of a multi-icon one (see
    /// IconDrag's doc comment), and on just that one icon otherwise.
    selected_icons: Vec<ObjectId>,
    /// (toplevel, event time) of the most recent SELECT-click on an icon,
    /// used to recognize the next click on the *same* icon within
    /// ICON_DOUBLE_CLICK_MS as a double-click rather than two single
    /// clicks.
    last_icon_click: Option<(ObjectId, u32)>,
    /// SELECT is down on an icon and we're still waiting to see whether
    /// this turns into a drag or a plain click -- see IconDrag's doc
    /// comment.
    drag: Option<IconDrag>,
    /// Whether a wl_surface.frame callback is currently outstanding for
    /// this background -- see request_background_redraw.
    frame_requested: bool,
    /// A redraw was asked for while frame_requested was already true, so
    /// the frame callback handler should draw once more before going
    /// idle instead of just clearing frame_requested.
    redraw_pending: bool,
}

/// Tracks a SELECT press on an icon from the moment it goes down until
/// it's either confirmed as a drag (moved past ICON_DRAG_THRESHOLD) or
/// released as a plain click. Needed because the same button-down starts
/// both gestures identically -- authentic OPEN LOOK icons are both
/// draggable to reposition and double-click-to-restore, and there's no
/// way to tell which one a press is starting until it either moves or
/// doesn't.
///
/// Also doubles as the icon menu's Move item's state (`armed: true`) --
/// that's a discrete click, not a press-and-hold, so it starts already
/// `dragging` (no threshold to cross) and finalizes on the *next* press
/// anywhere rather than a matching Release, the same click-to-arm/click-
/// to-drop pattern the window menu's own Move item uses (see
/// `zopenlook_decoration_v1::move`'s doc comment) -- see
/// `open_icon_menu`'s `Move` handler and the armed-move check at the top
/// of `pointer_frame`.
#[derive(Clone)]
struct IconDrag {
    // The icon actually pressed to start this drag (or, for an armed
    // menu-triggered move, the icon whose menu it was armed from) --
    // this is the one double-click detection and a plain (no-threshold-
    // crossed) Release's resulting single-icon selection always go by,
    // regardless of how many icons `icons` below is actually moving.
    primary: ObjectId,
    // The Press event's own time, carried through to Release so double-
    // click detection compares press-to-press intervals (matching
    // last_icon_click's own timestamps) rather than press-to-release.
    // Unused (0) for an armed move, which never goes through Release.
    press_time: u32,
    // Pointer position establishing the drag's reference point,
    // background-surface-local. Known immediately for a real press-and-
    // hold drag; None for an armed move until the first Motion arrives
    // after arming, so the icon doesn't jump to reflect pointer movement
    // that happened before tracking started (the pointer isn't
    // necessarily anywhere near the icon at the moment Move is clicked,
    // since the click happens on the icon menu's own surface).
    press_pos: Option<(f64, f64)>,
    // Every icon this drag actually moves, each paired with its own
    // on-screen top-left at the moment of press (or arming), before any
    // drag offset is applied. A single-element Vec (just `primary`) for
    // an ordinary drag or Move started from an icon that wasn't part of
    // a multi-selection; the whole selection (`BackgroundOutput::
    // selected_icons`) when it was -- ADJUST-click's one real consumer,
    // see that field's doc comment. Every entry moves by the same delta,
    // each clamped to the background's bounds independently, so a
    // group drag can visually spread apart if one member reaches an
    // edge before the others do.
    icons: Vec<(ObjectId, (i32, i32))>,
    // Set once the pointer has moved past ICON_DRAG_THRESHOLD (a real
    // drag) or immediately (an armed move, no threshold to cross) --
    // once true, Release ends a real drag rather than acting as a click;
    // an armed move ignores Release entirely and ends on the next Press
    // instead (see `armed` below).
    dragging: bool,
    // True for a Move-item-armed move, false for an ordinary press-and-
    // hold drag. Changes how the drag ends: an ordinary drag ends on
    // Release of the button that started it; an armed move has no
    // button held at all, so it ends on the next Press anywhere instead
    // (handled up front in `pointer_frame`, before normal click
    // handling for that press runs).
    armed: bool,
}

struct Olshell {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    subcompositor: SubcompositorState,
    pool: SlotPool,
    panels: Vec<WorkspacePanel>,
    backgrounds: Vec<BackgroundOutput>,
    exit: bool,
    // Kept only to hold the binding alive; no requests are sent on this
    // one, so nothing reads it beyond the Option check at startup.
    #[allow(dead_code)]
    foreign_toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,
    workspaces_manager: Option<ZopenlookWorkspacesManagerV1>,
    decoration_manager: Option<ZopenlookDecorationManagerV1>,
    session_manager: Option<ZopenlookSessionManagerV1>,
    toplevels: std::collections::HashMap<ObjectId, ToplevelInfo>,
    font: fontdue::Font,
    menu: Menu,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    // The surface a keyboard enter most recently reported, without a
    // matching leave yet -- three things ever request keyboard focus (a
    // root-menu popup's Exclusive layer surface, the window menu, and
    // the icon menu, the latter two via openlook-decoration's
    // grab_keyboard -- see each struct's own doc comment), so this is
    // how Escape tells which one, if any, to close.
    keyboard_focus: Option<wl_surface::WlSurface>,
    popups: Vec<MenuPopup>,
    window_menu: Option<WindowMenu>,
    icon_menu: Option<IconMenu>,
    notice: Option<Notice>,
}

impl Olshell {
    /// Draws one output's panel -- see WorkspacePanel's doc comment on why
    /// there's one of these per output rather than a single shared one.
    fn draw_panel(&mut self, panel_index: usize) {
        let Some(panel) = self.panels.get(panel_index) else {
            return;
        };
        // Nothing to paint into yet -- the compositor hasn't sent this
        // panel's first configure. A workspace_count/active_changed event
        // arriving before then would otherwise draw into a degenerate
        // 1px-wide buffer for no reason.
        if panel.width == 0 {
            return;
        }
        let width = panel.width as i32;
        let height = panel.height.max(1) as i32;
        let scale = panel.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = PANEL_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        let panel = &self.panels[panel_index];
        for i in 0..panel.workspace_count {
            let (x0, x1) = workspace_segment_x(i);
            if x0 >= width {
                break;
            }
            let active = i == panel.active_workspace;
            let hovered = panel.hovered_workspace == Some(i);
            let (fill, text_color) = if active {
                (BACKGROUND_COLOR, WORKSPACE_ACTIVE_TEXT_COLOR)
            } else if hovered {
                (MENU_HOVER_COLOR, PANEL_TEXT_COLOR)
            } else {
                ((0xAA, 0xAA, 0xA2), PANEL_TEXT_COLOR)
            };
            fill_rect(canvas, buf_width, buf_height, scale, x0, 2, x1.min(width), height - 2, fill);

            let label = (i + 1).to_string();
            let label_width: i32 =
                label.chars().map(|c| self.font.metrics(c, PANEL_FONT_SIZE).advance_width.round() as i32).sum();
            let label_x = x0 + ((x1 - x0) - label_width) / 2;
            draw_text_row_centered(canvas, buf_width, scale, 0, height, label_x, &label, &self.font, PANEL_FONT_SIZE, text_color);
        }

        let panel = &self.panels[panel_index];
        let wl_surface = panel.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        panel.layer.commit();
    }

    /// Draws one output's background, including its icon tray (see
    /// BackgroundOutput's doc comment on why there's one of these per
    /// output rather than a single shared one, and
    /// minimized_toplevels_for_output for what belongs in the tray).
    fn draw_background(&mut self, index: usize) {
        let Some(bg) = self.backgrounds.get(index) else {
            return;
        };
        if bg.width == 0 {
            return;
        }
        let width = bg.width as i32;
        let height = bg.height.max(1) as i32;
        let scale = bg.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        // No panel (openlook-workspaces unavailable) means no
        // active-workspace to gate the tray on -- degrade the same way
        // the panel itself does, rather than showing every minimized
        // window on every workspace at once.
        let active_workspace = self.panels.iter().find(|p| p.output == bg.output).map(|p| p.active_workspace);
        let icon_ids = active_workspace.map(|w| self.minimized_toplevels_for_output(&bg.output, w));
        let icon_rects = icon_ids.as_ref().map(|ids| self.icon_layout(ids, height));
        let hovered_icon = bg.hovered_icon;
        let selected_icons = bg.selected_icons.clone();

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = BACKGROUND_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        if let (Some(icon_ids), Some(rects)) = (&icon_ids, &icon_rects) {
            for (i, id) in icon_ids.iter().enumerate() {
                let (x0, y0, x1, y1) = rects[i];
                // Dragged icons can land anywhere on the background, so
                // (unlike the old pure packed layout) a too-far entry
                // doesn't mean every entry after it is too far too --
                // skip rather than stop.
                if x0 < 0 || y0 < 0 || x1 > width || y1 > height {
                    continue;
                }
                let fill = if hovered_icon == Some(i) {
                    MENU_HOVER_COLOR
                } else if selected_icons.contains(id) {
                    ICON_SELECTED_COLOR
                } else {
                    ICON_BG_COLOR
                };
                fill_rect(canvas, buf_width, buf_height, scale, x0, y0, x1, y1, fill);
                fill_rect(canvas, buf_width, buf_height, scale, x0, y0, x1, y0 + 1, ICON_BORDER_COLOR);
                fill_rect(canvas, buf_width, buf_height, scale, x0, y1 - 1, x1, y1, ICON_BORDER_COLOR);
                fill_rect(canvas, buf_width, buf_height, scale, x0, y0, x0 + 1, y1, ICON_BORDER_COLOR);
                fill_rect(canvas, buf_width, buf_height, scale, x1 - 1, y0, x1, y1, ICON_BORDER_COLOR);

                let info = &self.toplevels[id];
                let glyph = info
                    .app_id
                    .chars()
                    .next()
                    .or_else(|| info.title.chars().next())
                    .unwrap_or('?')
                    .to_uppercase()
                    .next()
                    .unwrap();
                let glyph_width = self.font.metrics(glyph, ICON_GLYPH_FONT_SIZE).advance_width.round() as i32;
                draw_text_row_centered(
                    canvas, buf_width, scale, y0, y1 - y0, x0 + (x1 - x0 - glyph_width) / 2,
                    &glyph.to_string(), &self.font, ICON_GLYPH_FONT_SIZE, ICON_TEXT_COLOR,
                );

                let label = if info.title.is_empty() { &info.app_id } else { &info.title };
                let label_width: i32 =
                    label.chars().map(|c| self.font.metrics(c, ICON_FONT_SIZE).advance_width.round() as i32).sum();
                let label_x = (x0 + x1) / 2 - label_width / 2;
                draw_text_row_centered(
                    canvas, buf_width, scale, y1, ICON_LABEL_HEIGHT, label_x.max(0),
                    label, &self.font, ICON_FONT_SIZE, ICON_TEXT_COLOR,
                );
            }
        }

        let bg = &self.backgrounds[index];
        let wl_surface = bg.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        bg.layer.commit();
    }

    /// Redraws background `index`, but never more than once per compositor
    /// frame -- every caller that used to call draw_background directly
    /// now goes through here instead. Icon dragging calls this on every
    /// single pointer Motion, which arrives far faster than the display
    /// can present frames; drawing synchronously on each one (the
    /// original approach) backed up the connection badly enough that
    /// olcore killed olshell as a misbehaving client mid-drag (confirmed
    /// live: "Data too big for buffer" followed by "error in client
    /// communication" in olcore's log). Standard Wayland throttling:
    /// request a wl_surface.frame callback alongside the draw, and defer
    /// any redraw that's asked for before that callback fires -- see
    /// CompositorHandler::frame below, which drains a deferred redraw
    /// when it arrives.
    fn request_background_redraw(&mut self, qh: &QueueHandle<Self>, index: usize) {
        if self.backgrounds[index].frame_requested {
            self.backgrounds[index].redraw_pending = true;
            return;
        }
        self.backgrounds[index].frame_requested = true;
        let surface = self.backgrounds[index].layer.wl_surface().clone();
        // Requested before draw_background's own commit below so it's
        // associated with the frame that commit produces, the ordering
        // wl_surface.frame's own documentation recommends.
        surface.frame(qh, surface.clone());
        self.draw_background(index);
    }

    /// Requests a header decoration for `toplevel_id` if openlook-decoration
    /// is available and it doesn't already have one. Actual drawing happens
    /// once the compositor's first configure event arrives (draw_decoration
    /// below), same as every other surface here.
    fn ensure_decoration(&mut self, qh: &QueueHandle<Self>, toplevel_id: &ObjectId) {
        let Some(manager) = self.decoration_manager.as_ref() else {
            return;
        };
        let Some(info) = self.toplevels.get(toplevel_id) else {
            return;
        };
        if info.decoration.is_some() {
            return;
        }
        let Some(handle) = info.handle.clone() else {
            return;
        };

        log::info!("decoration: requesting header for {toplevel_id:?}");
        let surface = self.compositor.create_surface(qh);
        let object =
            manager.get_decoration(&surface, &handle, DECORATION_HEIGHT, qh, toplevel_id.clone());

        // Resize handles (2 top corners, 2 bottom corners, footer): all
        // subsurfaces of the header, same trick as the window menu.
        // Positioned once real dimensions are known, in draw_decoration
        // (via the next configure), not here.
        let new_handle = || {
            let (subsurface, handle_surface) = self.subcompositor.create_subsurface(surface.clone(), qh);
            subsurface.set_desync();
            ResizeHandle { subsurface, surface: handle_surface, hovered: false }
        };
        let top_left = new_handle();
        let top_right = new_handle();
        let bottom_left = new_handle();
        let bottom_right = new_handle();
        let footer = new_handle();

        // The two side border strips: also subsurfaces of the header, not
        // resize handles (no hover, not in ResizeRegion) since they're
        // purely decorative -- see BorderStrip's doc comment.
        let new_border = || {
            let (subsurface, border_surface) = self.subcompositor.create_subsurface(surface.clone(), qh);
            subsurface.set_desync();
            BorderStrip { subsurface, surface: border_surface }
        };
        let left_border = new_border();
        let right_border = new_border();

        // Newly-created subsurfaces don't actually composite until the
        // parent commits at least once after the relationship is
        // established -- see open_window_menu's identical follow-up
        // commit for the live-tested reasoning.
        surface.commit();

        let info = self.toplevels.get_mut(toplevel_id).unwrap();
        info.decoration = Some(Decoration {
            surface,
            object,
            width: 0,
            height: DECORATION_HEIGHT,
            scale: 1,
            toplevel_height: 0,
            button_hovered: false,
            sticky: false,
            top_left,
            top_right,
            bottom_left,
            bottom_right,
            footer,
            left_border,
            right_border,
        });
    }

    /// Draws one toplevel's header: OPEN LOOK-style raised bevel, a
    /// window-menu button in the top-left corner, and the centered title.
    fn draw_decoration(&mut self, toplevel_id: &ObjectId) {
        let Some(info) = self.toplevels.get(toplevel_id) else {
            return;
        };
        let Some(dec) = info.decoration.as_ref() else {
            return;
        };
        if dec.width == 0 {
            return;
        }
        let width = dec.width as i32;
        let height = dec.height.max(1) as i32;
        let scale = dec.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        // state code 2 is "activated" -- see focused_toplevel_handle().
        let focused = info.states.contains(&2);
        let header_bg = if focused { DECORATION_FOCUSED_BG_COLOR } else { DECORATION_BG_COLOR };

        let (r, g, b) = header_bg;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }
        // 3D beveled shading: raised-unpressed (light top, dark bottom) when
        // unfocused, per docs/OPENLOOK-REFERENCE.md; flipped to inset when
        // focused, to match the recessed look of the active window's header
        // in the reference screenshots.
        let (bevel_top, bevel_bottom) =
            if focused { (DECORATION_BEVEL_DARK, DECORATION_BEVEL_LIGHT) } else { (DECORATION_BEVEL_LIGHT, DECORATION_BEVEL_DARK) };
        // Just inside the top border below -- the bevel row's the focus
        // indicator, the border's the window edge, and they're not the
        // same thing (docs/DESIGN.md has the reasoning for each).
        paint_row(canvas, buf_width, scale, DECORATION_BORDER_WIDTH, bevel_top);
        if height > 1 {
            paint_row(canvas, buf_width, scale, height - 1, bevel_bottom);
        }

        // The plain black frame's top stretch, skipping the two top
        // corners -- their own bracket subsurfaces sit right on top of
        // this and are partly transparent (the bracket's notch), so
        // anything drawn underneath them must stop exactly at their
        // edges, not bleed into the gap. See border_side_rect for the
        // matching left/right stretches.
        fill_rect(canvas, buf_width, buf_height, scale, CORNER_HANDLE_SIZE, 0, width - CORNER_HANDLE_SIZE, DECORATION_BORDER_WIDTH, DECORATION_BORDER_COLOR);

        let (bx0, by0, bx1, by1) = dec.button_rect();
        let button_color = if dec.button_hovered { DECORATION_BUTTON_HOVER_COLOR } else { header_bg };
        fill_rect(canvas, buf_width, buf_height, scale, bx0, by0, bx1, by1, button_color);
        draw_button_glyph(canvas, buf_width, buf_height, scale, bx0, by0, bx1, by1, dec.button_hovered, DECORATION_TEXT_COLOR);

        if !info.title.is_empty() {
            draw_text_row_centered(
                canvas, buf_width, scale, 0, height, bx1 + DECORATION_BUTTON_MARGIN,
                &info.title, &self.font, DECORATION_FONT_SIZE, DECORATION_TEXT_COLOR,
            );
        }

        if dec.sticky {
            let (px0, py0, px1, py1) = dec.sticky_pushpin_rect();
            draw_pushpin(canvas, buf_width, buf_height, scale, px0, py0, px1, py1, true, PUSHPIN_PINNED_COLOR);
        }

        let wl_surface = &dec.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        wl_surface.commit();
        log::info!("decoration: drew {toplevel_id:?} at {width}x{height}");

        // Top corners only need the header's own width, already known;
        // bottom corners and the footer also need toplevel_height, from
        // the toplevel's own first commit, which can lag behind the
        // header's.
        let (tl_x, tl_y) = dec.corner_handle_position(ResizeRegion::TopLeft);
        dec.top_left.subsurface.set_position(tl_x, tl_y);
        draw_corner_handle(&mut self.pool, &dec.top_left.surface, scale, dec.top_left.hovered, ResizeRegion::TopLeft, header_bg);

        let (tr_x, tr_y) = dec.corner_handle_position(ResizeRegion::TopRight);
        dec.top_right.subsurface.set_position(tr_x, tr_y);
        draw_corner_handle(&mut self.pool, &dec.top_right.surface, scale, dec.top_right.hovered, ResizeRegion::TopRight, header_bg);

        if dec.toplevel_height > 0 {
            let (bl_x, bl_y) = dec.corner_handle_position(ResizeRegion::BottomLeft);
            dec.bottom_left.subsurface.set_position(bl_x, bl_y);
            draw_corner_handle(&mut self.pool, &dec.bottom_left.surface, scale, dec.bottom_left.hovered, ResizeRegion::BottomLeft, header_bg);

            let (br_x, br_y) = dec.corner_handle_position(ResizeRegion::BottomRight);
            dec.bottom_right.subsurface.set_position(br_x, br_y);
            draw_corner_handle(&mut self.pool, &dec.bottom_right.surface, scale, dec.bottom_right.hovered, ResizeRegion::BottomRight, header_bg);

            let (f_x, f_y, f_width) = dec.footer_rect();
            if f_width > 0 {
                dec.footer.subsurface.set_position(f_x, f_y);
                draw_footer(&mut self.pool, &dec.footer.surface, scale, f_width as u32, dec.footer.hovered);
            }

            let (lb_x, lb_y0, lb_y1) = dec.border_side_rect(false);
            if lb_y1 > lb_y0 {
                dec.left_border.subsurface.set_position(lb_x, lb_y0);
                draw_border_strip(&mut self.pool, &dec.left_border.surface, scale, (lb_y1 - lb_y0) as u32);
            }

            let (rb_x, rb_y0, rb_y1) = dec.border_side_rect(true);
            if rb_y1 > rb_y0 {
                dec.right_border.subsurface.set_position(rb_x, rb_y0);
                draw_border_strip(&mut self.pool, &dec.right_border.surface, scale, (rb_y1 - rb_y0) as u32);
            }
        }

        // All the resize handles' very first content commit (above) needs
        // a fresh parent commit to actually become visible, same
        // subsurface gotcha as the window menu -- confirmed live: the
        // early "nudge" commit in ensure_decoration, sent before any of
        // them had any content yet, wasn't enough on its own. No new
        // header content, just the nudge.
        dec.surface.commit();
    }

    /// Opens the window menu for `toplevel_id`'s decoration, replacing any
    /// other one already open. A subsurface of the decoration's own
    /// surface, positioned just below the header.
    fn open_window_menu(&mut self, qh: &QueueHandle<Self>, toplevel_id: &ObjectId) {
        self.close_window_menu();

        let Some(dec_surface) = self
            .toplevels
            .get(toplevel_id)
            .and_then(|info| info.decoration.as_ref())
            .map(|dec| dec.surface.clone())
        else {
            return;
        };

        let label_width = |label: &str| -> i32 {
            label
                .chars()
                .map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32)
                .sum()
        };
        // "Unstick" (Stick's other label, see draw_window_menu) is wider
        // than "Stick" -- included here unconditionally so the popup is
        // wide enough for it regardless of current sticky state, since
        // that's decided once, here, before it's known.
        let max_width = WINDOW_MENU_ITEMS
            .iter()
            .map(|item| label_width(item.label))
            .chain(std::iter::once(label_width("Unstick")))
            .max()
            .unwrap_or(0);
        // Widest accelerator key (see WindowMenuItem::accel_key's doc
        // comment) plus its diamond mark, reserved on every row the same
        // way the submenu arrow below is, so the popup doesn't need a
        // different width depending on which item's accelerator happens
        // to be widest.
        let accel_width = DIAMOND_MARK_GLYPH.first().map_or(0, |row| row.len() as i32) + ACCEL_MARK_GAP * 2
            + WINDOW_MENU_ITEMS
                .iter()
                .filter_map(|item| item.accel_key)
                .map(|c| label_width(&c.to_string()))
                .max()
                .unwrap_or(0);
        // The extra MENU_H_PADDING + SUBMENU_ARROW_SIZE is Move to
        // Workspace's submenu-indicator arrow (see draw_window_menu) --
        // reserved on every row, not just that one, so the popup doesn't
        // need a different width depending on which item has it.
        let width = (max_width + MENU_H_PADDING * 3 + SUBMENU_ARROW_SIZE + accel_width).max(80) as u32;
        let height = (WINDOW_MENU_ITEMS.len() as i32 * MENU_ROW_HEIGHT) as u32;

        let (subsurface, surface) = self.subcompositor.create_subsurface(dec_surface.clone(), qh);
        subsurface.set_position(0, DECORATION_HEIGHT as i32);
        // Desync so the menu's own commits apply immediately rather than
        // waiting on the header's next commit -- every other surface here
        // behaves that way too, and there's no reason this one shouldn't.
        subsurface.set_desync();

        // A plain subsurface has no wlr-layer-shell Exclusive-interactivity
        // equivalent of its own, so getting keyboard focus (and thus
        // Escape-to-close, see WindowMenu's doc comment) needs asking
        // olcore explicitly. No matching release_keyboard call on close --
        // close_window_menu always destroys surface right after, which is
        // enough on its own (see grab_keyboard's doc comment).
        if let Some(manager) = self.decoration_manager.as_ref() {
            manager.grab_keyboard(&surface);
        }

        self.window_menu = Some(WindowMenu {
            toplevel_id: toplevel_id.clone(),
            subsurface,
            surface,
            width,
            height,
            scale: 1,
            hovered: None,
            workspace_submenu: None,
        });
        self.draw_window_menu();

        // A newly-created subsurface doesn't actually show up until its
        // parent commits at least once *after* the subsurface relationship
        // is established, even in desync mode -- confirmed live: the menu
        // was positioned and hit-testable immediately (a second button
        // click toggled it closed correctly) but stayed invisible until
        // something else, e.g. pointer motion over it, indirectly caused a
        // repaint. No new content, just the nudge.
        dec_surface.commit();
    }

    fn close_window_menu(&mut self) {
        if let Some(wm) = self.window_menu.take() {
            if let Some(sm) = wm.workspace_submenu {
                sm.subsurface.destroy();
                sm.surface.destroy();
            }
            // Don't wait for a leave event that destroying our own surface
            // may or may not still generate -- drop the stale reference now
            // so a later Escape doesn't look this surface up and find
            // nothing (same reasoning as close_menu's matching guard for a
            // root-menu popup). olcore's own destroy-listener counterpart
            // (keyboard_grab_surface_handle_destroy) is what actually hands
            // focus back to a toplevel; nothing to request on this end.
            if self.keyboard_focus.as_ref() == Some(&wm.surface) {
                self.keyboard_focus = None;
            }
            wm.subsurface.destroy();
            wm.surface.destroy();
        }
    }

    fn draw_window_menu(&mut self) {
        let Some(wm) = self.window_menu.as_ref() else {
            return;
        };
        let width = wm.width as i32;
        let height = wm.height as i32;
        let scale = wm.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = MENU_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        // Drives Stick's label ("Unstick" once toggled on) and Move to
        // Workspace's disabled state below -- olcore doesn't expose
        // per-item state generally, just this one via sticky on
        // Decoration (kept in sync by the sticky_changed event).
        let sticky = self
            .toplevels
            .get(&wm.toplevel_id)
            .and_then(|info| info.decoration.as_ref())
            .is_some_and(|dec| dec.sticky);

        for (i, item) in WINDOW_MENU_ITEMS.iter().enumerate() {
            let row_y0 = i as i32 * MENU_ROW_HEIGHT;
            // A sticky window is visible on every workspace regardless of
            // workspace_index, and un-sticking always commits to whatever
            // workspace is active *then* (see toggle_sticky's protocol
            // doc comment) rather than to whatever assign_toplevel might
            // have set it to -- so moving a sticky window's target
            // workspace can't actually do anything right now, and the
            // item is disabled while sticky rather than opening a
            // submenu with nothing meaningful in it.
            let disabled =
                item.disabled || (matches!(item.action, WindowMenuAction::MoveToWorkspace) && sticky);
            if !disabled && wm.hovered == Some(i) {
                // Equivalent to the manual per-pixel loop this replaced,
                // now via fill_rect so the scale multiplication happens in
                // one place rather than needing its own here too.
                draw_pill_highlight(canvas, buf_width, buf_height, scale, MENU_PILL_MARGIN, row_y0, width - MENU_PILL_MARGIN, row_y0 + MENU_ROW_HEIGHT);
            }
            let color = if disabled { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR };
            let label = if matches!(item.action, WindowMenuAction::ToggleSticky) && sticky {
                "Unstick"
            } else {
                item.label
            };
            draw_text_row_centered(
                canvas, buf_width, scale, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                label, &self.font, MENU_FONT_SIZE, color,
            );
            if matches!(item.action, WindowMenuAction::MoveToWorkspace) {
                let ax1 = width - MENU_H_PADDING;
                let ax0 = ax1 - SUBMENU_ARROW_SIZE;
                let ay0 = row_y0 + (MENU_ROW_HEIGHT - SUBMENU_ARROW_SIZE) / 2;
                let ay1 = ay0 + SUBMENU_ARROW_SIZE;
                draw_submenu_arrow(canvas, buf_width, buf_height, scale, ax0, ay0, ax1, ay1, color);
            }
            if let Some(accel_key) = item.accel_key {
                let key_str = accel_key.to_string();
                let key_width: i32 =
                    key_str.chars().map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32).sum();
                let diamond_w = DIAMOND_MARK_GLYPH.first().map_or(0, |row| row.len() as i32);
                let diamond_h = DIAMOND_MARK_GLYPH.len() as i32;
                let key_x = width - MENU_H_PADDING - key_width;
                let diamond_x = key_x - ACCEL_MARK_GAP - diamond_w;
                let diamond_y = row_y0 + (MENU_ROW_HEIGHT - diamond_h) / 2;
                blit_bitmap(canvas, buf_width, buf_height, scale, diamond_x, diamond_y, DIAMOND_MARK_GLYPH, color);
                draw_text_row_centered(
                    canvas, buf_width, scale, row_y0, MENU_ROW_HEIGHT, key_x,
                    &key_str, &self.font, MENU_FONT_SIZE, color,
                );
            }
        }

        let wl_surface = &wm.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        wl_surface.commit();
    }

    /// Restores (unset_minimized) and refocuses toplevel `id` -- shared by
    /// double-clicking its icon and the icon menu's Open item, the two
    /// ways to trigger the same action. unset_minimized alone would bring
    /// the window back but leave focus wherever it already was (possibly
    /// nowhere), so activate it too, the same pairing clicking a taskbar
    /// entry does elsewhere.
    fn restore_toplevel(&mut self, id: &ObjectId) {
        if let Some(handle) = self.toplevels.get(id).and_then(|info| info.handle.clone()) {
            handle.unset_minimized();
            if let Some(seat) = self.seat_state.seats().next() {
                handle.activate(&seat);
            }
        }
    }

    /// Opens toplevel_id's icon menu (Open/Move/Properties) positioned
    /// just below its icon on background_index's tray -- a no-op if the
    /// icon isn't actually in the tray right now (e.g. the click and the
    /// tray contents raced). Always closes whatever icon menu was already
    /// open first, same as the window menu's own open always does; the
    /// caller is responsible for the toggle-closed behavior when this
    /// exact icon's menu was the one already open (see pointer_frame's
    /// BTN_RIGHT handling), the same pattern open_window_menu's caller
    /// already uses for the header button.
    fn open_icon_menu(
        &mut self,
        qh: &QueueHandle<Self>,
        background_index: usize,
        toplevel_id: &ObjectId,
        group: Vec<ObjectId>,
    ) {
        self.close_icon_menu();

        let output = self.backgrounds[background_index].output.clone();
        let Some(active_workspace) =
            self.panels.iter().find(|p| p.output == output).map(|p| p.active_workspace)
        else {
            return;
        };
        let icon_ids = self.minimized_toplevels_for_output(&output, active_workspace);
        let Some(icon_index) = icon_ids.iter().position(|id| id == toplevel_id) else {
            return;
        };
        let bg_height = self.backgrounds[background_index].height as i32;
        let rects = self.icon_layout(&icon_ids, bg_height);
        let (ix0, _, _, iy1) = rects[icon_index];

        let label_width = |label: &str| -> i32 {
            label.chars().map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32).sum()
        };
        let max_width = ICON_MENU_ITEMS.iter().map(|item| label_width(item.label)).max().unwrap_or(0);
        let width = (max_width + MENU_H_PADDING * 2).max(80) as u32;
        let height = (ICON_MENU_ITEMS.len() as i32 * MENU_ROW_HEIGHT) as u32;

        let bg_surface = self.backgrounds[background_index].layer.wl_surface().clone();
        let (subsurface, surface) = self.subcompositor.create_subsurface(bg_surface.clone(), qh);
        // Just below the icon's label, left-aligned with the icon itself.
        subsurface.set_position(ix0, iy1 + ICON_LABEL_HEIGHT);
        subsurface.set_desync();

        // A plain subsurface has no wlr-layer-shell Exclusive-interactivity
        // equivalent of its own, so getting keyboard focus (and thus
        // Escape-to-close) needs asking olcore explicitly -- same
        // reasoning as the window menu's own grab_keyboard call.
        if let Some(manager) = self.decoration_manager.as_ref() {
            manager.grab_keyboard(&surface);
        }

        self.icon_menu = Some(IconMenu {
            toplevel_id: toplevel_id.clone(),
            group,
            background_index,
            subsurface,
            surface,
            width,
            height,
            scale: 1,
            hovered: None,
        });
        self.draw_icon_menu();

        // Same "parent needs a fresh commit after the subsurface
        // relationship is established, even in desync mode" nudge
        // open_window_menu's own doc comment explains.
        bg_surface.commit();
    }

    fn close_icon_menu(&mut self) {
        if let Some(im) = self.icon_menu.take() {
            if self.keyboard_focus.as_ref() == Some(&im.surface) {
                self.keyboard_focus = None;
            }
            im.subsurface.destroy();
            im.surface.destroy();
        }
    }

    fn draw_icon_menu(&mut self) {
        let Some(im) = self.icon_menu.as_ref() else {
            return;
        };
        let width = im.width as i32;
        let height = im.height as i32;
        let scale = im.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = MENU_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        for (i, item) in ICON_MENU_ITEMS.iter().enumerate() {
            let row_y0 = i as i32 * MENU_ROW_HEIGHT;
            if !item.disabled && im.hovered == Some(i) {
                draw_pill_highlight(canvas, buf_width, buf_height, scale, MENU_PILL_MARGIN, row_y0, width - MENU_PILL_MARGIN, row_y0 + MENU_ROW_HEIGHT);
            }
            let color = if item.disabled { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR };
            draw_text_row_centered(
                canvas, buf_width, scale, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                item.label, &self.font, MENU_FONT_SIZE, color,
            );
        }

        let wl_surface = &im.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        wl_surface.commit();
    }

    /// Opens the window menu's "Move to Workspace" submenu -- a no-op if
    /// it's already open (the MoveToWorkspace click handler calls
    /// close_workspace_submenu instead in that case, same toggle pattern
    /// the header button uses for the window menu itself) or if there's
    /// nowhere to move to at all (one workspace, one output).
    fn open_workspace_submenu(&mut self, qh: &QueueHandle<Self>, toplevel_id: &ObjectId) {
        let Some(current_output) = self.toplevels.get(toplevel_id).and_then(|info| info.output.clone()) else {
            return;
        };
        let Some(current_panel_index) = self.panels.iter().position(|p| p.output == current_output) else {
            return;
        };
        // Belt-and-suspenders: draw_window_menu already disables this
        // item's row while sticky (see its doc comment on why moving a
        // sticky window's target workspace can't do anything), so a
        // pointer press shouldn't be able to reach here in the first
        // place -- but this guard keeps that true regardless.
        let sticky = self
            .toplevels
            .get(toplevel_id)
            .and_then(|info| info.decoration.as_ref())
            .is_some_and(|dec| dec.sticky);
        if sticky {
            return;
        }
        let Some(wm) = self.window_menu.as_ref() else {
            return;
        };
        let wm_surface = wm.surface.clone();
        let wm_width = wm.width as i32;
        let row = WINDOW_MENU_ITEMS
            .iter()
            .position(|item| matches!(item.action, WindowMenuAction::MoveToWorkspace))
            .expect("Move to Workspace is always in WINDOW_MENU_ITEMS");
        let row_y = row as i32 * MENU_ROW_HEIGHT;

        let current_workspace = self.toplevels.get(toplevel_id).and_then(|info| info.workspace_index);

        // Current output's workspaces first (matching the plain flat list
        // this submenu always showed when there was only one output),
        // then every other output's in whatever order olshell discovered
        // them -- each one under its own non-interactive header row, but
        // only once there's more than one output to choose from at all,
        // so a single-monitor setup never grows headers it has no use
        // for.
        let multi_output = self.panels.len() > 1;
        let mut panel_order: Vec<usize> = (0..self.panels.len()).collect();
        panel_order.sort_by_key(|&i| if i == current_panel_index { 0 } else { 1 });

        let mut rows = Vec::new();
        for panel_index in panel_order {
            let panel = &self.panels[panel_index];
            if panel.workspace_count == 0 {
                continue;
            }
            if multi_output {
                let name = self
                    .output_state
                    .info(&panel.output)
                    .and_then(|info| info.name)
                    .unwrap_or_else(|| format!("Display {}", panel_index + 1));
                rows.push(WorkspaceSubmenuRow::OutputHeader { name });
            }
            let is_current_output = panel_index == current_panel_index;
            for index in 0..panel.workspace_count {
                let current = is_current_output && Some(index) == current_workspace;
                rows.push(WorkspaceSubmenuRow::Workspace { output: panel.output.clone(), index, current });
            }
        }
        // Nothing anywhere to actually move to (one output, one
        // workspace) -- same "nowhere else to move to" bail the old
        // single-output count<=1 check covered, generalized to also
        // cover "and there's no other output either".
        let any_target = rows.iter().any(|row| matches!(row, WorkspaceSubmenuRow::Workspace { current: false, .. }));
        if !any_target {
            return;
        }

        let label_width = |label: &str| -> i32 {
            label
                .chars()
                .map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32)
                .sum()
        };
        let max_width = rows
            .iter()
            .map(|row| match row {
                WorkspaceSubmenuRow::OutputHeader { name } => label_width(name),
                WorkspaceSubmenuRow::Workspace { index, .. } => label_width(&format!("Workspace {}", index + 1)),
            })
            .max()
            .unwrap_or(0);
        let width = (max_width + MENU_H_PADDING * 2).max(80) as u32;
        let height = (rows.len() as i32 * MENU_ROW_HEIGHT) as u32;

        let (subsurface, surface) = self.subcompositor.create_subsurface(wm_surface.clone(), qh);
        subsurface.set_position(wm_width, row_y);
        subsurface.set_desync();

        if let Some(wm) = self.window_menu.as_mut() {
            wm.workspace_submenu = Some(WorkspaceSubmenu { subsurface, surface, width, height, hovered: None, rows });
        }
        self.draw_workspace_submenu();

        // Same subsurface-visibility nudge every other handle in this
        // chain needs -- see open_window_menu's identical comment.
        wm_surface.commit();
    }

    fn close_workspace_submenu(&mut self) {
        if let Some(wm) = self.window_menu.as_mut() {
            if let Some(sm) = wm.workspace_submenu.take() {
                sm.subsurface.destroy();
                sm.surface.destroy();
            }
        }
    }

    fn draw_workspace_submenu(&mut self) {
        let Some(sm) = self.window_menu.as_ref().and_then(|wm| wm.workspace_submenu.as_ref()) else {
            return;
        };
        // Doesn't track its own scale -- always redrawn alongside its
        // parent window menu, whose scale it reads (see WindowMenu::
        // scale's doc comment).
        let scale = self.window_menu.as_ref().map_or(1, |wm| wm.scale);
        let width = sm.width as i32;
        let height = sm.height as i32;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = MENU_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        for (i, row) in sm.rows.iter().enumerate() {
            let row_y0 = i as i32 * MENU_ROW_HEIGHT;
            // Only a Workspace row that isn't the current one can be
            // hovered -- an OutputHeader is a plain separator, and the
            // current-workspace row is disabled, same as everywhere else
            // a disabled item never highlights.
            let hovered = matches!(row, WorkspaceSubmenuRow::Workspace { current: false, .. })
                && sm.hovered == Some(i);
            if hovered {
                draw_pill_highlight(canvas, buf_width, buf_height, scale, MENU_PILL_MARGIN, row_y0, width - MENU_PILL_MARGIN, row_y0 + MENU_ROW_HEIGHT);
            }
            let (label, color) = match row {
                WorkspaceSubmenuRow::OutputHeader { name } => (name.clone(), MENU_TITLE_COLOR),
                WorkspaceSubmenuRow::Workspace { index, current, .. } => (
                    format!("Workspace {}", index + 1),
                    if *current { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR },
                ),
            };
            draw_text_row_centered(
                canvas, buf_width, scale, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                &label, &self.font, MENU_FONT_SIZE, color,
            );
        }

        let wl_surface = &sm.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        wl_surface.commit();
    }

    /// Opens the root menu at `(x, y)` (surface-local coordinates on
    /// `output`'s background, which is fullscreen on that output so those
    /// are effectively screen coordinates local to it). Anchoring a layer
    /// surface to top-left with margins is the standard wlr-layer-shell
    /// trick for pixel-precise popup placement; binding the popup itself to
    /// `output` (the same one the triggering click landed on) keeps it on
    /// that placement rather than whichever output the compositor would
    /// otherwise pick arbitrarily.
    fn open_menu(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput, x: f64, y: f64) {
        // Drop any existing *unpinned* popup -- OPEN LOOK only ever shows
        // one transient menu at a time, regardless of which output it's
        // on. A pinned one is left alone: it's a persistent palette now,
        // independent of whatever else olshell does (see MenuPopup's doc
        // comment), so opening a fresh menu on another output shouldn't
        // undo a pin any more than it would close a real physical palette
        // sitting on another monitor.
        self.popups.retain(|p| p.pinned);

        let items = self.menu.items.clone();
        let title = self.menu.title.clone();

        let label_width = |label: &str| -> i32 {
            label
                .chars()
                .map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32)
                .sum()
        };
        // The header row needs to fit the title text *and* the pushpin
        // without them colliding, so it gets extra reserved width.
        let mut max_width = title.as_deref().map_or(0, label_width) + POPUP_PUSHPIN_WIDTH + MENU_H_PADDING;
        for item in &items {
            max_width = max_width.max(label_width(item.label()));
        }
        let width = (max_width + MENU_H_PADDING * 2).max(80) as u32;
        // Header row (pushpin, always present) + one row per item.
        let rows = items.len() as i32 + 1;
        let height = (rows * MENU_ROW_HEIGHT).max(MENU_ROW_HEIGHT) as u32;

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("olshell-menu"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_margin(y as i32, 0, 0, x as i32);
        layer.set_size(width, height);
        // Exclusive so olcore grants it keyboard focus while mapped (see
        // layer_surface_map() there) -- that's what lets Escape reach us.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.popups.push(MenuPopup {
            layer,
            output: output.clone(),
            items,
            title,
            width,
            height,
            scale: 1,
            hovered: None,
            pinned: false,
        });
    }

    /// Closes one popup by index. Just drops it -- sctk's LayerSurface::Drop
    /// already destroys the zwlr_layer_surface_v1 role object before the
    /// wl_surface, in the order the protocol requires. Destroying the
    /// wl_surface ourselves first (as this used to do) is a protocol
    /// violation: "surface was destroyed before its role object".
    fn close_menu(&mut self, index: usize) {
        let popup = self.popups.remove(index);
        // Don't wait for a leave event that destroying our own surface may
        // or may not still generate -- drop the stale reference now so a
        // later Escape doesn't look this surface up and find nothing.
        if self.keyboard_focus.as_ref() == Some(popup.layer.wl_surface()) {
            self.keyboard_focus = None;
        }
    }

    /// Opens the Exit... confirmation Notice, centered on `output` -- see
    /// Notice's own doc comment for why no anchor is set at all. A no-op
    /// if one's already open (there's only ever one thing to confirm
    /// right now, so this shouldn't be reachable, but replacing it out
    /// from under an in-progress confirmation would be a worse failure
    /// mode than just ignoring the second request).
    fn open_notice(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput) {
        if self.notice.is_some() {
            return;
        }

        let message_width: i32 =
            NOTICE_MESSAGE.chars().map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32).sum();
        let button_label_width = |label: &str| -> i32 {
            label.chars().map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32).sum()
        };
        let button_widths: Vec<i32> =
            NOTICE_BUTTONS.iter().map(|label| button_label_width(label) + NOTICE_BUTTON_H_PADDING * 2).collect();
        let total_button_width: i32 =
            button_widths.iter().sum::<i32>() + NOTICE_BUTTON_GAP * (NOTICE_BUTTONS.len() as i32 - 1);

        let content_width = message_width.max(total_button_width);
        let width = (content_width + (NOTICE_PADDING + NOTICE_BORDER_WIDTH) * 2).max(200) as u32;
        let message_row_height = MENU_FONT_SIZE.ceil() as i32;
        let height = (message_row_height + NOTICE_BUTTON_VGAP + NOTICE_BUTTON_HEIGHT
            + (NOTICE_PADDING + NOTICE_BORDER_WIDTH) * 2) as u32;

        // Buttons form one centered row below the message -- same
        // reasoning as drawNoticeBox's own buttonX/buttonY math.
        let button_row_y0 = height as i32 - NOTICE_BORDER_WIDTH - NOTICE_PADDING - NOTICE_BUTTON_HEIGHT;
        let mut button_x = (width as i32 - total_button_width) / 2;
        let button_rects: Vec<(i32, i32, i32, i32)> = button_widths
            .iter()
            .map(|&w| {
                let rect = (button_x, button_row_y0, button_x + w, button_row_y0 + NOTICE_BUTTON_HEIGHT);
                button_x += w + NOTICE_BUTTON_GAP;
                rect
            })
            .collect();

        let surface = self.compositor.create_surface(qh);
        let layer =
            self.layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some("olshell-notice"), Some(output));
        layer.set_size(width, height);
        // No anchor at all -- see Notice's own doc comment on why that's
        // what centers this on the output.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.notice = Some(Notice { layer, width, height, scale: 1, pressed: None, button_rects });
    }

    fn close_notice(&mut self) {
        if let Some(notice) = self.notice.take() {
            if self.keyboard_focus.as_ref() == Some(notice.layer.wl_surface()) {
                self.keyboard_focus = None;
            }
        }
    }

    fn draw_notice(&mut self) {
        let Some(notice) = self.notice.as_ref() else {
            return;
        };
        let width = notice.width as i32;
        let height = notice.height as i32;
        let scale = notice.scale;
        let buf_width = width * scale;
        let buf_height = height * scale;
        let stride = buf_width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = MENU_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        // Outer beveled frame -- see NOTICE_BORDER_WIDTH's doc comment on
        // why this is one bevel layer rather than real olwm's nested
        // chiseled double-box. Raised (light top/left, dark bottom/
        // right), same convention as everywhere else in olshell's chrome.
        let bw = NOTICE_BORDER_WIDTH;
        fill_rect(canvas, buf_width, buf_height, scale, 0, 0, width, bw, DECORATION_BEVEL_LIGHT);
        fill_rect(canvas, buf_width, buf_height, scale, 0, 0, bw, height, DECORATION_BEVEL_LIGHT);
        fill_rect(canvas, buf_width, buf_height, scale, 0, height - bw, width, height, DECORATION_BEVEL_DARK);
        fill_rect(canvas, buf_width, buf_height, scale, width - bw, 0, width, height, DECORATION_BEVEL_DARK);

        // Message, left-aligned near the top -- matches drawNoticeBox's
        // own "REMIND: all strings are along the left edge" layout.
        draw_text_row_centered(
            canvas, buf_width, scale, NOTICE_PADDING + NOTICE_BORDER_WIDTH, MENU_FONT_SIZE.ceil() as i32,
            NOTICE_PADDING + NOTICE_BORDER_WIDTH, NOTICE_MESSAGE, &self.font, MENU_FONT_SIZE, MENU_TEXT_COLOR,
        );

        for (i, &(x0, y0, x1, y1)) in notice.button_rects.iter().enumerate() {
            let pressed = notice.pressed == Some(i);
            draw_button(canvas, buf_width, buf_height, scale, x0, y0, x1, y1, NOTICE_BUTTONS[i], &self.font, pressed);
        }

        let wl_surface = notice.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, buf_width, buf_height);
        notice.layer.commit();
    }

    /// The toplevel whose decoration surface `surface` is, if any.
    fn decoration_toplevel_id(&self, surface: &wl_surface::WlSurface) -> Option<ObjectId> {
        self.toplevels.iter().find_map(|(id, info)| {
            info.decoration.as_ref().filter(|dec| dec.surface == *surface).map(|_| id.clone())
        })
    }

    /// (toplevel, is_right) for whichever bottom corner handle `surface`
    /// is, if any.
    /// (toplevel, which region) for whichever resize handle `surface` is,
    /// if any.
    fn resize_region_at(&self, surface: &wl_surface::WlSurface) -> Option<(ObjectId, ResizeRegion)> {
        self.toplevels.iter().find_map(|(id, info)| {
            let dec = info.decoration.as_ref()?;
            ResizeRegion::ALL
                .into_iter()
                .find(|&region| dec.resize_handle(region).surface == *surface)
                .map(|region| (id.clone(), region))
        })
    }

    /// Index into self.panels of whichever panel `surface` is, if any.
    fn panel_at(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.panels.iter().position(|p| p.layer.wl_surface() == surface)
    }

    /// Index into self.backgrounds of whichever output's background
    /// `surface` is, if any.
    fn background_at(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.backgrounds.iter().position(|b| b.layer.wl_surface() == surface)
    }

    /// Redraws whichever background shows `output`'s icon tray, if any --
    /// for keeping it in sync whenever something the tray's contents
    /// depend on changes (a toplevel's minimized state, title, or
    /// app_id; a toplevel disappearing; which workspace is active) but
    /// isn't already covered by some other redraw, the way hovering the
    /// tray itself is. Without this, a change only became visible once
    /// something else happened to redraw the background anyway (e.g. the
    /// pointer moving over it) -- confirmed live as an icon not
    /// appearing/disappearing until the pointer crossed the tray.
    fn redraw_background_for_output(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput) {
        if let Some(index) = self.backgrounds.iter().position(|b| &b.output == output) {
            self.request_background_redraw(qh, index);
        }
    }

    /// Index into self.popups of whichever root-menu popup `surface` is,
    /// if any.
    fn popup_at(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.popups.iter().position(|p| p.layer.wl_surface() == surface)
    }

    /// Minimized toplevels currently showing an icon on `output`'s
    /// background, in tray order (index 0 is icon_rect(0, ..)'s slot, and
    /// so on) -- draw_background and the background's own pointer
    /// handling both call this so they can never disagree about which
    /// icon is which. A toplevel qualifies the same way
    /// olcore's toplevel_is_visible does for the window itself, just
    /// inverted: minimized, and either sticky or on this output's active
    /// workspace -- both already tracked client-side (via
    /// wlr-foreign-toplevel-management's states and the workspaces
    /// protocol's toplevel_workspace event respectively), so this needs
    /// no protocol addition. Sorted by title so the tray doesn't reorder
    /// itself from one draw to the next merely because self.toplevels is
    /// a HashMap.
    fn minimized_toplevels_for_output(&self, output: &wl_output::WlOutput, active_workspace: u32) -> Vec<ObjectId> {
        let mut ids: Vec<ObjectId> = self
            .toplevels
            .iter()
            .filter(|(_, info)| {
                info.states.contains(&1)
                    && info.output.as_ref() == Some(output)
                    && (info.decoration.as_ref().is_some_and(|dec| dec.sticky)
                        || info.workspace_index == Some(active_workspace))
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort_by_key(|id| self.toplevels[id].title.clone());
        ids
    }

    /// The on-screen rect (icon box only -- the label sits below y1) for
    /// each id in `icon_ids`, same order, for a background of the given
    /// height. An id the user has dragged (ToplevelInfo::icon_position)
    /// renders there; every other id packs left-to-right along the
    /// bottom in its own sub-sequence, exactly as icon_rect always did,
    /// skipping over whichever indices are spoken for by dragged icons.
    /// Shared by drawing, hit-testing, and drag start so none of them can
    /// disagree about where an icon actually is.
    fn icon_layout(&self, icon_ids: &[ObjectId], bg_height: i32) -> Vec<(i32, i32, i32, i32)> {
        let mut default_index = 0;
        icon_ids
            .iter()
            .map(|id| match self.toplevels.get(id).and_then(|info| info.icon_position) {
                Some((x0, y0)) => (x0, y0, x0 + ICON_SIZE, y0 + ICON_SIZE),
                None => {
                    let rect = icon_rect(default_index, bg_height);
                    default_index += 1;
                    rect
                }
            })
            .collect()
    }

    /// Icon tray index at `(x, y)` (background-local) for the output
    /// `background_index` names, if any.
    fn icon_at(&self, background_index: usize, x: f64, y: f64) -> Option<usize> {
        let bg = &self.backgrounds[background_index];
        let panel = self.panels.iter().find(|p| p.output == bg.output)?;
        let icon_ids = self.minimized_toplevels_for_output(&bg.output, panel.active_workspace);
        let rects = self.icon_layout(&icon_ids, bg.height as i32);
        rects.iter().position(|&(x0, y0, x1, y1)| {
            x >= x0 as f64 && x < x1 as f64 && y >= y0 as f64 && y < y1 as f64
        })
    }

    /// Workspace index (0-based) the panel-local `x` falls on, if any,
    /// out of `workspace_count` total on that panel. Takes the count
    /// explicitly (rather than being a method reading it off self) since
    /// it's per-panel now, not a single global.
    fn workspace_at(x: f64, workspace_count: u32) -> Option<u32> {
        let x = x as i32;
        (0..workspace_count).find(|&i| {
            let (x0, x1) = workspace_segment_x(i);
            x >= x0 && x < x1
        })
    }

    /// The toplevel wlr-foreign-toplevel-management currently reports as
    /// activated, if any -- state code 2, per that protocol's state enum
    /// (0=maximized, 1=minimized, 2=activated, 3=fullscreen).
    fn focused_toplevel_handle(&self) -> Option<ZwlrForeignToplevelHandleV1> {
        self.toplevels.values().find(|info| info.states.contains(&2)).and_then(|info| info.handle.clone())
    }

    /// Runs a popup menu item's command via `sh -c`, detached. The spawned
    /// child is reaped on a background thread so it doesn't linger as a
    /// zombie for the rest of olshell's (long) lifetime.
    fn run_command(command: &str) {
        log::info!("root menu: running {command:?}");
        match std::process::Command::new("sh").arg("-c").arg(command).spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => log::warn!("root menu: failed to run {command:?}: {e}"),
        }
    }
}

/// Draws a resize-corner handle: a filled circle marker (reusing
/// draw_pushpin's shape) on an otherwise fully transparent buffer, so it
/// reads as a small floating marker over the toplevel's own corner rather
/// than a rectangle sitting on top of it.
///
/// The glyph itself is an L-shaped bracket -- a "framing square" -- rather
/// than the obround/pill placeholder used before: confirmed against
/// `screenshots/sunos551-ow1-scr-01.png`'s Text Editor window at all four
/// corners (cross-checked pixel-by-pixel, not just visually), which
/// consistently show a right-angle bracket hugging each corner rather than
/// a rounded shape. Each corner's bracket elbow sits at that corner, with
/// its two arms reaching along the two edges away from it; region picks
/// which corner via corner_flip(). The bevel direction is the same
/// absolute top-left-light/bottom-right-dark convention used everywhere
/// else in the header, not one that rotates with the bracket -- confirmed
/// against the same screenshot: e.g. the top-right corner's bracket still
/// has its top and left-facing surfaces lit and its right-facing surface
/// shadowed, same as bottom-right's.
fn draw_corner_handle(
    pool: &mut SlotPool,
    surface: &wl_surface::WlSurface,
    scale: i32,
    hovered: bool,
    region: ResizeRegion,
    fill_color: (u8, u8, u8),
) {
    // Every other quantity in this function derives from `size`, so
    // scaling it once here (rather than threading scale through fill_rect
    // calls, which this function doesn't make) scales the whole glyph,
    // bracket thickness included, proportionally.
    let size = CORNER_HANDLE_SIZE * scale;
    let stride = size * 4;
    let (buffer, canvas) =
        pool.create_buffer(size, size, stride, wl_shm::Format::Argb8888).expect("failed to create buffer");
    // Zero the whole pixel, not just alpha: SlotPool buffers are reused
    // memory, so leftover RGB from a previous draw would still be sitting
    // there. wl_shm Argb8888 buffers are expected premultiplied, and a
    // stale-RGB/zero-alpha pixel isn't validly premultiplied -- confirmed
    // live, it rendered as a faint ghost of whatever was drawn here before
    // instead of true transparency.
    canvas.fill(0);

    let (flip_x, flip_y) = region.corner_flip();
    let thickness = size * 3 / 7;
    let in_bracket = |x: i32, y: i32| -> bool {
        let ax = if flip_x { size - 1 - x } else { x };
        let ay = if flip_y { size - 1 - y } else { y };
        ax >= size - thickness || ay >= size - thickness
    };
    for y in 0..size {
        for x in 0..size {
            if !in_bracket(x, y) {
                continue;
            }
            let top_facing = y == 0 || !in_bracket(x, y - 1);
            let left_facing = x == 0 || !in_bracket(x - 1, y);
            let bottom_facing = y == size - 1 || !in_bracket(x, y + 1);
            let right_facing = x == size - 1 || !in_bracket(x + 1, y);
            let color = if hovered {
                DECORATION_BUTTON_HOVER_COLOR
            } else if top_facing || left_facing {
                DECORATION_BEVEL_LIGHT
            } else if bottom_facing || right_facing {
                DECORATION_BEVEL_DARK
            } else {
                fill_color
            };
            let (r, g, b) = color;
            let idx = ((y * size + x) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
    buffer.attach_to(surface).expect("failed to attach buffer");
    surface.set_buffer_scale(scale);
    surface.damage_buffer(0, 0, size, size);
    surface.commit();
}

/// Draws the footer strip: a thin horizontal bar, vertically centered in
/// an otherwise fully transparent buffer -- same reasoning as
/// draw_corner_handle (floats over the toplevel's own content, and needs
/// RGB zeroed along with alpha for the same premultiplication reason).
/// Unlike draw_corner_handle, this one calls fill_rect, so (following
/// fill_rect's own doc comment) `width`/`height` stay logical throughout
/// and only the separate buf_width/buf_height pair is scaled, to avoid
/// scaling the same coordinates twice over.
fn draw_footer(pool: &mut SlotPool, surface: &wl_surface::WlSurface, scale: i32, width: u32, hovered: bool) {
    let width = width as i32;
    let height = CORNER_HANDLE_SIZE;
    let buf_width = width * scale;
    let buf_height = height * scale;
    let stride = buf_width * 4;
    let (buffer, canvas) =
        pool.create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888).expect("failed to create buffer");
    canvas.fill(0);
    let color = if hovered { DECORATION_BUTTON_HOVER_COLOR } else { DECORATION_TEXT_COLOR };
    let bar_y0 = height / 2 - 1;
    let bar_y1 = height / 2 + 1;
    fill_rect(canvas, buf_width, buf_height, scale, 0, bar_y0, width, bar_y1, color);
    // The plain black frame's bottom stretch, at the toplevel's actual
    // bottom edge -- same border the left/right strips and the header's
    // own top stretch draw, completing the frame around all four sides.
    fill_rect(canvas, buf_width, buf_height, scale, 0, height - DECORATION_BORDER_WIDTH, width, height, DECORATION_BORDER_COLOR);
    buffer.attach_to(surface).expect("failed to attach buffer");
    surface.set_buffer_scale(scale);
    surface.damage_buffer(0, 0, buf_width, buf_height);
    surface.commit();
}

/// Draws a solid black border strip -- see Decoration::border_side_rect().
/// Unlike the corner handles and footer, every pixel here is opaque, so
/// there's no transparency/premultiplication gotcha to work around, and no
/// fill_rect call either (every pixel gets the same color) -- so, like
/// draw_corner_handle, its own width/height can just be scaled directly.
fn draw_border_strip(pool: &mut SlotPool, surface: &wl_surface::WlSurface, scale: i32, height: u32) {
    let width = DECORATION_BORDER_WIDTH * scale;
    let height = height as i32 * scale;
    let stride = width * 4;
    let (buffer, canvas) =
        pool.create_buffer(width, height, stride, wl_shm::Format::Argb8888).expect("failed to create buffer");
    let (r, g, b) = DECORATION_BORDER_COLOR;
    for pixel in canvas.chunks_exact_mut(4) {
        pixel[0] = b;
        pixel[1] = g;
        pixel[2] = r;
        pixel[3] = 0xFF;
    }
    buffer.attach_to(surface).expect("failed to attach buffer");
    surface.set_buffer_scale(scale);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
}

fn draw_popup(pool: &mut SlotPool, font: &fontdue::Font, popup: &MenuPopup) {
    let width = popup.width as i32;
    let height = popup.height as i32;
    let scale = popup.scale;
    let buf_width = width * scale;
    let buf_height = height * scale;
    let stride = buf_width * 4;

    let (buffer, canvas) = pool
        .create_buffer(buf_width, buf_height, stride, wl_shm::Format::Argb8888)
        .expect("failed to create buffer");

    let (r, g, b) = MENU_BG_COLOR;
    for pixel in canvas.chunks_exact_mut(4) {
        pixel[0] = b;
        pixel[1] = g;
        pixel[2] = r;
        pixel[3] = 0xFF;
    }

    // Header row: always present (the pushpin needs somewhere to live even
    // for a title-less menu), title text drawn only if there is one.
    let (px0, py0, px1, py1) = popup.pushpin_rect();
    if let Some(title) = &popup.title {
        // draw_text centers in the *whole* canvas height, not just this
        // row -- for a multi-row popup that puts the title text down in
        // the first item's row instead of its own, leaving row 0 blank
        // and garbling whatever's hovered in row 1. Row-centered instead.
        // Bold: XView's own menu widget renders a menu's title item in
        // its bold_font and everything else in the plain font
        // (lib/libxview/menu/omi.c: `if (im->title) font =
        // std_image->bold_font;`) -- a toolkit-level convention, not a
        // one-off screenshot artifact, so this is the one piece of text
        // in olshell that should be bold.
        draw_bold_text_row_centered(
            canvas, buf_width, scale, 0, MENU_ROW_HEIGHT, px1 + MENU_H_PADDING,
            title, font, MENU_FONT_SIZE, MENU_TITLE_COLOR,
        );
    }
    let pushpin_color = if popup.pinned { PUSHPIN_PINNED_COLOR } else { PUSHPIN_UNPINNED_COLOR };
    draw_pushpin(canvas, buf_width, buf_height, scale, px0, py0, px1, py1, popup.pinned, pushpin_color);
    let row = popup.header_rows();

    for (i, item) in popup.items.iter().enumerate() {
        let row_y0 = (row + i as i32) * MENU_ROW_HEIGHT;
        if popup.hovered == Some(i) {
            draw_pill_highlight(canvas, buf_width, buf_height, scale, MENU_PILL_MARGIN, row_y0, width - MENU_PILL_MARGIN, row_y0 + MENU_ROW_HEIGHT);
        }
        draw_text_row_centered(
            canvas, buf_width, scale, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
            item.label(), font, MENU_FONT_SIZE, MENU_TEXT_COLOR,
        );
    }

    let wl_surface = popup.layer.wl_surface();
    buffer.attach_to(wl_surface).expect("failed to attach buffer");
    wl_surface.set_buffer_scale(scale);
    wl_surface.damage_buffer(0, 0, buf_width, buf_height);
    popup.layer.commit();
}

/// Draws `text` with its baseline at `baseline_y`, rather than centered in
/// the whole canvas -- e.g. for centering within a single menu row.
// scale multiplies row_y0/row_height/start_x/size here, once, before
// handing off to draw_text_at -- which stays scale-unaware, working
// purely in whatever pixel space it's given (see fill_rect's doc comment
// for the general split this follows).
#[allow(clippy::too_many_arguments)]
fn draw_text_row_centered(
    canvas: &mut [u8],
    canvas_width: i32,
    scale: i32,
    row_y0: i32,
    row_height: i32,
    start_x: i32,
    text: &str,
    font: &fontdue::Font,
    size: f32,
    color: (u8, u8, u8),
) -> i32 {
    let row_y0 = row_y0 * scale;
    let row_height = row_height * scale;
    let start_x = start_x * scale;
    let size = size * scale as f32;
    let baseline_y = row_y0 + row_height / 2 + (size as i32) / 3;
    draw_text_at(canvas, canvas_width, row_y0 + row_height, start_x, baseline_y, text, font, size, color)
}

/// Faux-bold variant of draw_text_row_centered: draws the text twice, the
/// second copy shifted 1px right, thickening strokes via double alpha-
/// blending. The bundled font (VT323) has no real bold weight to switch
/// to -- it's a deliberately single-weight retro terminal typeface -- so
/// this is the practical way to get XView's actual bold-title convention
/// (see the caller) without bundling a second, stylistically mismatched
/// font family just for one line of text.
#[allow(clippy::too_many_arguments)]
fn draw_bold_text_row_centered(
    canvas: &mut [u8],
    canvas_width: i32,
    scale: i32,
    row_y0: i32,
    row_height: i32,
    start_x: i32,
    text: &str,
    font: &fontdue::Font,
    size: f32,
    color: (u8, u8, u8),
) -> i32 {
    draw_text_row_centered(canvas, canvas_width, scale, row_y0, row_height, start_x, text, font, size, color);
    // The +1 here is logical, not physical -- draw_text_row_centered
    // multiplies it by scale along with start_x itself, so the faux-bold
    // offset stays a proportional 1 logical pixel (i.e. `scale` physical
    // ones) at any scale, not a hairline-thin single physical pixel once
    // scale > 1.
    draw_text_row_centered(canvas, canvas_width, scale, row_y0, row_height, start_x + 1, text, font, size, color)
}

// The window-menu button and pushpin glyphs below are traced pixel-for-
// pixel from Sun's own OLGlyph bitmap font (olgl14.bdf), not approximated.
// olwm/olvwm and the XView/OLIT toolkits drew this chrome by rendering
// characters from that font via libolgx (olgx_draw_abbrev_button,
// olgx_draw_pushpin) rather than by drawing bitmaps directly; the font
// itself is preserved, still under Sun's original 1989 "permission to use,
// copy, modify, and distribute... for any purpose and without fee" notice,
// in the historical XView/olwm source trees at github.com/MagnetarRocket/
// xview-openlook and github.com/ggodd/xview-64bit
// (xview-base/fonts/bdf/misc/olgl14.bdf) -- see docs/OPENLOOK-REFERENCE.md.
// Each string below is one bitmap row, '#' meaning "on"; draw_glyph_bitmap
// nearest-neighbor-scales whatever box the caller asks for, so the source
// bitmap's own resolution doesn't have to match olshell's chosen sizes.

/// Window-menu button glyph (OLGlyph encoding 22, `OLG_ABBREV_MENU_BUTTON`):
/// a rounded-square housing around a downward-pointing chevron.
const BUTTON_GLYPH_NORMAL: &[&str] = &[
    ".###############..",
    "#...............#.",
    "#...............##",
    "#...............##",
    "#...#########...##",
    "#...#.......#...##",
    "#....#.....#....##",
    "#....#.....#....##",
    "#.....#...#.....##",
    "#.....#...#.....##",
    "#......#.#......##",
    "#......#.#......##",
    "#.......#.......##",
    "#...............##",
    "#...............##",
    ".#################",
    "..###############.",
];

/// Window-menu button glyph, invoked/pressed state (encoding 23,
/// `OLG_ABBREV_MENU_BUTTON_INVERTED`). olshell has no separate button-press
/// state today (only hover, which already gets its own fill-color change --
/// see button_rect's caller), so this stands in for hover instead of going
/// untouched.
const BUTTON_GLYPH_PRESSED: &[&str] = &[
    ".###############..",
    "#...............#.",
    "#.#############.##",
    "#.#############.##",
    "#.##.........##.##",
    "#.##.#######.##.##",
    "#.###.#####.###.##",
    "#.###.#####.###.##",
    "#.####.###.####.##",
    "#.####.###.####.##",
    "#.#####.#.#####.##",
    "#.#####.#.#####.##",
    "#.######.######.##",
    "#.#############.##",
    "#...............##",
    ".#################",
    "..###############.",
];

// olgx_draw_pushpin actually has two distinct designs for these states, not
// one: encodings 100-105 are three-layer bevel composites (highlight/fill/
// shadow in three different colors) meant only for 3D rendering, and don't
// flatten cleanly to a single color -- tried first, and confirmed live to
// render as a compressed-looking blob, since a naive union of three bevel
// outlines is thicker and blockier than any one of them alone. olgx has a
// second, purpose-built flat single-color version of each for its 2D
// rendering path (`pupinout`/`pupinin` below) -- these are the ones that
// actually belong here, the same way the button glyph already uses its own
// flat variant (OLG_ABBREV_MENU_BUTTON) rather than the 3D bevel layers
// abbrev_button uses in 3D mode.

/// Pushpin, unpinned ("pushpin out") state -- olgx's flat single-color
/// glyph (encoding 19, `pupinout`): the pin lying on its side, head and
/// shaft outlined.
const PUSHPIN_GLYPH_UNPINNED: &[&str] = &[
    "...............###...........",
    "...............#..#.......##.",
    "...............#..#......#..#",
    "...............#..########..#",
    "...............#..#......#..#",
    "...............#..#......#..#",
    ".....###########..#......#..#",
    "......##########..#......#..#",
    "...............#..#......#..#",
    "...............#..########..#",
    ".##............#..###########",
    "#..#...........####......####",
    "#..#...........####.......##.",
    ".##............###...........",
];

/// Pushpin, pinned ("pushpin in") state -- olgx's flat single-color glyph
/// (encoding 20, `pupinin`): the pin pushed straight into the board, seen
/// at an angle, outlined.
const PUSHPIN_GLYPH_PINNED: &[&str] = &[
    "........###....",
    ".....###...##..",
    "...###.......#.",
    "..#..#.......#.",
    ".#..#.........#",
    ".#..#.........#",
    "#...#.........#",
    "#...##.......##",
    "#....#.......#.",
    "##...###...###.",
    ".#....#######..",
    ".##.....#####..",
    ".####....###...",
    "###########....",
    "##.............",
];

/// Submenu ("pullright") indicator -- olgx's "menu mark" glyph
/// (encodings 48/49/50, `HorizMeMa-UL`/`-LR`/`fill`), oriented
/// horizontally for a pullright item (`OLGX_HORIZ_MENU_MARK`; a vertical
/// orientation, encodings 45-47, marks the window-menu button itself --
/// see `olgx_draw_abbrev_button`'s 3D path). XView's own 2D rendering
/// combines all three layers in one color (`olgx_draw_menu_mark`'s
/// `!info->three_d` branch draws the UL and LR outline layers together,
/// then optionally the fill layer on top), which is what this traces --
/// a solid filled triangle, not just its outline.
const SUBMENU_ARROW_GLYPH: &[&str] = &[
    "##.........",
    "####.......",
    "######.....",
    "########...",
    "##########.",
    "###########",
    "##########.",
    "########...",
    "######.....",
    "####.......",
    "##.........",
];

/// The window-menu accelerator "Meta" mark -- see WindowMenuItem::
/// accel_key's doc comment for why a diamond, and why it isn't decorative.
/// Unlike the glyphs above, this isn't traced from OLGlyph: real olwm/
/// libolgx drew it procedurally (`olgx_draw_diamond_mark`, a six-point
/// outline, not a bitmap font character), so this is a plain hand-drawn
/// filled diamond of the same shape rather than a font trace.
const DIAMOND_MARK_GLYPH: &[&str] = &[
    "....#....",
    "...###...",
    "..#####..",
    ".#######.",
    "#########",
    ".#######.",
    "..#####..",
    "...###...",
    "....#....",
];
/// Gap between the diamond mark and the accelerator letter that follows
/// it, and between the label and the start of the diamond.
const ACCEL_MARK_GAP: i32 = 4;

// The pill-shaped menu-item highlight below is OLGlyph too (encodings
// 24-29 for the endcaps, 30/35/40 for the tileable middle segments,
// `ol_button.c`'s BUTTON_UL/_LL/_LEFT_ENDCAP_FILL/_LR/_UR/_RIGHT_ENDCAP_FILL/
// _TOP_1/_BOTTOM_1/_FILL_1), confirmed from source (see
// docs/OPENLOOK-REFERENCE.md) that olwm's window menu and XView's own menu
// widget both call the same olgx_draw_accel_button (libolgx) for this --
// no "Sun vs olvwm" design split, one shape both share. Unlike the fixed-
// size button/pushpin/arrow glyphs above, a menu-item highlight has to
// stretch to whatever width a row's text needs -- OPEN LOOK's own
// technique is fixed endcap glyphs plus a 1-pixel-wide middle glyph
// repeated exactly enough times to reach the needed width (see
// draw_pill_highlight), rather than draw_glyph_bitmap's smooth aspect-fit
// scaling used for the fixed-size glyphs above.
//
// Each endcap is really two glyphs, not one: comparing them confirms the
// convention used throughout olshell's chrome (light source upper-left) --
// PILL_LEFT_TOP_ARC (encoding 24) traces *most* of the left edge (the lit
// majority), leaving just the final few rows at the bottom-left tip to
// PILL_LEFT_BOTTOM_ARC (encoding 25, the shadowed minority); the right
// endcap is the mirror image, PILL_RIGHT_BOTTOM_ARC (encoding 27) tracing
// most of it and PILL_RIGHT_TOP_ARC (encoding 28) just the lit tip at the
// very top. Both pairs are drawn in two different colors (top_color/
// bottom_color in draw_pill_highlight) to actually produce that split.
//
// All nine share the same top row once BDF's per-glyph vertical offset
// (BBX's yoff, which differs slightly between them) is accounted for --
// confirmed by computing each glyph's absolute top row from its own BBX
// height/yoff pair, which comes out identical (-1) for all nine, meaning
// they're already correctly relatively aligned index-for-index as written
// below with no extra shifting needed.
const PILL_LEFT_TOP_ARC: &[&str] = &[
    ".......####",
    ".....##....",
    "....#......",
    "...#.......",
    "..#........",
    ".#.........",
    ".#.........",
    "#..........",
    "#..........",
    "#..........",
    "#..........",
    "#..........",
    "#..........",
    "#..........",
    ".#.........",
    ".#.........",
    "..#........",
    "...#.......",
    "...........",
    "...........",
    "...........",
    "...........",
];
const PILL_LEFT_BOTTOM_ARC: &[&str] = &[
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "....#......",
    ".....##....",
    ".......####",
];
// Solid filled endcap for the fill layer, drawn last -- one native pixel
// inset from the outline arcs above on every edge (20 rows tall vs. their
// 21-22), an authentic detail: the fill never touches the outline stroke,
// leaving it visible all the way around rather than being drawn over.
const PILL_LEFT_FILL: &[&str] = &[
    "...........",
    ".......####",
    ".....######",
    "....#######",
    "...########",
    "..#########",
    "..#########",
    ".##########",
    ".##########",
    ".##########",
    ".##########",
    ".##########",
    ".##########",
    ".##########",
    "..#########",
    "..#########",
    "...########",
    "....#######",
    ".....######",
    ".......####",
];
const PILL_RIGHT_BOTTOM_ARC: &[&str] = &[
    "...........",
    "...........",
    "...........",
    ".......#...",
    "........#..",
    ".........#.",
    ".........#.",
    "..........#",
    "..........#",
    "..........#",
    "..........#",
    "..........#",
    "..........#",
    "..........#",
    ".........#.",
    ".........#.",
    "........#..",
    ".......#...",
    "......#....",
    "....##.....",
    "####.......",
];
const PILL_RIGHT_TOP_ARC: &[&str] = &[
    "####.......",
    "....##.....",
    "......#....",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
    "...........",
];
const PILL_RIGHT_FILL: &[&str] = &[
    "...........",
    "####.......",
    "######.....",
    "#######....",
    "########...",
    "#########..",
    "#########..",
    "##########.",
    "##########.",
    "##########.",
    "##########.",
    "##########.",
    "##########.",
    "##########.",
    "#########..",
    "#########..",
    "########...",
    "#######....",
    "######.....",
    "####.......",
];
// The three tileable middle segments, each just 1 native pixel wide --
// repeated exactly (needed_width - 2*endcap_width) times in
// draw_pill_highlight, which always divides evenly since the tile is
// exactly 1 pixel wide, unlike a smooth scale factor would.
const PILL_TOP_TILE: &[&str] = &["#", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", "."];
const PILL_BOTTOM_TILE: &[&str] = &["." , ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", ".", "#"];
const PILL_FILL_TILE: &[&str] = &[".", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "#", "."];

/// Native pixel width of each pill endcap glyph above (all eleven wide).
const PILL_ENDCAP_WIDTH: i32 = 11;
/// Native pixel height of the tallest pill glyph (PILL_LEFT_TOP_ARC) --
/// the overall height every layer is composited within.
const PILL_HEIGHT: i32 = 22;
/// Inset from a menu row's own left/right edges before the pill starts --
/// so it reads as a highlight sitting just inside the row rather than
/// touching the menu's own outer border.
const MENU_PILL_MARGIN: i32 = 2;
/// Downward nudge from pure geometric centering -- see draw_pill_
/// highlight's doc comment on why the text drawn over this glyph needs
/// it to actually look centered.
const PILL_VERTICAL_BIAS: i32 = 2;

/// Blits `bitmap` ('#' = on) at native pixel size (times `scale` for
/// HiDPI), top-left at logical (x0, y0) -- unlike draw_glyph_bitmap, no
/// fit-to-box scaling: draw_pill_highlight's endcap and tile pieces have
/// to stay at native pixel size and tile edge-to-edge exactly, not be
/// independently stretched to fill some box.
#[allow(clippy::too_many_arguments)]
fn blit_bitmap(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    bitmap: &[&str],
    color: (u8, u8, u8),
) {
    let (r, g, b) = color;
    for (row, line) in bitmap.iter().enumerate() {
        for (col, ch) in line.bytes().enumerate() {
            if ch != b'#' {
                continue;
            }
            let px0 = (x0 + col as i32) * scale;
            let py0 = (y0 + row as i32) * scale;
            for py in py0.max(0)..(py0 + scale).min(canvas_height) {
                for px in px0.max(0)..(px0 + scale).min(canvas_width) {
                    let idx = ((py * canvas_width + px) * 4) as usize;
                    canvas[idx] = b;
                    canvas[idx + 1] = g;
                    canvas[idx + 2] = r;
                    canvas[idx + 3] = 0xFF;
                }
            }
        }
    }
}

/// Draws the window-menu/root-menu/icon-menu item hover highlight: an
/// obround (pill) shape, not the plain rectangle this replaces --
/// authentic OPEN LOOK, per the OLGlyph provenance and stretchable-glyph
/// technique described above. Three layers, each a left endcap, N tiles,
/// and a right endcap composited at the same origin (matching
/// ol_button.c's own three-XDrawText-calls approach): top_color drawn
/// first (the lit majority of the left arc, the lit minority of the right
/// one), then bottom_color (the mirror image), then fill_color last on
/// top -- safe because the fill glyphs are inset by construction and
/// never overwrite the outline pixels either arc layer drew. Colors match
/// olgx_draw_button's "invoked" (hovered) 3D coloring: dark on top, light
/// on bottom, i.e. recessed/inset, the same convention
/// DECORATION_BEVEL_DARK/_LIGHT already use for a focused header.
#[allow(clippy::too_many_arguments)]
/// Menu-item hover highlight: the recessed/"invoked" coloring
/// (`OLGX_INVOKED`'s `BG3` top/`WHITE` bottom/`BG2` fill) `olgx_draw_
/// accel_button` uses, via the same shared `draw_pill` a real standalone
/// button (see draw_button below, used by the Notice's own buttons) draws
/// with too -- see draw_pill's doc comment for the shape itself.
fn draw_pill_highlight(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) {
    draw_pill(
        canvas, canvas_width, canvas_height, scale, x0, y0, x1, y1,
        DECORATION_BEVEL_DARK, DECORATION_BEVEL_LIGHT, MENU_HOVER_COLOR,
    );
}

/// The obround (pill) shape shared by the menu-item hover highlight above
/// and a real standalone button (draw_button, below) -- both are
/// `olgx_draw_button`/`olgx_draw_accel_button` in real OPEN LOOK, the same
/// stretchable-glyph composite, just with different colors for different
/// states (raised/unpressed vs. recessed/invoked) and, for a menu-item
/// highlight, with a caller-drawn label rather than one centered inside
/// the shape itself. See PILL_LEFT_TOP_ARC's doc comment for what each
/// layer actually traces and why three separate colors compose into one
/// shape.
#[allow(clippy::too_many_arguments)]
fn draw_pill(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    top_color: (u8, u8, u8),
    bottom_color: (u8, u8, u8),
    fill_color: (u8, u8, u8),
) {
    let tiles = (x1 - x0 - 2 * PILL_ENDCAP_WIDTH).max(0);
    // Centering purely on PILL_HEIGHT within the row leaves the pill
    // sitting a couple pixels higher than the text drawn over it --
    // confirmed live (a screenshot showed visibly more empty pill above
    // "Close" than below it) and measured precisely by rendering the real
    // glyphs and this glyph together and comparing their pixel centers
    // (1.5 logical pixels apart) rather than guessing: draw_text_row_
    // centered's own baseline formula (row-center plus a downward bias of
    // size/3, tuned for its many other plain-rectangle callers) sits text
    // slightly lower than this glyph's own geometric center, invisible
    // against the flat rectangle this replaced but obvious against a
    // shape with a visible top/bottom edge. PILL_VERTICAL_BIAS closes
    // that gap empirically rather than deriving it from font metrics,
    // matching how OPEN LOOK's own bitmap chrome was tuned by eye too.
    let y = y0 + ((y1 - y0) - PILL_HEIGHT) / 2 + PILL_VERTICAL_BIAS;
    let right_x = x0 + PILL_ENDCAP_WIDTH + tiles;

    blit_bitmap(canvas, canvas_width, canvas_height, scale, x0, y, PILL_LEFT_TOP_ARC, top_color);
    for i in 0..tiles {
        blit_bitmap(canvas, canvas_width, canvas_height, scale, x0 + PILL_ENDCAP_WIDTH + i, y, PILL_TOP_TILE, top_color);
    }
    blit_bitmap(canvas, canvas_width, canvas_height, scale, right_x, y, PILL_RIGHT_TOP_ARC, top_color);

    blit_bitmap(canvas, canvas_width, canvas_height, scale, x0, y, PILL_LEFT_BOTTOM_ARC, bottom_color);
    for i in 0..tiles {
        blit_bitmap(canvas, canvas_width, canvas_height, scale, x0 + PILL_ENDCAP_WIDTH + i, y, PILL_BOTTOM_TILE, bottom_color);
    }
    blit_bitmap(canvas, canvas_width, canvas_height, scale, right_x, y, PILL_RIGHT_BOTTOM_ARC, bottom_color);

    blit_bitmap(canvas, canvas_width, canvas_height, scale, x0, y, PILL_LEFT_FILL, fill_color);
    for i in 0..tiles {
        blit_bitmap(canvas, canvas_width, canvas_height, scale, x0 + PILL_ENDCAP_WIDTH + i, y, PILL_FILL_TILE, fill_color);
    }
    blit_bitmap(canvas, canvas_width, canvas_height, scale, right_x, y, PILL_RIGHT_FILL, fill_color);
}

/// A standalone clickable button, the "oblong button" from the widget
/// vocabulary table -- unlike draw_pill_highlight (a highlight drawn
/// *under* a label the caller draws separately, left-aligned), this draws
/// its own label centered inside the shape, and its coloring reflects
/// pressed state rather than always being the recessed/invoked look:
/// raised (light top/dark bottom) unless `pressed`, matching the same
/// raised-unless-invoked convention used throughout olshell's chrome
/// (see the window decoration header's own focus-bevel paragraph in
/// docs/DESIGN.md). Currently only the Notice's Exit/Cancel buttons use
/// this; MENU_BG_COLOR as the normal fill blends into the Notice's own
/// background, reading as a raised bump on it, matching real OPEN LOOK's
/// `OLGX_BG1` fill for a normal (non-menu-item) button.
#[allow(clippy::too_many_arguments)]
fn draw_button(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    label: &str,
    font: &fontdue::Font,
    pressed: bool,
) {
    let (top_color, bottom_color, fill_color) = if pressed {
        (DECORATION_BEVEL_DARK, DECORATION_BEVEL_LIGHT, MENU_HOVER_COLOR)
    } else {
        (DECORATION_BEVEL_LIGHT, DECORATION_BEVEL_DARK, MENU_BG_COLOR)
    };
    draw_pill(canvas, canvas_width, canvas_height, scale, x0, y0, x1, y1, top_color, bottom_color, fill_color);
    let label_width: i32 =
        label.chars().map(|c| font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32).sum();
    let label_x = x0 + ((x1 - x0) - label_width) / 2;
    draw_text_row_centered(
        canvas, canvas_width, scale, y0, y1 - y0, label_x, label, font, MENU_FONT_SIZE, MENU_TEXT_COLOR,
    );
}

/// Renders one of the bitmaps above into box (x0,y0)-(x1,y1): scaled
/// *uniformly* (the same factor on both axes, so the glyph's own
/// proportions are never stretched) to the largest size that fits within
/// the box, then centered. draw_pushpin shares one box between two
/// states with different native aspect ratios (the pinned glyph is a
/// compact square, the unpinned one nearly 2:1 wide -- see
/// PUSHPIN_GLYPH_PINNED/UNPINNED's doc comments); independently
/// stretching each axis to exactly fill the box, the first attempt at
/// this function, squashed whichever glyph was further from the box's
/// own aspect ratio -- confirmed live as the unpinned pushpin looking
/// noticeably horizontally compressed.
///
/// Within the centered sub-box, each destination pixel is on if *any*
/// source pixel in the region it covers is on, rather than point-
/// sampling a single nearest source pixel -- plain nearest-neighbor
/// dropped nearly all of the pushpin's unpinned glyph on an even earlier
/// attempt, since most sampled points landed in the gaps between one of
/// its 1px-wide outline strokes. Shared by draw_button_glyph and
/// draw_pushpin so both go through the same scaling logic.
#[allow(clippy::too_many_arguments)]
fn draw_glyph_bitmap(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    bitmap: &[&str],
    color: (u8, u8, u8),
) {
    let (x0, y0, x1, y1) = (x0 * scale, y0 * scale, x1 * scale, y1 * scale);
    let (r, g, b) = color;
    let src_h = bitmap.len() as i32;
    let src_w = bitmap.first().map_or(0, |row| row.len() as i32);
    let box_w = x1 - x0;
    let box_h = y1 - y0;
    if src_h == 0 || src_w == 0 || box_w <= 0 || box_h <= 0 {
        return;
    }
    let scale = (box_w as f64 / src_w as f64).min(box_h as f64 / src_h as f64);
    let dst_w = ((src_w as f64) * scale).round().max(1.0) as i32;
    let dst_h = ((src_h as f64) * scale).round().max(1.0) as i32;
    let off_x = x0 + (box_w - dst_w) / 2;
    let off_y = y0 + (box_h - dst_h) / 2;

    let rows: Vec<&[u8]> = bitmap.iter().map(|row| row.as_bytes()).collect();
    for dy in 0..dst_h {
        let sy0 = (dy * src_h / dst_h).clamp(0, src_h - 1);
        let sy1 = (((dy + 1) * src_h + dst_h - 1) / dst_h).clamp(sy0 + 1, src_h);
        for dx in 0..dst_w {
            let sx0 = (dx * src_w / dst_w).clamp(0, src_w - 1);
            let sx1 = (((dx + 1) * src_w + dst_w - 1) / dst_w).clamp(sx0 + 1, src_w);
            let on = (sy0..sy1)
                .any(|sy| (sx0..sx1).any(|sx| rows[sy as usize].get(sx as usize) == Some(&b'#')));
            if !on {
                continue;
            }
            let px = off_x + dx;
            let py = off_y + dy;
            if px < 0 || py < 0 || px >= canvas_width || py >= canvas_height {
                continue;
            }
            let idx = ((py * canvas_width + px) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
}

/// Draws the pushpin glyph within box (x0,y0)-(x1,y1): the pinned or
/// unpinned OLGlyph shape (see PUSHPIN_GLYPH_PINNED/UNPINNED), scaled to
/// fit.
#[allow(clippy::too_many_arguments)]
fn draw_pushpin(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    pinned: bool,
    color: (u8, u8, u8),
) {
    let bitmap = if pinned { PUSHPIN_GLYPH_PINNED } else { PUSHPIN_GLYPH_UNPINNED };
    draw_glyph_bitmap(canvas, canvas_width, canvas_height, scale, x0, y0, x1, y1, bitmap, color);
}

/// Fills one full-width logical row of `canvas` with an opaque color --
/// used for the light/dark bevel edges along a decoration header's top and
/// bottom. `y` is logical and becomes a `scale`-pixel-tall physical band,
/// same reasoning as the bold-text offset above: a 1-logical-pixel bevel
/// line should stay a proportional single line at any scale, not shrink to
/// a hairline-thin single physical pixel once scale > 1.
fn paint_row(canvas: &mut [u8], canvas_width: i32, scale: i32, y: i32, color: (u8, u8, u8)) {
    let (r, g, b) = color;
    for yy in (y * scale)..(y * scale + scale) {
        for x in 0..canvas_width {
            let idx = ((yy * canvas_width + x) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
}

/// Panel-local (x0, x1) span of workspace segment `index` (0-based). Shared
/// by drawing and hit-testing so they can never disagree about where a
/// segment actually is.
fn workspace_segment_x(index: u32) -> (i32, i32) {
    let x0 = WORKSPACE_STRIP_MARGIN + index as i32 * WORKSPACE_SEGMENT_WIDTH;
    (x0, x0 + WORKSPACE_SEGMENT_WIDTH - WORKSPACE_SEGMENT_GAP)
}

/// Background-local (x0, y0, x1, y1) box of icon tray slot `index`
/// (0-based, left to right, bottom-anchored) given the background's own
/// height. Shared by drawing and hit-testing, same reasoning as
/// workspace_segment_x. Doesn't wrap to a second row -- a rare enough
/// number of simultaneously-minimized windows on one output that it
/// isn't worth the complexity yet; they just keep marching right.
fn icon_rect(index: usize, bg_height: i32) -> (i32, i32, i32, i32) {
    let x0 = ICON_GAP + index as i32 * (ICON_SIZE + ICON_GAP);
    let y1 = bg_height - ICON_MARGIN_BOTTOM - ICON_LABEL_HEIGHT;
    let y0 = y1 - ICON_SIZE;
    (x0, y0, x0 + ICON_SIZE, y1)
}

// canvas_width/canvas_height are physical pixels (the buffer's actual
// size, already multiplied by scale by the caller); every other
// coordinate argument -- x0/y0/x1/y1 here, and likewise throughout every
// other primitive below that takes a `scale` -- stays in the same logical
// units the rest of each draw_* function's layout math already uses, and
// gets multiplied by `scale` right here, once, rather than at every call
// site. See docs/DESIGN.md's HiDPI entry for why this split (logical
// layout, scaled only at the final raster step) is the design.
#[allow(clippy::too_many_arguments)]
fn fill_rect(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: (u8, u8, u8),
) {
    let (x0, y0, x1, y1) = (x0 * scale, y0 * scale, x1 * scale, y1 * scale);
    let (r, g, b) = color;
    for y in y0.max(0)..y1.min(canvas_height) {
        for x in x0.max(0)..x1.min(canvas_width) {
            let idx = ((y * canvas_width + x) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
}

/// Draws the window-menu button's full glyph -- housing and chevron
/// together, since OLGlyph's own bitmap includes both (see
/// BUTTON_GLYPH_NORMAL) -- into the given box, scaled to fit. `inverted`
/// selects the pressed-state glyph; see BUTTON_GLYPH_PRESSED's doc comment
/// for why olshell's only caller passes hover for this.
#[allow(clippy::too_many_arguments)]
fn draw_button_glyph(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    inverted: bool,
    color: (u8, u8, u8),
) {
    let bitmap = if inverted { BUTTON_GLYPH_PRESSED } else { BUTTON_GLYPH_NORMAL };
    draw_glyph_bitmap(canvas, canvas_width, canvas_height, scale, x0, y0, x1, y1, bitmap, color);
}

/// Draws a small rightward-pointing wedge -- the window menu's indicator
/// that an item opens a submenu rather than acting immediately -- traced
/// from OLGlyph, see SUBMENU_ARROW_GLYPH's doc comment.
#[allow(clippy::too_many_arguments)]
fn draw_submenu_arrow(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    scale: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: (u8, u8, u8),
) {
    draw_glyph_bitmap(canvas, canvas_width, canvas_height, scale, x0, y0, x1, y1, SUBMENU_ARROW_GLYPH, color);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_at(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    start_x: i32,
    baseline_y: i32,
    text: &str,
    font: &fontdue::Font,
    size: f32,
    color: (u8, u8, u8),
) -> i32 {
    let (r, g, b) = color;
    let mut x = start_x;

    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, size);
        let glyph_x0 = x + metrics.xmin;
        let glyph_y0 = baseline_y - metrics.height as i32 - metrics.ymin;

        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let coverage = bitmap[row * metrics.width + col] as u32;
                if coverage == 0 {
                    continue;
                }
                let px = glyph_x0 + col as i32;
                let py = glyph_y0 + row as i32;
                if px < 0 || py < 0 || px >= canvas_width || py >= canvas_height {
                    continue;
                }
                let idx = ((py * canvas_width + px) * 4) as usize;
                for (i, fg) in [b, g, r].into_iter().enumerate() {
                    let bg = canvas[idx + i] as u32;
                    canvas[idx + i] = ((fg as u32 * coverage + bg * (255 - coverage)) / 255) as u8;
                }
            }
        }

        x += metrics.advance_width.round() as i32;
    }

    x
}

impl CompositorHandler for Olshell {
    /// Dispatches to whichever of the six scale-tracked surface owners
    /// `surface` belongs to (see each struct's own `scale` field doc
    /// comment for why exactly these six and not, say, WorkspaceSubmenu or
    /// Decoration's child chrome pieces too), using the same surface-
    /// identity helpers pointer_frame already relies on for this. Each
    /// arm's own draw_* call does the actual `set_buffer_scale` (see
    /// fill_rect's doc comment for the logical/physical split every one of
    /// them follows) -- this function only updates the tracked `scale`
    /// field and asks for a redraw.
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if let Some(i) = self.background_at(surface) {
            self.backgrounds[i].scale = new_factor;
            self.request_background_redraw(qh, i);
        } else if let Some(i) = self.panel_at(surface) {
            self.panels[i].scale = new_factor;
            self.draw_panel(i);
        } else if let Some(toplevel_id) = self.decoration_toplevel_id(surface) {
            if let Some(dec) = self.toplevels.get_mut(&toplevel_id).and_then(|info| info.decoration.as_mut()) {
                dec.scale = new_factor;
            }
            // Redraws the header and all seven child chrome pieces
            // (footer/corners/borders), which read this same scale --
            // see Decoration::scale's doc comment.
            self.draw_decoration(&toplevel_id);
        } else if self.window_menu.as_ref().is_some_and(|wm| wm.surface == *surface) {
            if let Some(wm) = self.window_menu.as_mut() {
                wm.scale = new_factor;
            }
            self.draw_window_menu();
            if self.window_menu.as_ref().is_some_and(|wm| wm.workspace_submenu.is_some()) {
                self.draw_workspace_submenu();
            }
        } else if self.icon_menu.as_ref().is_some_and(|im| im.surface == *surface) {
            if let Some(im) = self.icon_menu.as_mut() {
                im.scale = new_factor;
            }
            self.draw_icon_menu();
        } else if let Some(i) = self.popup_at(surface) {
            self.popups[i].scale = new_factor;
            draw_popup(&mut self.pool, &self.font, &self.popups[i]);
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    // Fires once for each wl_surface.frame callback request_background_redraw
    // makes -- the only caller of that request today. Clears the
    // outstanding-callback flag and, if a redraw was deferred while it was
    // outstanding, performs it now (requesting the next callback in turn).
    fn frame(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _time: u32) {
        let Some(index) = self.background_at(surface) else {
            return;
        };
        self.backgrounds[index].frame_requested = false;
        if self.backgrounds[index].redraw_pending {
            self.backgrounds[index].redraw_pending = false;
            self.request_background_redraw(qh, index);
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Olshell {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    /// Creates this output's background and panel (see BackgroundOutput's
    /// and WorkspacePanel's doc comments) -- fires for every output that
    /// exists at startup too, once the event loop starts dispatching, not
    /// just ones that appear later. The background doesn't depend on any
    /// optional protocol, so it's always created; the panel needs
    /// openlook-workspaces, so if that isn't available this output just
    /// gets a background (and therefore a working root menu) but no panel
    /// -- same degrade-gracefully spirit as the other optional protocols,
    /// just at the whole-panel granularity instead of finer.
    fn new_output(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let bg_surface = self.compositor.create_surface(qh);
        let bg_layer = self.layer_shell.create_layer_surface(
            qh,
            bg_surface,
            Layer::Background,
            Some("olshell-background"),
            Some(&output),
        );
        bg_layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        bg_layer.set_size(0, 0);
        bg_layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        bg_layer.commit();
        self.backgrounds.push(BackgroundOutput {
            output: output.clone(),
            layer: bg_layer,
            width: 0,
            height: 0,
            scale: 1,
            hovered_icon: None,
            selected_icons: Vec::new(),
            last_icon_click: None,
            drag: None,
            frame_requested: false,
            redraw_pending: false,
        });

        let Some(manager) = self.workspaces_manager.as_ref() else {
            return;
        };

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Top,
            Some("olshell-panel"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, PANEL_HEIGHT);
        layer.set_exclusive_zone(PANEL_HEIGHT as i32);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();

        let workspaces = manager.get_output_workspaces(&output, qh, ());

        self.panels.push(WorkspacePanel {
            output,
            layer,
            workspaces,
            width: 0,
            height: PANEL_HEIGHT,
            scale: 1,
            workspace_count: 0,
            active_workspace: 0,
            hovered_workspace: None,
        });
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {}

    /// The client-side half of tearing a panel and background down when
    /// their monitor goes away -- olcore doesn't send anything new for
    /// this (see output_destroy's doc comment in core/main.c): the
    /// ordinary wl_output global going away is already the standard
    /// signal, and sctk surfaces it here. If that was the last output,
    /// there's nothing left to show a background, panel, or root menu on,
    /// so olshell exits -- the same role a single global background's
    /// closed event used to play back when there was only ever one.
    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(index) = self.panels.iter().position(|p| p.output == output) {
            let panel = self.panels.remove(index);
            panel.workspaces.destroy();
            // panel.layer's own Drop impl destroys the layer surface and
            // its underlying wl_surface -- same as elsewhere in olshell.
        }
        if let Some(index) = self.backgrounds.iter().position(|b| b.output == output) {
            self.backgrounds.remove(index);
        }
        if self.backgrounds.is_empty() {
            self.exit = true;
        }
    }
}

impl LayerShellHandler for Olshell {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // A background closing (typically its output going away -- see
        // output_destroyed, which handles the same removal from that
        // side; this lookup is idempotent so whichever fires first wins)
        // takes the whole shell down only once every last one is gone --
        // a single panel or background closing on its own, with other
        // outputs still up, shouldn't.
        if let Some(index) = self.background_at(layer.wl_surface()) {
            self.backgrounds.remove(index);
            if self.backgrounds.is_empty() {
                self.exit = true;
            }
        } else if let Some(index) = self.popup_at(layer.wl_surface()) {
            self.close_menu(index);
        } else if self.notice.as_ref().is_some_and(|n| n.layer.wl_surface() == layer.wl_surface()) {
            self.close_notice();
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(index) = self.panels.iter().position(|p| layer.wl_surface() == p.layer.wl_surface()) {
            let panel = &mut self.panels[index];
            panel.width = configure.new_size.0;
            panel.height = if configure.new_size.1 > 0 {
                configure.new_size.1
            } else {
                PANEL_HEIGHT
            };
            self.draw_panel(index);
        } else if let Some(index) = self.background_at(layer.wl_surface()) {
            let bg = &mut self.backgrounds[index];
            bg.width = configure.new_size.0;
            bg.height = configure.new_size.1;
            self.request_background_redraw(qh, index);
        } else if let Some(index) = self.popup_at(layer.wl_surface()) {
            let popup = &mut self.popups[index];
            if configure.new_size.0 > 0 {
                popup.width = configure.new_size.0;
            }
            if configure.new_size.1 > 0 {
                popup.height = configure.new_size.1;
            }
            draw_popup(&mut self.pool, &self.font, popup);
        } else if self.notice.as_ref().is_some_and(|n| n.layer.wl_surface() == layer.wl_surface()) {
            // button_rects were computed from the size we requested in
            // open_notice and stay valid as long as the compositor just
            // echoes it back, which wlr-layer-shell always does for an
            // explicitly-sized surface like this one -- width/height are
            // updated defensively, but nothing recomputes button_rects.
            if let Some(notice) = self.notice.as_mut() {
                if configure.new_size.0 > 0 {
                    notice.width = configure.new_size.0;
                }
                if configure.new_size.1 > 0 {
                    notice.height = configure.new_size.1;
                }
            }
            self.draw_notice();
        }
    }
}

impl ShmHandler for Olshell {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for Olshell {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}
}

impl PointerHandler for Olshell {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let background_index = self.background_at(&event.surface);
            let panel_index = self.panel_at(&event.surface);
            let popup_index = self.popup_at(&event.surface);
            let decoration_toplevel = self.decoration_toplevel_id(&event.surface);
            let resize_region = self.resize_region_at(&event.surface);
            let on_window_menu =
                self.window_menu.as_ref().is_some_and(|wm| event.surface == wm.surface);
            let on_workspace_submenu = self
                .window_menu
                .as_ref()
                .and_then(|wm| wm.workspace_submenu.as_ref())
                .is_some_and(|sm| event.surface == sm.surface);
            let on_icon_menu = self.icon_menu.as_ref().is_some_and(|im| event.surface == im.surface);
            let on_notice = self.notice.as_ref().is_some_and(|n| event.surface == *n.layer.wl_surface());

            // A Notice is modal (see its own doc comment) -- while one's
            // open, every pointer event on any other surface is swallowed
            // outright, before any of the usual per-surface handling below
            // even runs. Checked ahead of the armed-drag/window-menu/icon-
            // menu pre-steps below too, since none of those should still
            // react to a click while a confirmation is pending either.
            if self.notice.is_some() && !on_notice {
                continue;
            }
            // Captured before the "click elsewhere closes it" step below
            // clears it, so the button-click handler can tell a second
            // click on the button that opened this exact menu (which
            // should toggle it closed) apart from a click that should
            // open one (a different toplevel's button, or this one after
            // the menu was already closed some other way).
            let window_menu_toplevel = self.window_menu.as_ref().map(|wm| wm.toplevel_id.clone());
            let icon_menu_toplevel = self.icon_menu.as_ref().map(|im| im.toplevel_id.clone());

            // A Move-item-armed icon move (see IconDrag's doc comment) ends
            // on the next press anywhere, not a matching Release -- there's
            // no button held for it to match. Checked and consumed before
            // anything else a press might otherwise do, since "confirm the
            // pending move" takes priority over whatever's actually under
            // the pointer (the same way a real window move grab consumes
            // the confirming click entirely rather than also delivering it
            // to whatever's underneath).
            if let PointerEventKind::Press { .. } = event.kind {
                let armed = self.backgrounds.iter().position(|bg| bg.drag.as_ref().is_some_and(|d| d.armed));
                if let Some(i) = armed {
                    self.backgrounds[i].drag = None;
                    self.request_background_redraw(qh, i);
                    continue;
                }
            }

            // No keyboard focus on the window menu yet (see WindowMenu's
            // doc comment), so a press anywhere else is how it closes --
            // handled up front so it applies regardless of what else that
            // press goes on to do below (e.g. opening a different one).
            // A press on the workspace submenu doesn't count as "elsewhere"
            // either, even though it's a different surface than the window
            // menu itself -- it's still part of the same menu from the
            // user's perspective.
            if let PointerEventKind::Press { .. } = event.kind {
                if self.window_menu.is_some() && !on_window_menu && !on_workspace_submenu {
                    self.close_window_menu();
                }
                // Same reasoning, for the icon menu.
                if self.icon_menu.is_some() && !on_icon_menu {
                    self.close_icon_menu();
                }
            }

            match event.kind {
                // MENU (right-button) on an icon opens its own popup
                // (Open/Move/Properties) instead of the root menu, same as
                // a real OPEN LOOK icon would; a second MENU-click on the
                // icon whose menu is already open toggles it closed
                // instead of reopening it, same convention the window
                // menu's own header button uses.
                PointerEventKind::Press { button, .. } if background_index.is_some() && button == BTN_RIGHT => {
                    let i = background_index.unwrap();
                    let icon_index = self.icon_at(i, event.position.0, event.position.1);
                    let output = self.backgrounds[i].output.clone();
                    let active_workspace =
                        self.panels.iter().find(|p| p.output == output).map(|p| p.active_workspace);
                    let clicked_id = icon_index.and_then(|idx| {
                        let w = active_workspace?;
                        self.minimized_toplevels_for_output(&output, w).get(idx).cloned()
                    });
                    match clicked_id {
                        Some(id) if icon_menu_toplevel.as_ref() != Some(&id) => {
                            // If the clicked icon is already a member of a
                            // multi-selection (see selected_icons' doc
                            // comment), Open/Move act on the whole
                            // selection; otherwise MENU replaces the
                            // selection with just this one icon, same as a
                            // plain SELECT-click would.
                            let is_group_member = {
                                let selected = &self.backgrounds[i].selected_icons;
                                selected.len() > 1 && selected.contains(&id)
                            };
                            let group = if is_group_member {
                                self.backgrounds[i].selected_icons.clone()
                            } else {
                                self.backgrounds[i].selected_icons = vec![id.clone()];
                                self.request_background_redraw(qh, i);
                                vec![id.clone()]
                            };
                            self.open_icon_menu(qh, i, &id, group);
                        }
                        Some(_) => {
                            // Toggling closed: the pre-step above already
                            // closed it since this press isn't on the menu
                            // itself, so there's nothing more to do here.
                        }
                        None => {
                            self.open_menu(qh, &output, event.position.0, event.position.1);
                        }
                    }
                }
                // ADJUST (middle-click) on an icon toggles it in/out of a
                // multi-selection without disturbing the rest -- see
                // selected_icons' doc comment for what actually consumes
                // that selection. Middle-clicking empty background, or an
                // icon that's already been removed from the tray, is a
                // no-op: ADJUST only ever modifies an existing selection,
                // never replaces or clears it the way a plain SELECT-click
                // on empty background does.
                PointerEventKind::Press { button, .. } if background_index.is_some() && button == BTN_MIDDLE => {
                    let i = background_index.unwrap();
                    let icon_index = self.icon_at(i, event.position.0, event.position.1);
                    let output = self.backgrounds[i].output.clone();
                    let active_workspace =
                        self.panels.iter().find(|p| p.output == output).map(|p| p.active_workspace);
                    let clicked_id = icon_index.and_then(|idx| {
                        let w = active_workspace?;
                        self.minimized_toplevels_for_output(&output, w).get(idx).cloned()
                    });
                    if let Some(id) = clicked_id {
                        let bg = &mut self.backgrounds[i];
                        match bg.selected_icons.iter().position(|s| *s == id) {
                            Some(pos) => {
                                bg.selected_icons.remove(pos);
                            }
                            None => bg.selected_icons.push(id),
                        }
                        self.request_background_redraw(qh, i);
                    }
                }
                PointerEventKind::Motion { .. } if background_index.is_some() => {
                    let i = background_index.unwrap();
                    // If SELECT is down on an icon, this motion either
                    // starts or continues a drag rather than updating
                    // hover -- see IconDrag's doc comment.
                    let drag = self.backgrounds[i]
                        .drag
                        .as_ref()
                        .map(|d| (d.icons.clone(), d.press_pos, d.dragging));
                    if let Some((icons, press_pos, already_dragging)) = drag {
                        let press_pos = match press_pos {
                            Some(p) => p,
                            None => {
                                // First motion after an armed (menu-
                                // triggered) move -- establish the
                                // reference point now, from wherever the
                                // pointer happens to be, so the icon(s)
                                // don't jump to reflect movement that
                                // happened before tracking started (the
                                // click that armed this happened on the
                                // icon menu's own surface, not
                                // necessarily anywhere near the icon).
                                if let Some(d) = self.backgrounds[i].drag.as_mut() {
                                    d.press_pos = Some(event.position);
                                }
                                event.position
                            }
                        };
                        let dx = event.position.0 - press_pos.0;
                        let dy = event.position.1 - press_pos.1;
                        if already_dragging || dx.hypot(dy) >= ICON_DRAG_THRESHOLD {
                            let bg = &self.backgrounds[i];
                            let max_x = (bg.width as i32 - ICON_SIZE).max(0);
                            let max_y = (bg.height as i32 - ICON_SIZE - ICON_LABEL_HEIGHT).max(0);
                            // Every dragged icon moves by the same delta,
                            // each clamped to the background independently
                            // -- see IconDrag::icons' doc comment.
                            for (id, origin) in &icons {
                                let new_x = (origin.0 + dx.round() as i32).clamp(0, max_x);
                                let new_y = (origin.1 + dy.round() as i32).clamp(0, max_y);
                                if let Some(info) = self.toplevels.get_mut(id) {
                                    info.icon_position = Some((new_x, new_y));
                                }
                            }
                            let bg = &mut self.backgrounds[i];
                            if let Some(d) = bg.drag.as_mut() {
                                d.dragging = true;
                            }
                            // Visual "picked up" feedback for as long as the
                            // drag lasts; stays selected after the drop too,
                            // same as most desktop icon dragging conventions
                            // -- a no-op assignment for a group drag, which
                            // was already exactly this set.
                            bg.selected_icons = icons.iter().map(|(id, _)| id.clone()).collect();
                            self.request_background_redraw(qh, i);
                        }
                    } else {
                        let hovered = self.icon_at(i, event.position.0, event.position.1);
                        if self.backgrounds[i].hovered_icon != hovered {
                            self.backgrounds[i].hovered_icon = hovered;
                            self.request_background_redraw(qh, i);
                        }
                    }
                }
                PointerEventKind::Leave { .. } if background_index.is_some() => {
                    let i = background_index.unwrap();
                    if self.backgrounds[i].hovered_icon.take().is_some() {
                        self.request_background_redraw(qh, i);
                    }
                }
                // SELECT (left-button) press on an icon starts tracking a
                // possible drag or click -- which one this turns out to be
                // isn't decided until Release (below), based on how far the
                // pointer moves while held (see IconDrag's doc comment).
                // Pressing the background outside any icon has no drag/click
                // ambiguity to defer -- it just clears whatever was selected,
                // immediately.
                PointerEventKind::Press { button, time, .. } if background_index.is_some() && button == BTN_LEFT => {
                    let i = background_index.unwrap();
                    let icon_index = self.icon_at(i, event.position.0, event.position.1);
                    let output = self.backgrounds[i].output.clone();
                    let active_workspace =
                        self.panels.iter().find(|p| p.output == output).map(|p| p.active_workspace);
                    let icon_ids = active_workspace.map(|w| self.minimized_toplevels_for_output(&output, w));
                    let clicked = icon_index.zip(icon_ids.as_ref()).and_then(|(idx, ids)| {
                        let height = self.backgrounds[i].height as i32;
                        let rects = self.icon_layout(ids, height);
                        let id = ids.get(idx)?.clone();
                        let &(x0, y0, ..) = rects.get(idx)?;
                        Some((id, (x0, y0), ids.clone(), rects))
                    });

                    if let Some((id, origin, ids, rects)) = clicked {
                        // If the pressed icon is already part of a multi-
                        // selection, the whole selection drags together --
                        // see IconDrag::icons' and selected_icons' doc
                        // comments.
                        let selected = self.backgrounds[i].selected_icons.clone();
                        let icons = if selected.len() > 1 && selected.contains(&id) {
                            selected
                                .iter()
                                .filter_map(|sid| {
                                    let idx = ids.iter().position(|i| i == sid)?;
                                    let &(x0, y0, ..) = rects.get(idx)?;
                                    Some((sid.clone(), (x0, y0)))
                                })
                                .collect()
                        } else {
                            vec![(id.clone(), origin)]
                        };
                        self.backgrounds[i].drag = Some(IconDrag {
                            primary: id,
                            press_time: time,
                            press_pos: Some(event.position),
                            icons,
                            dragging: false,
                            armed: false,
                        });
                    } else if !self.backgrounds[i].selected_icons.is_empty() {
                        self.backgrounds[i].selected_icons.clear();
                        self.request_background_redraw(qh, i);
                    }
                }
                // Ends whatever the matching Press started. A drag that
                // already moved the icon (see Motion above) just lets go;
                // otherwise this was a plain click -- select/highlight it,
                // or restore it if it's a second click on the same icon
                // within ICON_DOUBLE_CLICK_MS (double-click), matching
                // authentic OPEN LOOK.
                PointerEventKind::Release { button, .. } if background_index.is_some() && button == BTN_LEFT => {
                    let i = background_index.unwrap();
                    let Some(drag) = self.backgrounds[i].drag.take() else {
                        continue;
                    };
                    if drag.dragging {
                        continue;
                    }
                    let is_double_click = match &self.backgrounds[i].last_icon_click {
                        Some((last_id, last_time)) => {
                            *last_id == drag.primary
                                && drag.press_time.wrapping_sub(*last_time) <= ICON_DOUBLE_CLICK_MS
                        }
                        None => false,
                    };
                    let bg = &mut self.backgrounds[i];
                    if is_double_click {
                        bg.selected_icons.clear();
                        bg.last_icon_click = None;
                    } else {
                        // A plain click, even on an icon that was part of a
                        // multi-selection, collapses the selection down to
                        // just this one -- same convention as most desktop
                        // icon-selection models (a drag is what's needed to
                        // move the group without disturbing it, see
                        // IconDrag::icons' doc comment).
                        bg.selected_icons = vec![drag.primary.clone()];
                        bg.last_icon_click = Some((drag.primary.clone(), drag.press_time));
                    }
                    self.request_background_redraw(qh, i);

                    if is_double_click {
                        self.restore_toplevel(&drag.primary);
                    }
                }
                PointerEventKind::Motion { .. } if panel_index.is_some() => {
                    let i = panel_index.unwrap();
                    let hovered = Olshell::workspace_at(event.position.0, self.panels[i].workspace_count);
                    if self.panels[i].hovered_workspace != hovered {
                        self.panels[i].hovered_workspace = hovered;
                        self.draw_panel(i);
                    }
                }
                PointerEventKind::Leave { .. } if panel_index.is_some() => {
                    let i = panel_index.unwrap();
                    if self.panels[i].hovered_workspace.take().is_some() {
                        self.draw_panel(i);
                    }
                }
                PointerEventKind::Press { button, .. } if panel_index.is_some() && button == BTN_LEFT => {
                    let panel = &self.panels[panel_index.unwrap()];
                    if let Some(index) = Olshell::workspace_at(event.position.0, panel.workspace_count) {
                        panel.workspaces.switch_to(index);
                    }
                }
                // ADJUST (middle-click) a segment to move the focused
                // window there instead of switching to it -- borrowed
                // from modern multi-workspace desktops; no OPEN LOOK
                // precedent, but a fitting use for the ADJUST button
                // (extend/modify an existing selection) all the same.
                // Acts through the *clicked* panel's own workspaces
                // object, which is what makes this a real cross-monitor
                // move when the focused window is on a different output
                // than the one clicked -- see
                // output_workspaces_handle_assign_toplevel in olcore.
                PointerEventKind::Press { button, .. } if panel_index.is_some() && button == BTN_MIDDLE => {
                    let panel = &self.panels[panel_index.unwrap()];
                    if let Some(index) = Olshell::workspace_at(event.position.0, panel.workspace_count) {
                        if let Some(handle) = self.focused_toplevel_handle() {
                            panel.workspaces.assign_toplevel(&handle, index);
                        }
                    }
                }
                PointerEventKind::Motion { .. } if decoration_toplevel.is_some() => {
                    let id = decoration_toplevel.unwrap();
                    if let Some(dec) =
                        self.toplevels.get_mut(&id).and_then(|info| info.decoration.as_mut())
                    {
                        let hovered = dec.is_on_button(event.position.0, event.position.1);
                        if dec.button_hovered != hovered {
                            dec.button_hovered = hovered;
                            self.draw_decoration(&id);
                        }
                    }
                }
                PointerEventKind::Leave { .. } if decoration_toplevel.is_some() => {
                    let id = decoration_toplevel.unwrap();
                    if let Some(dec) =
                        self.toplevels.get_mut(&id).and_then(|info| info.decoration.as_mut())
                    {
                        if dec.button_hovered {
                            dec.button_hovered = false;
                            self.draw_decoration(&id);
                        }
                    }
                }
                PointerEventKind::Press { button, .. }
                    if decoration_toplevel.is_some() && button == BTN_LEFT =>
                {
                    let id = decoration_toplevel.unwrap();
                    let on_button = self
                        .toplevels
                        .get(&id)
                        .and_then(|info| info.decoration.as_ref())
                        .is_some_and(|dec| dec.is_on_button(event.position.0, event.position.1));
                    if on_button {
                        // A second click on the button that opened the
                        // currently-showing menu toggles it closed instead
                        // of reopening it -- the pre-step above already
                        // closed it since this press isn't on the menu
                        // itself, so there's nothing more to do here.
                        if window_menu_toplevel.as_ref() != Some(&id) {
                            self.open_window_menu(qh, &id);
                        }
                    } else if let Some(dec) =
                        self.toplevels.get(&id).and_then(|info| info.decoration.as_ref())
                    {
                        // A press anywhere else on the header drags the
                        // window, same as a real title bar. held=1: this
                        // fires from the press itself, so the button is
                        // still down -- the move ends when it's released.
                        dec.object._move(1);
                    }
                }
                PointerEventKind::Motion { .. } if resize_region.is_some() => {
                    let (id, region) = resize_region.unwrap();
                    if let Some(dec) =
                        self.toplevels.get_mut(&id).and_then(|info| info.decoration.as_mut())
                    {
                        let handle = dec.resize_handle_mut(region);
                        if !handle.hovered {
                            handle.hovered = true;
                            self.draw_decoration(&id);
                        }
                    }
                }
                PointerEventKind::Leave { .. } if resize_region.is_some() => {
                    let (id, region) = resize_region.unwrap();
                    if let Some(dec) =
                        self.toplevels.get_mut(&id).and_then(|info| info.decoration.as_mut())
                    {
                        let handle = dec.resize_handle_mut(region);
                        if handle.hovered {
                            handle.hovered = false;
                            self.draw_decoration(&id);
                        }
                    }
                }
                PointerEventKind::Press { button, .. } if resize_region.is_some() && button == BTN_LEFT => {
                    let (id, region) = resize_region.unwrap();
                    if let Some(dec) = self.toplevels.get(&id).and_then(|info| info.decoration.as_ref()) {
                        // held=1: this fires from the press itself, same
                        // reasoning as the header drag above -- a real
                        // press-hold-drag gesture, not a discrete click.
                        dec.object.resize(region.edges(), 1);
                    }
                }
                PointerEventKind::Motion { .. } if on_window_menu => {
                    // Computed before the mutable borrow below, same
                    // reasoning as draw_window_menu's identically-named
                    // local -- Move to Workspace is disabled while sticky.
                    let sticky = window_menu_toplevel.as_ref().is_some_and(|id| {
                        self.toplevels
                            .get(id)
                            .and_then(|info| info.decoration.as_ref())
                            .is_some_and(|dec| dec.sticky)
                    });
                    let changed = if let Some(wm) = self.window_menu.as_mut() {
                        let hovered = wm.item_at(event.position.1).filter(|&i| {
                            let item = &WINDOW_MENU_ITEMS[i];
                            !item.disabled
                                && !(sticky && matches!(item.action, WindowMenuAction::MoveToWorkspace))
                        });
                        if wm.hovered != hovered {
                            wm.hovered = hovered;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if changed {
                        self.draw_window_menu();
                    }
                }
                PointerEventKind::Leave { .. } if on_window_menu => {
                    let had_hover = self.window_menu.as_ref().is_some_and(|wm| wm.hovered.is_some());
                    if had_hover {
                        if let Some(wm) = self.window_menu.as_mut() {
                            wm.hovered = None;
                        }
                        self.draw_window_menu();
                    }
                }
                PointerEventKind::Press { button, .. } if on_window_menu && button == BTN_LEFT => {
                    let selection = self.window_menu.as_ref().and_then(|wm| {
                        wm.item_at(event.position.1).map(|index| (wm.toplevel_id.clone(), index))
                    });
                    // Everything else closes the whole window menu once
                    // handled; Move to Workspace instead opens (or closes)
                    // its submenu and leaves the window menu itself open,
                    // same as clicking the header button toggles the
                    // window menu without touching anything else.
                    let mut close_menu = true;
                    if let Some((toplevel_id, index)) = selection {
                        let item = &WINDOW_MENU_ITEMS[index];
                        let sticky = self
                            .toplevels
                            .get(&toplevel_id)
                            .and_then(|info| info.decoration.as_ref())
                            .is_some_and(|dec| dec.sticky);
                        let disabled = item.disabled
                            || (matches!(item.action, WindowMenuAction::MoveToWorkspace) && sticky);
                        if !disabled {
                            match item.action {
                                WindowMenuAction::Minimize => {
                                    if let Some(handle) =
                                        self.toplevels.get(&toplevel_id).and_then(|i| i.handle.as_ref())
                                    {
                                        handle.set_minimized();
                                    }
                                }
                                WindowMenuAction::ToggleMaximize => {
                                    if let Some(info) = self.toplevels.get(&toplevel_id) {
                                        if let Some(handle) = info.handle.as_ref() {
                                            if info.states.contains(&0) {
                                                handle.unset_maximized();
                                            } else {
                                                handle.set_maximized();
                                            }
                                        }
                                    }
                                }
                                WindowMenuAction::Move => {
                                    if let Some(dec) = self
                                        .toplevels
                                        .get(&toplevel_id)
                                        .and_then(|i| i.decoration.as_ref())
                                    {
                                        // held=0: this is a discrete menu
                                        // click, not a held drag -- see
                                        // the protocol doc comment on why
                                        // that has to be asserted rather
                                        // than inferred by olcore.
                                        dec.object._move(0);
                                    }
                                }
                                WindowMenuAction::Resize => {
                                    if let Some(dec) = self
                                        .toplevels
                                        .get(&toplevel_id)
                                        .and_then(|i| i.decoration.as_ref())
                                    {
                                        dec.object.resize(EDGE_BOTTOM | EDGE_RIGHT, 0);
                                    }
                                }
                                WindowMenuAction::Lower => {
                                    if let Some(dec) = self
                                        .toplevels
                                        .get(&toplevel_id)
                                        .and_then(|i| i.decoration.as_ref())
                                    {
                                        dec.object.lower();
                                    }
                                }
                                WindowMenuAction::Quit => {
                                    if let Some(dec) = self
                                        .toplevels
                                        .get(&toplevel_id)
                                        .and_then(|i| i.decoration.as_ref())
                                    {
                                        dec.object.quit();
                                    }
                                }
                                WindowMenuAction::ToggleSticky => {
                                    if let Some(dec) = self
                                        .toplevels
                                        .get(&toplevel_id)
                                        .and_then(|i| i.decoration.as_ref())
                                    {
                                        dec.object.toggle_sticky();
                                    }
                                }
                                WindowMenuAction::MoveToWorkspace => {
                                    if self
                                        .window_menu
                                        .as_ref()
                                        .is_some_and(|wm| wm.workspace_submenu.is_some())
                                    {
                                        self.close_workspace_submenu();
                                    } else {
                                        self.open_workspace_submenu(qh, &toplevel_id);
                                    }
                                    close_menu = false;
                                }
                                WindowMenuAction::Unimplemented => {
                                    log::info!("window menu: {} not yet implemented", item.label);
                                }
                            }
                        }
                    }
                    if close_menu {
                        self.close_window_menu();
                    }
                }
                PointerEventKind::Motion { .. } if on_icon_menu => {
                    let changed = if let Some(im) = self.icon_menu.as_mut() {
                        let hovered = im.item_at(event.position.1).filter(|&i| !ICON_MENU_ITEMS[i].disabled);
                        if im.hovered != hovered {
                            im.hovered = hovered;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if changed {
                        self.draw_icon_menu();
                    }
                }
                PointerEventKind::Leave { .. } if on_icon_menu => {
                    let had_hover = self.icon_menu.as_ref().is_some_and(|im| im.hovered.is_some());
                    if had_hover {
                        if let Some(im) = self.icon_menu.as_mut() {
                            im.hovered = None;
                        }
                        self.draw_icon_menu();
                    }
                }
                PointerEventKind::Press { button, .. } if on_icon_menu && button == BTN_LEFT => {
                    let selection = self.icon_menu.as_ref().and_then(|im| {
                        im.item_at(event.position.1)
                            .map(|index| (im.toplevel_id.clone(), im.group.clone(), im.background_index, index))
                    });
                    if let Some((toplevel_id, group, background_index, index)) = selection {
                        let item = &ICON_MENU_ITEMS[index];
                        if !item.disabled {
                            match item.action {
                                // Acts on the whole group -- just
                                // toplevel_id unless the menu was opened on
                                // an icon that was part of a multi-
                                // selection (see IconMenu::group's doc
                                // comment).
                                IconMenuAction::Open => {
                                    for id in &group {
                                        self.restore_toplevel(id);
                                    }
                                }
                                IconMenuAction::Move => {
                                    let output = self.backgrounds[background_index].output.clone();
                                    let active_workspace = self
                                        .panels
                                        .iter()
                                        .find(|p| p.output == output)
                                        .map(|p| p.active_workspace);
                                    let icons = active_workspace.map(|w| {
                                        let ids = self.minimized_toplevels_for_output(&output, w);
                                        let height = self.backgrounds[background_index].height as i32;
                                        let rects = self.icon_layout(&ids, height);
                                        group
                                            .iter()
                                            .filter_map(|gid| {
                                                let idx = ids.iter().position(|id| id == gid)?;
                                                let &(x0, y0, ..) = rects.get(idx)?;
                                                Some((gid.clone(), (x0, y0)))
                                            })
                                            .collect::<Vec<_>>()
                                    });
                                    if let Some(icons) = icons.filter(|icons| !icons.is_empty()) {
                                        let bg = &mut self.backgrounds[background_index];
                                        bg.drag = Some(IconDrag {
                                            primary: toplevel_id,
                                            press_time: 0,
                                            press_pos: None,
                                            icons,
                                            dragging: true,
                                            armed: true,
                                        });
                                        bg.selected_icons = group;
                                        self.request_background_redraw(qh, background_index);
                                    }
                                }
                                IconMenuAction::Unimplemented => {
                                    log::info!("icon menu: {} not yet implemented", item.label);
                                }
                            }
                        }
                    }
                    self.close_icon_menu();
                }
                // Mouse-over alone never changes a notice button's
                // appearance in real olwm (see Notice::pressed's doc
                // comment) -- this only matters while a button is already
                // held (`pressed.is_some()`), to cancel the press if the
                // pointer leaves it before Release, matching
                // noticeInterposer's own MotionNotify handling.
                PointerEventKind::Motion { .. } if on_notice => {
                    if let Some(notice) = self.notice.as_mut() {
                        if let Some(pressed) = notice.pressed {
                            if notice.button_at(event.position.0, event.position.1) != Some(pressed) {
                                notice.pressed = None;
                                self.draw_notice();
                            }
                        }
                    }
                }
                PointerEventKind::Press { button, .. } if on_notice && button == BTN_LEFT => {
                    if let Some(notice) = self.notice.as_mut() {
                        if let Some(index) = notice.button_at(event.position.0, event.position.1) {
                            notice.pressed = Some(index);
                            self.draw_notice();
                        }
                    }
                }
                // Only a button that's still depressed (pressed.is_some())
                // and still under the pointer counts as clicked, matching
                // noticeInterposer's own ButtonRelease handling -- Motion
                // above already cleared `pressed` if the pointer left the
                // button first, so reaching here with a match means this
                // release really did land on the same button it started
                // on.
                PointerEventKind::Release { button, .. } if on_notice && button == BTN_LEFT => {
                    let selection = self.notice.as_ref().and_then(|notice| {
                        let index = notice.button_at(event.position.0, event.position.1)?;
                        (notice.pressed == Some(index)).then_some(index)
                    });
                    if let Some(0) = selection {
                        if let Some(manager) = self.session_manager.as_ref() {
                            manager.exit();
                        }
                    }
                    if selection.is_some() {
                        self.close_notice();
                    }
                }
                PointerEventKind::Motion { .. } if on_workspace_submenu => {
                    let changed = if let Some(sm) =
                        self.window_menu.as_mut().and_then(|wm| wm.workspace_submenu.as_mut())
                    {
                        let hovered = sm.item_at(event.position.1).filter(|&i| sm.is_selectable(i));
                        if sm.hovered != hovered {
                            sm.hovered = hovered;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if changed {
                        self.draw_workspace_submenu();
                    }
                }
                PointerEventKind::Leave { .. } if on_workspace_submenu => {
                    let had_hover = self
                        .window_menu
                        .as_ref()
                        .and_then(|wm| wm.workspace_submenu.as_ref())
                        .is_some_and(|sm| sm.hovered.is_some());
                    if had_hover {
                        if let Some(sm) =
                            self.window_menu.as_mut().and_then(|wm| wm.workspace_submenu.as_mut())
                        {
                            sm.hovered = None;
                        }
                        self.draw_workspace_submenu();
                    }
                }
                PointerEventKind::Press { button, .. } if on_workspace_submenu && button == BTN_LEFT => {
                    let selection = self.window_menu.as_ref().and_then(|wm| {
                        let sm = wm.workspace_submenu.as_ref()?;
                        let row_index = sm.item_at(event.position.1).filter(|&i| sm.is_selectable(i))?;
                        let WorkspaceSubmenuRow::Workspace { output, index, .. } = &sm.rows[row_index] else {
                            unreachable!("is_selectable guarantees a Workspace row");
                        };
                        Some((wm.toplevel_id.clone(), output.clone(), *index))
                    });
                    if let Some((toplevel_id, output, index)) = selection {
                        if let (Some(panel), Some(handle)) = (
                            self.panels.iter().find(|p| p.output == output),
                            self.toplevels.get(&toplevel_id).and_then(|i| i.handle.as_ref()),
                        ) {
                            panel.workspaces.assign_toplevel(handle, index);
                        }
                    }
                    // The whole point was picking a workspace -- done now,
                    // regardless of whether the press landed on a real row
                    // or the disabled current-workspace one.
                    self.close_window_menu();
                }
                PointerEventKind::Motion { .. } if popup_index.is_some() => {
                    let popup = &mut self.popups[popup_index.unwrap()];
                    let hovered = popup.item_at(event.position.1);
                    if popup.hovered != hovered {
                        popup.hovered = hovered;
                        draw_popup(&mut self.pool, &self.font, popup);
                    }
                }
                PointerEventKind::Release { button, .. } if button == BTN_RIGHT => {
                    let mut command_to_run = None;
                    let mut exit_requested_on = None;
                    let mut close_index = None;

                    if let Some(i) = popup_index {
                        let popup = &mut self.popups[i];
                        if popup.is_on_pushpin(event.position.0, event.position.1) {
                            // Pinning a transient popup makes it persistent;
                            // clicking the pushpin again on an already-
                            // pinned popup is how you dismiss it, since
                            // there's no button-hold to release into once
                            // it's just sitting there open.
                            if popup.pinned {
                                close_index = Some(i);
                            } else {
                                popup.pinned = true;
                                draw_popup(&mut self.pool, &self.font, popup);
                            }
                        } else if let Some(item_index) = popup.item_at(event.position.1) {
                            match &popup.items[item_index] {
                                MenuNode::Item { command, .. } => {
                                    command_to_run = Some(command.clone());
                                }
                                MenuNode::Submenu { .. } => {
                                    log::info!("root menu: submenus aren't interactive yet");
                                }
                                MenuNode::Exit { .. } => {
                                    exit_requested_on = Some(popup.output.clone());
                                }
                            }
                            if !popup.pinned {
                                close_index = Some(i);
                            }
                        } else if !popup.pinned {
                            // Released on this popup's own padding, not an
                            // item or the pushpin.
                            close_index = Some(i);
                        }
                    } else if let Some(i) = self.popups.iter().position(|p| !p.pinned) {
                        // Released off any popup entirely -- that's how the
                        // transient one (there's at most one at a time, see
                        // open_menu) gets dismissed. A pinned popup, on this
                        // output or any other, stays up through this, same
                        // as a real persistent palette would.
                        close_index = Some(i);
                    }

                    if let Some(command) = command_to_run {
                        Self::run_command(&command);
                    }
                    // Doesn't terminate anything itself -- opens the
                    // confirmation Notice (see its own doc comment), which
                    // is what actually sends session_manager.exit() if its
                    // Exit button is clicked.
                    if let Some(output) = exit_requested_on {
                        self.open_notice(qh, &output);
                    }
                    if let Some(i) = close_index {
                        self.close_menu(i);
                    }
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for Olshell {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.clone());
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        if self.keyboard_focus.as_ref() == Some(surface) {
            self.keyboard_focus = None;
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        // The only surfaces we ever request keyboard focus for are a
        // root-menu popup (wlr-layer-shell's Exclusive interactivity, see
        // MenuPopup's doc comment), the window menu, and the icon menu
        // (the latter two via openlook-decoration's grab_keyboard, see
        // each struct's own doc comment) -- keyboard_focus is how Escape
        // tells which one, if any, olcore actually handed focus to, so
        // only that one closes.
        if event.keysym == Keysym::Escape {
            let Some(surface) = self.keyboard_focus.as_ref() else {
                return;
            };
            if let Some(index) = self.popup_at(surface) {
                self.close_menu(index);
            } else if self.window_menu.as_ref().is_some_and(|wm| wm.surface == *surface) {
                // One level at a time, same as clicking Move to Workspace
                // again closes just the submenu it opened rather than the
                // whole window menu -- keyboard focus stays on the window
                // menu's own surface throughout (see WindowMenu's doc
                // comment), so this is the only way Escape can tell the
                // submenu is the topmost thing open right now.
                if self.window_menu.as_ref().is_some_and(|wm| wm.workspace_submenu.is_some()) {
                    self.close_workspace_submenu();
                } else {
                    self.close_window_menu();
                }
            } else if self.icon_menu.as_ref().is_some_and(|im| im.surface == *surface) {
                self.close_icon_menu();
            } else if self.notice.as_ref().is_some_and(|n| *n.layer.wl_surface() == *surface) {
                // Matches ACTION_CANCEL in noticeInterposer -- dismisses
                // without acting, same as Return does just below (since
                // NOTICE_DEFAULT_BUTTON is Cancel here too).
                self.close_notice();
            }
        } else if event.keysym == Keysym::Return {
            // Matches ACTION_EXEC_DEFAULT in noticeInterposer: Return
            // always triggers the *default* button, not necessarily the
            // one being hovered/pressed -- NOTICE_DEFAULT_BUTTON is
            // Cancel, so this just dismisses, same as Escape.
            if self
                .keyboard_focus
                .as_ref()
                .is_some_and(|surface| self.notice.as_ref().is_some_and(|n| n.layer.wl_surface() == surface))
            {
                if NOTICE_DEFAULT_BUTTON == 0 {
                    if let Some(manager) = self.session_manager.as_ref() {
                        manager.exit();
                    }
                }
                self.close_notice();
            }
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
        _layout: u32,
    ) {
    }
}

impl ProvidesRegistryState for Olshell {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Olshell);
delegate_output!(Olshell);
delegate_shm!(Olshell);
delegate_seat!(Olshell);
delegate_pointer!(Olshell);
delegate_keyboard!(Olshell);
delegate_layer!(Olshell);
delegate_registry!(Olshell);
delegate_subcompositor!(Olshell);

// Neither wlr-foreign-toplevel-management nor openlook-workspaces are
// sctk-integrated protocols, so these are plain wayland-client Dispatch
// impls rather than sctk delegate macros. This is read-only: it enumerates
// and tracks toplevels/workspaces, but doesn't yet send any of the control
// requests (activate/close/maximize/switch_to) -- that's real taskbar UI
// work, not wiring.
impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Olshell {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.insert(
                toplevel.id(),
                ToplevelInfo { handle: Some(toplevel), ..Default::default() },
            );
        }
    }

    wayland_client::event_created_child!(Olshell, ZwlrForeignToplevelManagerV1, [
        0 => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Olshell {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;

        match event {
            Event::Title { title } => {
                state.toplevels.entry(proxy.id()).or_default().title = title;
            }
            Event::AppId { app_id } => {
                state.toplevels.entry(proxy.id()).or_default().app_id = app_id;
            }
            Event::State { state: state_bytes } => {
                let states = state_bytes
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
                    .collect();
                state.toplevels.entry(proxy.id()).or_default().states = states;
            }
            Event::Done => {
                if let Some(info) = state.toplevels.get(&proxy.id()) {
                    log::info!(
                        "toplevel: title={:?} app_id={:?} states={:?}",
                        info.title, info.app_id, info.states
                    );
                }
                state.ensure_decoration(qh, &proxy.id());
                state.draw_decoration(&proxy.id());
                // Covers a minimized/unminimized state change (the icon
                // tray's entire reason for existing) as well as a title
                // or app_id change while minimized (the icon's own
                // glyph/label).
                if let Some(output) = state.toplevels.get(&proxy.id()).and_then(|info| info.output.clone()) {
                    state.redraw_background_for_output(qh, &output);
                }
            }
            Event::Closed => {
                if state.window_menu.as_ref().is_some_and(|wm| wm.toplevel_id == proxy.id()) {
                    state.close_window_menu();
                }
                if state.icon_menu.as_ref().is_some_and(|im| im.toplevel_id == proxy.id()) {
                    state.close_icon_menu();
                }
                if let Some(mut info) = state.toplevels.remove(&proxy.id()) {
                    if let Some(dec) = info.decoration.take() {
                        dec.object.destroy();
                        dec.surface.destroy();
                    }
                    // A minimized toplevel closing outright (not just
                    // being restored) needs its icon gone too.
                    if let Some(output) = info.output.take() {
                        state.redraw_background_for_output(qh, &output);
                    }
                }
                proxy.destroy();
            }
            Event::OutputEnter { .. } | Event::OutputLeave { .. } | Event::Parent { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<ZopenlookDecorationManagerV1, ()> for Olshell {
    fn event(
        _state: &mut Self,
        _proxy: &ZopenlookDecorationManagerV1,
        _event: zopenlook_decoration_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No events defined by this interface.
    }
}

impl Dispatch<ZopenlookSessionManagerV1, ()> for Olshell {
    fn event(
        _state: &mut Self,
        _proxy: &ZopenlookSessionManagerV1,
        _event: zopenlook_session_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No events defined by this interface.
    }
}

impl Dispatch<ZopenlookDecorationV1, ObjectId> for Olshell {
    fn event(
        state: &mut Self,
        proxy: &ZopenlookDecorationV1,
        event: zopenlook_decoration_v1::Event,
        toplevel_id: &ObjectId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zopenlook_decoration_v1::Event;

        match event {
            Event::Configure { serial, width, height, toplevel_height } => {
                log::info!("decoration: configure {toplevel_id:?} {width}x{height} (toplevel {toplevel_height})");
                proxy.ack_configure(serial);
                if let Some(dec) =
                    state.toplevels.get_mut(toplevel_id).and_then(|info| info.decoration.as_mut())
                {
                    dec.width = width;
                    dec.height = height;
                    dec.toplevel_height = toplevel_height;
                }
                state.draw_decoration(toplevel_id);
            }
            Event::Closed => {
                if state.window_menu.as_ref().is_some_and(|wm| &wm.toplevel_id == toplevel_id) {
                    state.close_window_menu();
                }
                if let Some(dec) =
                    state.toplevels.get_mut(toplevel_id).and_then(|info| info.decoration.take())
                {
                    for region in ResizeRegion::ALL {
                        let handle = dec.resize_handle(region);
                        handle.subsurface.destroy();
                        handle.surface.destroy();
                    }
                    dec.object.destroy();
                    dec.surface.destroy();
                }
            }
            Event::StickyChanged { sticky } => {
                if let Some(dec) =
                    state.toplevels.get_mut(toplevel_id).and_then(|info| info.decoration.as_mut())
                {
                    dec.sticky = sticky != 0;
                }
                state.draw_decoration(toplevel_id);
            }
        }
    }
}

impl Dispatch<ZopenlookWorkspacesManagerV1, ()> for Olshell {
    fn event(
        state: &mut Self,
        _proxy: &ZopenlookWorkspacesManagerV1,
        event: zopenlook_workspaces_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zopenlook_workspaces_manager_v1::Event;

        match event {
            // WorkspaceCount/ActiveChanged moved to
            // ZopenlookWorkspacesOutputV1 now that workspaces are
            // per-output -- see that Dispatch impl below.
            Event::ToplevelWorkspace { toplevel, output, index } => {
                let title = state.toplevels.get(&toplevel.id()).map(|i| i.title.clone());
                log::info!("workspaces: toplevel {title:?} -> output {output:?} workspace {index}");
                if let Some(info) = state.toplevels.get_mut(&toplevel.id()) {
                    if info.output.as_ref() != Some(&output) {
                        // A dragged icon position is only meaningful on the
                        // output it was set on -- falls back to the default
                        // packed layout on whichever output it lands on now.
                        info.icon_position = None;
                    }
                    info.output = Some(output);
                    info.workspace_index = Some(index);
                }
            }
        }
    }
}

impl Dispatch<ZopenlookWorkspacesOutputV1, ()> for Olshell {
    fn event(
        state: &mut Self,
        proxy: &ZopenlookWorkspacesOutputV1,
        event: zopenlook_workspaces_output_v1::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zopenlook_workspaces_output_v1::Event;

        let Some(index) = state.panels.iter().position(|p| p.workspaces == *proxy) else {
            return;
        };

        match event {
            Event::WorkspaceCount { count } => {
                state.panels[index].workspace_count = count;
                log::info!("workspaces: panel {index}: {count} available");
                state.draw_panel(index);
            }
            Event::ActiveChanged { index: active } => {
                state.panels[index].active_workspace = active;
                log::info!("workspaces: panel {index}: active = {active}");
                state.draw_panel(index);
                // A non-sticky minimized window's icon only shows on its
                // own workspace, same as the window itself would --
                // switching workspaces can change which icons belong in
                // this output's tray.
                let output = state.panels[index].output.clone();
                state.redraw_background_for_output(qh, &output);
            }
        }
    }
}
