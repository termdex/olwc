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

const MENU_FONT_SIZE: f32 = 18.0;
const MENU_ROW_HEIGHT: i32 = 26;
const MENU_H_PADDING: i32 = 12;
const MENU_BG_COLOR: (u8, u8, u8) = (0xD8, 0xD8, 0xD0);
const MENU_HOVER_COLOR: (u8, u8, u8) = (0x8A, 0x9E, 0xB0);
const MENU_TITLE_COLOR: (u8, u8, u8) = (0x40, 0x40, 0x38);
const MENU_TEXT_COLOR: (u8, u8, u8) = (0x18, 0x18, 0x18);

const PUSHPIN_SIZE: i32 = 10;
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
    WindowMenuItem { label: "Close", action: WindowMenuAction::Minimize, disabled: false },
    WindowMenuItem { label: "Full Size", action: WindowMenuAction::ToggleMaximize, disabled: false },
    WindowMenuItem { label: "Move", action: WindowMenuAction::Move, disabled: false },
    WindowMenuItem { label: "Resize", action: WindowMenuAction::Resize, disabled: false },
    WindowMenuItem { label: "Properties", action: WindowMenuAction::Unimplemented, disabled: true },
    WindowMenuItem { label: "Back", action: WindowMenuAction::Lower, disabled: false },
    WindowMenuItem { label: "Stick", action: WindowMenuAction::ToggleSticky, disabled: false },
    WindowMenuItem { label: "Move to Workspace", action: WindowMenuAction::MoveToWorkspace, disabled: false },
    WindowMenuItem { label: "Quit", action: WindowMenuAction::Quit, disabled: false },
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
}

/// The header (title bar) chrome olshell draws for one toplevel it doesn't
/// own, via openlook-decoration. Rendering only; the window-menu popup its
/// button is meant to open is follow-up work (see pointer_frame below).
struct Decoration {
    surface: wl_surface::WlSurface,
    object: ZopenlookDecorationV1,
    width: u32,
    height: u32,
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
        let x0 = x1 - PUSHPIN_SIZE;
        let y0 = (self.height as i32 - PUSHPIN_SIZE) / 2;
        (x0, y0, x1, y0 + PUSHPIN_SIZE)
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
    items: Vec<MenuNode>,
    title: Option<String>,
    width: u32,
    height: u32,
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
        let x1 = x0 + PUSHPIN_SIZE;
        let y0 = (MENU_ROW_HEIGHT - PUSHPIN_SIZE) / 2;
        (x0, y0, x1, y0 + PUSHPIN_SIZE)
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
        toplevels: std::collections::HashMap::new(),
        font,
        menu,
        pointer: None,
        keyboard: None,
        keyboard_focus: None,
        popups: Vec::new(),
        window_menu: None,
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
    /// Index (into the per-draw list draw_background/icon_at both
    /// recompute -- see minimized_toplevels_for_output) of the icon the
    /// pointer is currently over, if any.
    hovered_icon: Option<usize>,
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
    toplevels: std::collections::HashMap<ObjectId, ToplevelInfo>,
    font: fontdue::Font,
    menu: Menu,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    // The surface a keyboard enter most recently reported, without a
    // matching leave yet -- the only thing that ever requests keyboard
    // focus is a root-menu popup's Exclusive layer surface (see
    // MenuPopup's doc comment), so this is how Escape tells *which*
    // popup to close now that more than one can be open at once.
    keyboard_focus: Option<wl_surface::WlSurface>,
    popups: Vec<MenuPopup>,
    window_menu: Option<WindowMenu>,
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
        let stride = width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
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
            fill_rect(canvas, width, height, x0, 2, x1.min(width), height - 2, fill);

            let label = (i + 1).to_string();
            let label_width: i32 =
                label.chars().map(|c| self.font.metrics(c, PANEL_FONT_SIZE).advance_width.round() as i32).sum();
            let label_x = x0 + ((x1 - x0) - label_width) / 2;
            draw_text_row_centered(canvas, width, 0, height, label_x, &label, &self.font, PANEL_FONT_SIZE, text_color);
        }

        let panel = &self.panels[panel_index];
        let wl_surface = panel.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
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
        let stride = width * 4;

        // No panel (openlook-workspaces unavailable) means no
        // active-workspace to gate the tray on -- degrade the same way
        // the panel itself does, rather than showing every minimized
        // window on every workspace at once.
        let active_workspace = self.panels.iter().find(|p| p.output == bg.output).map(|p| p.active_workspace);
        let icon_ids = active_workspace.map(|w| self.minimized_toplevels_for_output(&bg.output, w));
        let hovered_icon = bg.hovered_icon;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .expect("failed to create buffer");

        let (r, g, b) = BACKGROUND_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }

        if let Some(icon_ids) = &icon_ids {
            for (i, id) in icon_ids.iter().enumerate() {
                let (x0, y0, x1, y1) = icon_rect(i, height);
                if x1 > width {
                    break;
                }
                let fill = if hovered_icon == Some(i) { MENU_HOVER_COLOR } else { ICON_BG_COLOR };
                fill_rect(canvas, width, height, x0, y0, x1, y1, fill);
                fill_rect(canvas, width, height, x0, y0, x1, y0 + 1, ICON_BORDER_COLOR);
                fill_rect(canvas, width, height, x0, y1 - 1, x1, y1, ICON_BORDER_COLOR);
                fill_rect(canvas, width, height, x0, y0, x0 + 1, y1, ICON_BORDER_COLOR);
                fill_rect(canvas, width, height, x1 - 1, y0, x1, y1, ICON_BORDER_COLOR);

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
                    canvas, width, y0, y1 - y0, x0 + (x1 - x0 - glyph_width) / 2,
                    &glyph.to_string(), &self.font, ICON_GLYPH_FONT_SIZE, ICON_TEXT_COLOR,
                );

                let label = if info.title.is_empty() { &info.app_id } else { &info.title };
                let label_width: i32 =
                    label.chars().map(|c| self.font.metrics(c, ICON_FONT_SIZE).advance_width.round() as i32).sum();
                let label_x = (x0 + x1) / 2 - label_width / 2;
                draw_text_row_centered(
                    canvas, width, y1, ICON_LABEL_HEIGHT, label_x.max(0),
                    label, &self.font, ICON_FONT_SIZE, ICON_TEXT_COLOR,
                );
            }
        }

        let bg = &self.backgrounds[index];
        let wl_surface = bg.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        bg.layer.commit();
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
        let stride = width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
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
        paint_row(canvas, width, DECORATION_BORDER_WIDTH, bevel_top);
        if height > 1 {
            paint_row(canvas, width, height - 1, bevel_bottom);
        }

        // The plain black frame's top stretch, skipping the two top
        // corners -- their own bracket subsurfaces sit right on top of
        // this and are partly transparent (the bracket's notch), so
        // anything drawn underneath them must stop exactly at their
        // edges, not bleed into the gap. See border_side_rect for the
        // matching left/right stretches.
        fill_rect(canvas, width, height, CORNER_HANDLE_SIZE, 0, width - CORNER_HANDLE_SIZE, DECORATION_BORDER_WIDTH, DECORATION_BORDER_COLOR);

        let (bx0, by0, bx1, by1) = dec.button_rect();
        let button_color = if dec.button_hovered { DECORATION_BUTTON_HOVER_COLOR } else { header_bg };
        fill_rect(canvas, width, height, bx0, by0, bx1, by1, button_color);
        draw_chevron(canvas, width, height, bx0, by0, bx1, by1, DECORATION_TEXT_COLOR);

        if !info.title.is_empty() {
            draw_text_row_centered(
                canvas, width, 0, height, bx1 + DECORATION_BUTTON_MARGIN,
                &info.title, &self.font, DECORATION_FONT_SIZE, DECORATION_TEXT_COLOR,
            );
        }

        if dec.sticky {
            let (px0, py0, px1, py1) = dec.sticky_pushpin_rect();
            draw_pushpin(canvas, width, height, px0, py0, px1, py1, true, PUSHPIN_PINNED_COLOR);
        }

        let wl_surface = &dec.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        wl_surface.commit();
        log::info!("decoration: drew {toplevel_id:?} at {width}x{height}");

        // Top corners only need the header's own width, already known;
        // bottom corners and the footer also need toplevel_height, from
        // the toplevel's own first commit, which can lag behind the
        // header's.
        let (tl_x, tl_y) = dec.corner_handle_position(ResizeRegion::TopLeft);
        dec.top_left.subsurface.set_position(tl_x, tl_y);
        draw_corner_handle(&mut self.pool, &dec.top_left.surface, dec.top_left.hovered, ResizeRegion::TopLeft, header_bg);

        let (tr_x, tr_y) = dec.corner_handle_position(ResizeRegion::TopRight);
        dec.top_right.subsurface.set_position(tr_x, tr_y);
        draw_corner_handle(&mut self.pool, &dec.top_right.surface, dec.top_right.hovered, ResizeRegion::TopRight, header_bg);

        if dec.toplevel_height > 0 {
            let (bl_x, bl_y) = dec.corner_handle_position(ResizeRegion::BottomLeft);
            dec.bottom_left.subsurface.set_position(bl_x, bl_y);
            draw_corner_handle(&mut self.pool, &dec.bottom_left.surface, dec.bottom_left.hovered, ResizeRegion::BottomLeft, header_bg);

            let (br_x, br_y) = dec.corner_handle_position(ResizeRegion::BottomRight);
            dec.bottom_right.subsurface.set_position(br_x, br_y);
            draw_corner_handle(&mut self.pool, &dec.bottom_right.surface, dec.bottom_right.hovered, ResizeRegion::BottomRight, header_bg);

            let (f_x, f_y, f_width) = dec.footer_rect();
            if f_width > 0 {
                dec.footer.subsurface.set_position(f_x, f_y);
                draw_footer(&mut self.pool, &dec.footer.surface, f_width as u32, dec.footer.hovered);
            }

            let (lb_x, lb_y0, lb_y1) = dec.border_side_rect(false);
            if lb_y1 > lb_y0 {
                dec.left_border.subsurface.set_position(lb_x, lb_y0);
                draw_border_strip(&mut self.pool, &dec.left_border.surface, (lb_y1 - lb_y0) as u32);
            }

            let (rb_x, rb_y0, rb_y1) = dec.border_side_rect(true);
            if rb_y1 > rb_y0 {
                dec.right_border.subsurface.set_position(rb_x, rb_y0);
                draw_border_strip(&mut self.pool, &dec.right_border.surface, (rb_y1 - rb_y0) as u32);
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
        // The extra MENU_H_PADDING + SUBMENU_ARROW_SIZE is Move to
        // Workspace's submenu-indicator arrow (see draw_window_menu) --
        // reserved on every row, not just that one, so the popup doesn't
        // need a different width depending on which item has it.
        let width = (max_width + MENU_H_PADDING * 3 + SUBMENU_ARROW_SIZE).max(80) as u32;
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
        let stride = width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
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
                let (hr, hg, hb) = MENU_HOVER_COLOR;
                for y in row_y0..(row_y0 + MENU_ROW_HEIGHT).min(height) {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;
                        canvas[idx] = hb;
                        canvas[idx + 1] = hg;
                        canvas[idx + 2] = hr;
                        canvas[idx + 3] = 0xFF;
                    }
                }
            }
            let color = if disabled { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR };
            let label = if matches!(item.action, WindowMenuAction::ToggleSticky) && sticky {
                "Unstick"
            } else {
                item.label
            };
            draw_text_row_centered(
                canvas, width, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                label, &self.font, MENU_FONT_SIZE, color,
            );
            if matches!(item.action, WindowMenuAction::MoveToWorkspace) {
                let ax1 = width - MENU_H_PADDING;
                let ax0 = ax1 - SUBMENU_ARROW_SIZE;
                let ay0 = row_y0 + (MENU_ROW_HEIGHT - SUBMENU_ARROW_SIZE) / 2;
                let ay1 = ay0 + SUBMENU_ARROW_SIZE;
                draw_submenu_arrow(canvas, width, height, ax0, ay0, ax1, ay1, color);
            }
        }

        let wl_surface = &wm.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
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
        let width = sm.width as i32;
        let height = sm.height as i32;
        let stride = width * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
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
                let (hr, hg, hb) = MENU_HOVER_COLOR;
                for y in row_y0..(row_y0 + MENU_ROW_HEIGHT).min(height) {
                    for x in 0..width {
                        let idx = ((y * width + x) * 4) as usize;
                        canvas[idx] = hb;
                        canvas[idx + 1] = hg;
                        canvas[idx + 2] = hr;
                        canvas[idx + 3] = 0xFF;
                    }
                }
            }
            let (label, color) = match row {
                WorkspaceSubmenuRow::OutputHeader { name } => (name.clone(), MENU_TITLE_COLOR),
                WorkspaceSubmenuRow::Workspace { index, current, .. } => (
                    format!("Workspace {}", index + 1),
                    if *current { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR },
                ),
            };
            draw_text_row_centered(
                canvas, width, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                &label, &self.font, MENU_FONT_SIZE, color,
            );
        }

        let wl_surface = &sm.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
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
        let mut max_width = title.as_deref().map_or(0, label_width) + PUSHPIN_SIZE + MENU_H_PADDING;
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

        self.popups.push(MenuPopup { layer, items, title, width, height, hovered: None, pinned: false });
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
    fn redraw_background_for_output(&mut self, output: &wl_output::WlOutput) {
        if let Some(index) = self.backgrounds.iter().position(|b| &b.output == output) {
            self.draw_background(index);
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

    /// Icon tray index at `(x, y)` (background-local) for the output
    /// `background_index` names, if any.
    fn icon_at(&self, background_index: usize, x: f64, y: f64) -> Option<usize> {
        let bg = &self.backgrounds[background_index];
        let panel = self.panels.iter().find(|p| p.output == bg.output)?;
        let count = self.minimized_toplevels_for_output(&bg.output, panel.active_workspace).len();
        (0..count).find(|&i| {
            let (x0, y0, x1, y1) = icon_rect(i, bg.height as i32);
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
    hovered: bool,
    region: ResizeRegion,
    fill_color: (u8, u8, u8),
) {
    let size = CORNER_HANDLE_SIZE;
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
    surface.damage_buffer(0, 0, size, size);
    surface.commit();
}

/// Draws the footer strip: a thin horizontal bar, vertically centered in
/// an otherwise fully transparent buffer -- same reasoning as
/// draw_corner_handle (floats over the toplevel's own content, and needs
/// RGB zeroed along with alpha for the same premultiplication reason).
fn draw_footer(pool: &mut SlotPool, surface: &wl_surface::WlSurface, width: u32, hovered: bool) {
    let width = width as i32;
    let height = CORNER_HANDLE_SIZE;
    let stride = width * 4;
    let (buffer, canvas) =
        pool.create_buffer(width, height, stride, wl_shm::Format::Argb8888).expect("failed to create buffer");
    canvas.fill(0);
    let color = if hovered { DECORATION_BUTTON_HOVER_COLOR } else { DECORATION_TEXT_COLOR };
    let bar_y0 = height / 2 - 1;
    let bar_y1 = height / 2 + 1;
    fill_rect(canvas, width, height, 0, bar_y0, width, bar_y1, color);
    // The plain black frame's bottom stretch, at the toplevel's actual
    // bottom edge -- same border the left/right strips and the header's
    // own top stretch draw, completing the frame around all four sides.
    fill_rect(canvas, width, height, 0, height - DECORATION_BORDER_WIDTH, width, height, DECORATION_BORDER_COLOR);
    buffer.attach_to(surface).expect("failed to attach buffer");
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
}

/// Draws a solid black border strip -- see Decoration::border_side_rect().
/// Unlike the corner handles and footer, every pixel here is opaque, so
/// there's no transparency/premultiplication gotcha to work around.
fn draw_border_strip(pool: &mut SlotPool, surface: &wl_surface::WlSurface, height: u32) {
    let width = DECORATION_BORDER_WIDTH;
    let height = height as i32;
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
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
}

fn draw_popup(pool: &mut SlotPool, font: &fontdue::Font, popup: &MenuPopup) {
    let width = popup.width as i32;
    let height = popup.height as i32;
    let stride = width * 4;

    let (buffer, canvas) = pool
        .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
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
        draw_text_row_centered(
            canvas, width, 0, MENU_ROW_HEIGHT, px1 + MENU_H_PADDING,
            title, font, MENU_FONT_SIZE, MENU_TITLE_COLOR,
        );
    }
    let pushpin_color = if popup.pinned { PUSHPIN_PINNED_COLOR } else { PUSHPIN_UNPINNED_COLOR };
    draw_pushpin(canvas, width, height, px0, py0, px1, py1, popup.pinned, pushpin_color);
    let row = popup.header_rows();

    for (i, item) in popup.items.iter().enumerate() {
        let row_y0 = (row + i as i32) * MENU_ROW_HEIGHT;
        if popup.hovered == Some(i) {
            let (hr, hg, hb) = MENU_HOVER_COLOR;
            for y in row_y0..(row_y0 + MENU_ROW_HEIGHT).min(height) {
                for x in 0..width {
                    let idx = ((y * width + x) * 4) as usize;
                    canvas[idx] = hb;
                    canvas[idx + 1] = hg;
                    canvas[idx + 2] = hr;
                    canvas[idx + 3] = 0xFF;
                }
            }
        }
        draw_text_row_centered(
            canvas, width, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
            item.label(), font, MENU_FONT_SIZE, MENU_TEXT_COLOR,
        );
    }

    let wl_surface = popup.layer.wl_surface();
    buffer.attach_to(wl_surface).expect("failed to attach buffer");
    wl_surface.damage_buffer(0, 0, width, height);
    popup.layer.commit();
}

/// Draws `text` with its baseline at `baseline_y`, rather than centered in
/// the whole canvas -- e.g. for centering within a single menu row.
fn draw_text_row_centered(
    canvas: &mut [u8],
    canvas_width: i32,
    row_y0: i32,
    row_height: i32,
    start_x: i32,
    text: &str,
    font: &fontdue::Font,
    size: f32,
    color: (u8, u8, u8),
) -> i32 {
    let baseline_y = row_y0 + row_height / 2 + (size as i32) / 3;
    draw_text_at(canvas, canvas_width, row_y0 + row_height, start_x, baseline_y, text, font, size, color)
}

/// Draws the pushpin glyph within box (x0,y0)-(x1,y1): a filled circle when
/// pinned (the pin pushed in/engaged), or just its outline when not.
#[allow(clippy::too_many_arguments)]
fn draw_pushpin(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    pinned: bool,
    color: (u8, u8, u8),
) {
    let (r, g, b) = color;
    let radius = (x1 - x0).min(y1 - y0) / 2;
    let cx = (x0 + x1) / 2;
    let cy = (y0 + y1) / 2;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let dist2 = dx * dx + dy * dy;
            let inside = dist2 <= radius * radius;
            let on_ring = inside && (pinned || dist2 >= (radius - 2) * (radius - 2));
            if !on_ring {
                continue;
            }
            let px = cx + dx;
            let py = cy + dy;
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

/// Fills one full-width row of `canvas` with an opaque color -- used for the
/// light/dark bevel edges along a decoration header's top and bottom.
fn paint_row(canvas: &mut [u8], canvas_width: i32, y: i32, color: (u8, u8, u8)) {
    let (r, g, b) = color;
    for x in 0..canvas_width {
        let idx = ((y * canvas_width + x) * 4) as usize;
        canvas[idx] = b;
        canvas[idx + 1] = g;
        canvas[idx + 2] = r;
        canvas[idx + 3] = 0xFF;
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

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: (u8, u8, u8),
) {
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

/// Draws a small downward-pointing chevron (the window-menu button glyph)
/// filling the given box. Drawn geometrically rather than as a font glyph,
/// same reasoning as draw_pushpin -- no dependency on a specific glyph
/// being present in the embedded font. Exact proportions are a placeholder;
/// docs/OPENLOOK-REFERENCE.md notes the real glyph shape still needs asset
/// work.
#[allow(clippy::too_many_arguments)]
fn draw_chevron(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: (u8, u8, u8),
) {
    let (r, g, b) = color;
    let w = x1 - x0;
    let h = y1 - y0;
    let inset = (w.min(h) / 4).max(1);
    let top = y0 + inset;
    let bottom = y1 - inset;
    let mid_x = (x0 + x1) / 2;
    for y in top..bottom {
        if bottom == top {
            break;
        }
        // Narrows linearly from the full inset width at `top` down to a
        // point at `bottom`, forming a downward-pointing "v".
        let t = 1.0 - (y - top) as f64 / (bottom - top) as f64;
        let half = ((mid_x - x0 - inset) as f64 * t) as i32;
        for x in (mid_x - half)..=(mid_x + half) {
            if x < 0 || y < 0 || x >= canvas_width || y >= canvas_height {
                continue;
            }
            let idx = ((y * canvas_width + x) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
}

/// Draws a small rightward-pointing wedge -- the window menu's indicator
/// that an item opens a submenu rather than acting immediately. Same
/// placeholder-geometry reasoning as draw_chevron (which this mirrors,
/// x/y swapped): an approximation, not an asset-accurate glyph.
#[allow(clippy::too_many_arguments)]
fn draw_submenu_arrow(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: (u8, u8, u8),
) {
    let (r, g, b) = color;
    let w = x1 - x0;
    let h = y1 - y0;
    let inset = (w.min(h) / 4).max(1);
    let left = x0 + inset;
    let right = x1 - inset;
    let mid_y = (y0 + y1) / 2;
    for x in left..right {
        if right == left {
            break;
        }
        // Narrows linearly from the full inset height at `left` down to a
        // point at `right`, forming a rightward-pointing "wedge".
        let t = 1.0 - (x - left) as f64 / (right - left) as f64;
        let half = ((mid_y - y0 - inset) as f64 * t) as i32;
        for y in (mid_y - half)..=(mid_y + half) {
            if x < 0 || y < 0 || x >= canvas_width || y >= canvas_height {
                continue;
            }
            let idx = ((y * canvas_width + x) * 4) as usize;
            canvas[idx] = b;
            canvas[idx + 1] = g;
            canvas[idx + 2] = r;
            canvas[idx + 3] = 0xFF;
        }
    }
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
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
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
            hovered_icon: None,
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
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
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
            self.draw_background(index);
        } else if let Some(index) = self.popup_at(layer.wl_surface()) {
            let popup = &mut self.popups[index];
            if configure.new_size.0 > 0 {
                popup.width = configure.new_size.0;
            }
            if configure.new_size.1 > 0 {
                popup.height = configure.new_size.1;
            }
            draw_popup(&mut self.pool, &self.font, popup);
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
            // Captured before the "click elsewhere closes it" step below
            // clears it, so the button-click handler can tell a second
            // click on the button that opened this exact menu (which
            // should toggle it closed) apart from a click that should
            // open one (a different toplevel's button, or this one after
            // the menu was already closed some other way).
            let window_menu_toplevel = self.window_menu.as_ref().map(|wm| wm.toplevel_id.clone());

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
            }

            match event.kind {
                PointerEventKind::Press { button, .. } if background_index.is_some() && button == BTN_RIGHT => {
                    let output = self.backgrounds[background_index.unwrap()].output.clone();
                    self.open_menu(qh, &output, event.position.0, event.position.1);
                }
                PointerEventKind::Motion { .. } if background_index.is_some() => {
                    let i = background_index.unwrap();
                    let hovered = self.icon_at(i, event.position.0, event.position.1);
                    if self.backgrounds[i].hovered_icon != hovered {
                        self.backgrounds[i].hovered_icon = hovered;
                        self.draw_background(i);
                    }
                }
                PointerEventKind::Leave { .. } if background_index.is_some() => {
                    let i = background_index.unwrap();
                    if self.backgrounds[i].hovered_icon.take().is_some() {
                        self.draw_background(i);
                    }
                }
                // SELECT (left-click) an icon to restore it -- unset_minimized
                // alone would bring the window back but leave focus wherever
                // it already was (possibly nowhere), so activate it too, the
                // same pairing clicking a taskbar entry does elsewhere; no
                // OPEN LOOK precedent for a "restore" gesture specifically
                // (a real icon would have its own menu with an Open item),
                // but a single click is the simplest thing that could work,
                // consistent with the workspace strip's own SELECT-click.
                PointerEventKind::Press { button, .. } if background_index.is_some() && button == BTN_LEFT => {
                    let i = background_index.unwrap();
                    if let Some(icon_index) = self.icon_at(i, event.position.0, event.position.1) {
                        let output = self.backgrounds[i].output.clone();
                        let active_workspace =
                            self.panels.iter().find(|p| p.output == output).map(|p| p.active_workspace);
                        let handle = active_workspace.and_then(|w| {
                            let ids = self.minimized_toplevels_for_output(&output, w);
                            let id = ids.get(icon_index)?.clone();
                            self.toplevels.get(&id)?.handle.clone()
                        });
                        if let Some(handle) = handle {
                            handle.unset_minimized();
                            if let Some(seat) = self.seat_state.seats().next() {
                                handle.activate(&seat);
                            }
                        }
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
        // MenuPopup's doc comment) and the window menu (openlook-
        // decoration's grab_keyboard, see WindowMenu's doc comment) --
        // keyboard_focus is how Escape tells which one, if either,
        // olcore actually handed focus to, so only that one closes.
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
                    state.redraw_background_for_output(&output);
                }
            }
            Event::Closed => {
                if state.window_menu.as_ref().is_some_and(|wm| wm.toplevel_id == proxy.id()) {
                    state.close_window_menu();
                }
                if let Some(mut info) = state.toplevels.remove(&proxy.id()) {
                    if let Some(dec) = info.decoration.take() {
                        dec.object.destroy();
                        dec.surface.destroy();
                    }
                    // A minimized toplevel closing outright (not just
                    // being restored) needs its icon gone too.
                    if let Some(output) = info.output.take() {
                        state.redraw_background_for_output(&output);
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
        _qh: &QueueHandle<Self>,
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
                state.redraw_background_for_output(&output);
            }
        }
    }
}
