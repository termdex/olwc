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
// to zwlr_foreign_toplevel_handle_v1 resolve via the wlr crate's interfaces.
mod openlook_workspaces {
    pub mod v1 {
        pub mod client {
            use wayland_client;
            use wayland_protocols_wlr::foreign_toplevel::v1::client::*;

            pub mod __interfaces {
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

use openlook_workspaces::v1::client::zopenlook_workspaces_manager_v1::{
    self, ZopenlookWorkspacesManagerV1,
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
    Close,
    ToggleMaximize,
    Move,
    Resize,
    Lower,
    Quit,
    ToggleSticky,
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
// handles trigger, Back reuses the same "lower to bottom" olcore exposes,
// and Quit closes every toplevel sharing this one's client connection
// (see the decoration protocol's quit request) rather than just this
// window, the same Close-window/Quit-application distinction most desktop
// environments still draw. Properties is disabled to match the reference
// screenshot (shown grayed out there too).
const WINDOW_MENU_ITEMS: &[WindowMenuItem] = &[
    WindowMenuItem { label: "Close", action: WindowMenuAction::Close, disabled: false },
    WindowMenuItem { label: "Full Size", action: WindowMenuAction::ToggleMaximize, disabled: false },
    WindowMenuItem { label: "Move", action: WindowMenuAction::Move, disabled: false },
    WindowMenuItem { label: "Resize", action: WindowMenuAction::Resize, disabled: false },
    WindowMenuItem { label: "Properties", action: WindowMenuAction::Unimplemented, disabled: true },
    WindowMenuItem { label: "Back", action: WindowMenuAction::Lower, disabled: false },
    WindowMenuItem { label: "Stick", action: WindowMenuAction::ToggleSticky, disabled: false },
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
/// both are olshell-owned, so this needs no protocol extension, unlike the
/// decoration itself -- positioned just below the header, extending down
/// over the window's content like the reference screenshots show. No
/// pushpin (the reference doesn't show one on window menus, only on root
/// menu-style pinnable menus) and no keyboard focus yet, so Escape doesn't
/// close it -- click elsewhere does. Follow-up work, same as Escape support
/// was for the root menu.
struct WindowMenu {
    toplevel_id: ObjectId,
    subsurface: wl_subsurface::WlSubsurface,
    surface: wl_surface::WlSurface,
    width: u32,
    height: u32,
    hovered: Option<usize>,
}

impl WindowMenu {
    fn item_at(&self, y: f64) -> Option<usize> {
        let row = (y / MENU_ROW_HEIGHT as f64) as usize;
        (row < WINDOW_MENU_ITEMS.len()).then_some(row)
    }
}

/// A transient root-menu popup: press MENU on the background to open one,
/// drag to highlight an item, release over it to run the item's command
/// and dismiss (release elsewhere just dismisses). Submenu entries are
/// rendered but not yet interactive -- opening a nested popup on hover is
/// follow-up work. No pushpin/persist gesture yet either.
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

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Top,
        Some("olshell-panel"),
        None,
    );
    layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, PANEL_HEIGHT);
    layer.set_exclusive_zone(PANEL_HEIGHT as i32);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();

    // The desktop background doubles as the OPEN LOOK "root window": MENU
    // (right-button) clicks on it open the root menu. Non-exclusive so it
    // doesn't compete with the panel for space.
    let bg_surface = compositor.create_surface(&qh);
    let background = layer_shell.create_layer_surface(
        &qh,
        bg_surface,
        Layer::Background,
        Some("olshell-background"),
        None,
    );
    background.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    background.set_size(0, 0);
    background.set_keyboard_interactivity(KeyboardInteractivity::None);
    background.commit();

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
        layer,
        width: 0,
        height: PANEL_HEIGHT,
        background,
        bg_width: 0,
        bg_height: 0,
        exit: false,
        foreign_toplevel_manager,
        workspaces_manager,
        decoration_manager,
        toplevels: std::collections::HashMap::new(),
        workspace_count: 0,
        active_workspace: 0,
        font,
        menu,
        pointer: None,
        keyboard: None,
        popup: None,
        window_menu: None,
        hovered_workspace: None,
    };

    while !state.exit {
        event_queue.blocking_dispatch(&mut state).expect("event dispatch failed");
    }
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
    layer: LayerSurface,
    width: u32,
    height: u32,
    background: LayerSurface,
    bg_width: u32,
    bg_height: u32,
    exit: bool,
    // Kept only to hold the binding alive; no requests are sent on this
    // one, so nothing reads it beyond the Option check at startup.
    #[allow(dead_code)]
    foreign_toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,
    workspaces_manager: Option<ZopenlookWorkspacesManagerV1>,
    decoration_manager: Option<ZopenlookDecorationManagerV1>,
    toplevels: std::collections::HashMap<ObjectId, ToplevelInfo>,
    workspace_count: u32,
    active_workspace: u32,
    font: fontdue::Font,
    menu: Menu,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    popup: Option<MenuPopup>,
    window_menu: Option<WindowMenu>,
    hovered_workspace: Option<u32>,
}

impl Olshell {
    fn draw_panel(&mut self) {
        // Nothing to paint into yet -- the compositor hasn't sent our first
        // configure. A toplevel update arriving before then would otherwise
        // draw into a degenerate 1px-wide buffer for no reason.
        if self.width == 0 {
            return;
        }
        let width = self.width as i32;
        let height = self.height.max(1) as i32;
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

        for i in 0..self.workspace_count {
            let (x0, x1) = workspace_segment_x(i);
            if x0 >= width {
                break;
            }
            let active = i == self.active_workspace;
            let hovered = self.hovered_workspace == Some(i);
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

        let wl_surface = self.layer.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        self.layer.commit();
    }

    fn draw_background(&mut self) {
        if self.bg_width == 0 {
            return;
        }
        let width = self.bg_width as i32;
        let height = self.bg_height.max(1) as i32;
        let stride = width * 4;

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

        let wl_surface = self.background.wl_surface();
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        self.background.commit();
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
        let width = (max_width + MENU_H_PADDING * 2).max(80) as u32;
        let height = (WINDOW_MENU_ITEMS.len() as i32 * MENU_ROW_HEIGHT) as u32;

        let (subsurface, surface) = self.subcompositor.create_subsurface(dec_surface.clone(), qh);
        subsurface.set_position(0, DECORATION_HEIGHT as i32);
        // Desync so the menu's own commits apply immediately rather than
        // waiting on the header's next commit -- every other surface here
        // behaves that way too, and there's no reason this one shouldn't.
        subsurface.set_desync();

        self.window_menu =
            Some(WindowMenu { toplevel_id: toplevel_id.clone(), subsurface, surface, width, height, hovered: None });
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

        // Only Stick's label depends on current state -- olcore doesn't
        // expose per-item state generally, just this one via sticky on
        // Decoration (kept in sync by the sticky_changed event).
        let sticky = self
            .toplevels
            .get(&wm.toplevel_id)
            .and_then(|info| info.decoration.as_ref())
            .is_some_and(|dec| dec.sticky);

        for (i, item) in WINDOW_MENU_ITEMS.iter().enumerate() {
            let row_y0 = i as i32 * MENU_ROW_HEIGHT;
            if !item.disabled && wm.hovered == Some(i) {
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
            let color = if item.disabled { WINDOW_MENU_DISABLED_COLOR } else { MENU_TEXT_COLOR };
            let label = if matches!(item.action, WindowMenuAction::ToggleSticky) && sticky {
                "Unstick"
            } else {
                item.label
            };
            draw_text_row_centered(
                canvas, width, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                label, &self.font, MENU_FONT_SIZE, color,
            );
        }

        let wl_surface = &wm.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        wl_surface.commit();
    }

    /// Opens the root menu at `(x, y)` (surface-local coordinates on the
    /// background, which is fullscreen so those are effectively screen
    /// coordinates). Anchoring a layer surface to top-left with margins is
    /// the standard wlr-layer-shell trick for pixel-precise popup placement.
    fn open_menu(&mut self, qh: &QueueHandle<Self>, x: f64, y: f64) {
        self.close_menu();

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
            None,
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_margin(y as i32, 0, 0, x as i32);
        layer.set_size(width, height);
        // Exclusive so olcore grants it keyboard focus while mapped (see
        // layer_surface_map() there) -- that's what lets Escape reach us.
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();

        self.popup = Some(MenuPopup { layer, items, title, width, height, hovered: None, pinned: false });
    }

    fn close_menu(&mut self) {
        // Just drop it -- sctk's LayerSurface::Drop already destroys the
        // zwlr_layer_surface_v1 role object before the wl_surface, in the
        // order the protocol requires. Destroying the wl_surface ourselves
        // first (as this used to do) is a protocol violation: "surface was
        // destroyed before its role object".
        self.popup = None;
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

    /// Workspace index (0-based) the panel-local `x` falls on, if any.
    fn workspace_at(&self, x: f64) -> Option<u32> {
        let x = x as i32;
        (0..self.workspace_count).find(|&i| {
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

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {}
    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {}
}

impl LayerShellHandler for Olshell {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if layer.wl_surface() == self.layer.wl_surface()
            || layer.wl_surface() == self.background.wl_surface()
        {
            self.exit = true;
        } else if self.popup.as_ref().is_some_and(|p| layer.wl_surface() == p.layer.wl_surface()) {
            self.popup = None;
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
        if layer.wl_surface() == self.layer.wl_surface() {
            self.width = configure.new_size.0;
            self.height = if configure.new_size.1 > 0 {
                configure.new_size.1
            } else {
                PANEL_HEIGHT
            };
            self.draw_panel();
        } else if layer.wl_surface() == self.background.wl_surface() {
            self.bg_width = configure.new_size.0;
            self.bg_height = configure.new_size.1;
            self.draw_background();
        } else if let Some(popup) = self.popup.as_mut() {
            if layer.wl_surface() == popup.layer.wl_surface() {
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
            let on_background = event.surface == *self.background.wl_surface();
            let on_panel = event.surface == *self.layer.wl_surface();
            let on_popup = self
                .popup
                .as_ref()
                .is_some_and(|p| event.surface == *p.layer.wl_surface());
            let decoration_toplevel = self.decoration_toplevel_id(&event.surface);
            let resize_region = self.resize_region_at(&event.surface);
            let on_window_menu =
                self.window_menu.as_ref().is_some_and(|wm| event.surface == wm.surface);
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
            if let PointerEventKind::Press { .. } = event.kind {
                if self.window_menu.is_some() && !on_window_menu {
                    self.close_window_menu();
                }
            }

            match event.kind {
                PointerEventKind::Press { button, .. } if on_background && button == BTN_RIGHT => {
                    self.open_menu(qh, event.position.0, event.position.1);
                }
                PointerEventKind::Motion { .. } if on_panel => {
                    let hovered = self.workspace_at(event.position.0);
                    if self.hovered_workspace != hovered {
                        self.hovered_workspace = hovered;
                        self.draw_panel();
                    }
                }
                PointerEventKind::Leave { .. } if on_panel => {
                    if self.hovered_workspace.take().is_some() {
                        self.draw_panel();
                    }
                }
                PointerEventKind::Press { button, .. } if on_panel && button == BTN_LEFT => {
                    if let Some(index) = self.workspace_at(event.position.0) {
                        if let Some(manager) = self.workspaces_manager.as_ref() {
                            manager.switch_to(index);
                        }
                    }
                }
                // ADJUST (middle-click) a segment to move the focused
                // window there instead of switching to it -- borrowed
                // from modern multi-workspace desktops; no OPEN LOOK
                // precedent, but a fitting use for the ADJUST button
                // (extend/modify an existing selection) all the same.
                PointerEventKind::Press { button, .. } if on_panel && button == BTN_MIDDLE => {
                    if let Some(index) = self.workspace_at(event.position.0) {
                        if let (Some(manager), Some(handle)) =
                            (self.workspaces_manager.as_ref(), self.focused_toplevel_handle())
                        {
                            manager.assign_toplevel(&handle, index);
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
                    let changed = if let Some(wm) = self.window_menu.as_mut() {
                        let hovered = wm
                            .item_at(event.position.1)
                            .filter(|&i| !WINDOW_MENU_ITEMS[i].disabled);
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
                    if let Some((toplevel_id, index)) = selection {
                        let item = &WINDOW_MENU_ITEMS[index];
                        if !item.disabled {
                            match item.action {
                                WindowMenuAction::Close => {
                                    if let Some(handle) =
                                        self.toplevels.get(&toplevel_id).and_then(|i| i.handle.as_ref())
                                    {
                                        handle.close();
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
                                WindowMenuAction::Unimplemented => {
                                    log::info!("window menu: {} not yet implemented", item.label);
                                }
                            }
                        }
                    }
                    self.close_window_menu();
                }
                PointerEventKind::Motion { .. } if on_popup => {
                    let popup = self.popup.as_mut().unwrap();
                    let hovered = popup.item_at(event.position.1);
                    if popup.hovered != hovered {
                        popup.hovered = hovered;
                        draw_popup(&mut self.pool, &self.font, popup);
                    }
                }
                PointerEventKind::Release { button, .. } if button == BTN_RIGHT => {
                    let mut command_to_run = None;
                    let mut should_close = false;

                    if let Some(popup) = self.popup.as_mut() {
                        if on_popup && popup.is_on_pushpin(event.position.0, event.position.1) {
                            // Pinning a transient popup makes it persistent;
                            // clicking the pushpin again on an already-
                            // pinned popup is how you dismiss it, since
                            // there's no button-hold to release into once
                            // it's just sitting there open.
                            if popup.pinned {
                                should_close = true;
                            } else {
                                popup.pinned = true;
                                draw_popup(&mut self.pool, &self.font, popup);
                            }
                        } else if on_popup {
                            if let Some(index) = popup.item_at(event.position.1) {
                                match &popup.items[index] {
                                    MenuNode::Item { command, .. } => {
                                        command_to_run = Some(command.clone());
                                    }
                                    MenuNode::Submenu { .. } => {
                                        log::info!("root menu: submenus aren't interactive yet");
                                    }
                                }
                                should_close = !popup.pinned;
                            } else if !popup.pinned {
                                // Released on the popup's own padding, not
                                // an item or the pushpin.
                                should_close = true;
                            }
                        } else if !popup.pinned {
                            // Released off the popup entirely -- a pinned
                            // popup stays up through this, same as a real
                            // persistent palette would.
                            should_close = true;
                        }
                    }

                    if let Some(command) = command_to_run {
                        Self::run_command(&command);
                    }
                    if should_close {
                        self.close_menu();
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
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        // The only surface we ever request keyboard focus for is the menu
        // popup (Exclusive interactivity), so there's nothing else to gate
        // this on -- if we're getting key events at all, they're for it.
        if event.keysym == Keysym::Escape && self.popup.is_some() {
            self.close_menu();
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
                state.draw_panel();
                state.ensure_decoration(qh, &proxy.id());
                state.draw_decoration(&proxy.id());
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
                }
                proxy.destroy();
                state.draw_panel();
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
            Event::WorkspaceCount { count } => {
                state.workspace_count = count;
                log::info!("workspaces: {count} available");
                state.draw_panel();
            }
            Event::ActiveChanged { index } => {
                state.active_workspace = index;
                log::info!("workspaces: active = {index}");
                state.draw_panel();
            }
            Event::ToplevelWorkspace { toplevel, index } => {
                let title = state.toplevels.get(&toplevel.id()).map(|i| i.title.clone());
                log::info!("workspaces: toplevel {title:?} -> workspace {index}");
            }
        }
    }
}
