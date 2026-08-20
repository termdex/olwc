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
// SELECT is the left one.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
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
const DECORATION_BEVEL_LIGHT: (u8, u8, u8) = (0xE8, 0xE8, 0xE8);
const DECORATION_BEVEL_DARK: (u8, u8, u8) = (0x70, 0x70, 0x70);
const DECORATION_TEXT_COLOR: (u8, u8, u8) = (0x18, 0x18, 0x18);
const DECORATION_BUTTON_SIZE: i32 = 14;
const DECORATION_BUTTON_MARGIN: i32 = 4;
const DECORATION_BUTTON_HOVER_COLOR: (u8, u8, u8) = MENU_HOVER_COLOR;
const DECORATION_FONT_SIZE: f32 = 15.0;

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
    WindowMenuItem { label: "Stick", action: WindowMenuAction::Unimplemented, disabled: false },
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
    button_hovered: bool,
}

impl Decoration {
    fn button_rect(&self) -> (i32, i32, i32, i32) {
        let x0 = DECORATION_BUTTON_MARGIN;
        let y0 = (self.height as i32 - DECORATION_BUTTON_SIZE) / 2;
        (x0, y0, x0 + DECORATION_BUTTON_SIZE, y0 + DECORATION_BUTTON_SIZE)
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

        let info = self.toplevels.get_mut(toplevel_id).unwrap();
        info.decoration = Some(Decoration {
            surface,
            object,
            width: 0,
            height: DECORATION_HEIGHT,
            button_hovered: false,
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

        let (r, g, b) = DECORATION_BG_COLOR;
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = b;
            pixel[1] = g;
            pixel[2] = r;
            pixel[3] = 0xFF;
        }
        // 3D beveled shading: a light edge along the top, a dark edge along
        // the bottom, raised-unpressed per docs/OPENLOOK-REFERENCE.md.
        paint_row(canvas, width, 0, DECORATION_BEVEL_LIGHT);
        if height > 1 {
            paint_row(canvas, width, height - 1, DECORATION_BEVEL_DARK);
        }

        let (bx0, by0, bx1, by1) = dec.button_rect();
        let button_color =
            if dec.button_hovered { DECORATION_BUTTON_HOVER_COLOR } else { DECORATION_BG_COLOR };
        fill_rect(canvas, width, height, bx0, by0, bx1, by1, button_color);
        draw_chevron(canvas, width, height, bx0, by0, bx1, by1, DECORATION_TEXT_COLOR);

        if !info.title.is_empty() {
            draw_text_row_centered(
                canvas, width, 0, height, bx1 + DECORATION_BUTTON_MARGIN,
                &info.title, &self.font, DECORATION_FONT_SIZE, DECORATION_TEXT_COLOR,
            );
        }

        let wl_surface = &dec.surface;
        buffer.attach_to(wl_surface).expect("failed to attach buffer");
        wl_surface.damage_buffer(0, 0, width, height);
        wl_surface.commit();
        log::info!("decoration: drew {toplevel_id:?} at {width}x{height}");
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
        let max_width = WINDOW_MENU_ITEMS.iter().map(|item| label_width(item.label)).max().unwrap_or(0);
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
            draw_text_row_centered(
                canvas, width, row_y0, MENU_ROW_HEIGHT, MENU_H_PADDING,
                item.label, &self.font, MENU_FONT_SIZE, color,
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

    /// Workspace index (0-based) the panel-local `x` falls on, if any.
    fn workspace_at(&self, x: f64) -> Option<u32> {
        let x = x as i32;
        (0..self.workspace_count).find(|&i| {
            let (x0, x1) = workspace_segment_x(i);
            x >= x0 && x < x1
        })
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
            Event::Configure { serial, width, height } => {
                log::info!("decoration: configure {toplevel_id:?} {width}x{height}");
                proxy.ack_configure(serial);
                if let Some(dec) =
                    state.toplevels.get_mut(toplevel_id).and_then(|info| info.decoration.as_mut())
                {
                    dec.width = width;
                    dec.height = height;
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
                    dec.object.destroy();
                    dec.surface.destroy();
                }
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
