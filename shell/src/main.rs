// olshell - the unprivileged half of olwc. An ordinary wlr-layer-shell
// Wayland client: no compositor privileges, talks to olcore purely over
// standard and (eventually) custom Wayland protocol extensions. This
// skeleton binds the core globals and paints a single anchored panel
// surface, standing in for the eventual root menu / workspace strip.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
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
};
use wayland_client::{
    backend::ObjectId,
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};

// Linux input event codes (linux/input-event-codes.h), as reported in
// wl_pointer button events. OPEN LOOK's MENU button is the right button.
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

mod menu;
use menu::{Menu, MenuNode};

const PANEL_HEIGHT: u32 = 28;
const PANEL_FONT_SIZE: f32 = 20.0;
const PANEL_ENTRY_GAP: i32 = 20;
const PANEL_TEXT_COLOR: (u8, u8, u8) = (0x20, 0x20, 0x20);

const BACKGROUND_COLOR: (u8, u8, u8) = (0x5A, 0x76, 0x8C);

const MENU_FONT_SIZE: f32 = 18.0;
const MENU_ROW_HEIGHT: i32 = 26;
const MENU_H_PADDING: i32 = 12;
const MENU_BG_COLOR: (u8, u8, u8) = (0xD8, 0xD8, 0xD0);
const MENU_HOVER_COLOR: (u8, u8, u8) = (0x8A, 0x9E, 0xB0);
const MENU_TITLE_COLOR: (u8, u8, u8) = (0x40, 0x40, 0x38);
const MENU_TEXT_COLOR: (u8, u8, u8) = (0x18, 0x18, 0x18);

// SIL Open Font License 1.1, see assets/fonts/OFL.txt.
static PANEL_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/VT323-Regular.ttf");

#[derive(Default, Debug)]
struct ToplevelInfo {
    title: String,
    app_id: String,
    states: Vec<u32>,
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
}

impl MenuPopup {
    fn title_rows(&self) -> i32 {
        if self.title.is_some() {
            1
        } else {
            0
        }
    }

    /// Item index under `y` (surface-local), if any.
    fn item_at(&self, y: f64) -> Option<usize> {
        // Do the boundary check and division in f64 throughout -- mixing in
        // i32 here is a trap: integer division truncates toward zero, not
        // toward -inf, so a title-row y (which makes the numerator
        // negative) doesn't reliably come out negative after dividing.
        let title_h = (self.title_rows() * MENU_ROW_HEIGHT) as f64;
        if y < title_h {
            return None;
        }
        let row = ((y - title_h) / MENU_ROW_HEIGHT as f64) as usize;
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

    let font = fontdue::Font::from_bytes(PANEL_FONT_BYTES, fontdue::FontSettings::default())
        .expect("failed to parse embedded panel font");

    let mut state = Olshell {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
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
        toplevels: std::collections::HashMap::new(),
        workspace_count: 0,
        active_workspace: 0,
        font,
        menu,
        pointer: None,
        popup: None,
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
    pool: SlotPool,
    layer: LayerSurface,
    width: u32,
    height: u32,
    background: LayerSurface,
    bg_width: u32,
    bg_height: u32,
    exit: bool,
    // Kept only to hold the binding alive; requests aren't sent yet, so
    // nothing reads these beyond the Option check at startup.
    #[allow(dead_code)]
    foreign_toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,
    #[allow(dead_code)]
    workspaces_manager: Option<ZopenlookWorkspacesManagerV1>,
    toplevels: std::collections::HashMap<ObjectId, ToplevelInfo>,
    workspace_count: u32,
    active_workspace: u32,
    font: fontdue::Font,
    menu: Menu,
    pointer: Option<wl_pointer::WlPointer>,
    popup: Option<MenuPopup>,
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

        // OpenLook-style neutral slate panel fill, opaque.
        for pixel in canvas.chunks_exact_mut(4) {
            pixel[0] = 0xBE; // B
            pixel[1] = 0xBE; // G
            pixel[2] = 0xBE; // R
            pixel[3] = 0xFF; // A
        }

        // Flat list of every mapped toplevel's title, left to right. No
        // workspace filtering, wrapping, or scrolling yet -- entries past
        // the panel's width are simply clipped.
        let mut x = 8;
        let mut titles: Vec<&str> = self
            .toplevels
            .values()
            .map(|info| info.title.as_str())
            .filter(|title| !title.is_empty())
            .collect();
        titles.sort_unstable();
        for title in titles {
            if x >= width {
                break;
            }
            x = draw_text(canvas, width, height, x, title, &self.font, PANEL_FONT_SIZE, PANEL_TEXT_COLOR);
            x += PANEL_ENTRY_GAP;
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

    /// Opens the root menu at `(x, y)` (surface-local coordinates on the
    /// background, which is fullscreen so those are effectively screen
    /// coordinates). Anchoring a layer surface to top-left with margins is
    /// the standard wlr-layer-shell trick for pixel-precise popup placement.
    fn open_menu(&mut self, qh: &QueueHandle<Self>, x: f64, y: f64) {
        self.close_menu();

        let items = self.menu.items.clone();
        let title = self.menu.title.clone();

        let mut max_width = 0i32;
        for label in title.iter().map(String::as_str).chain(items.iter().map(MenuNode::label)) {
            let w: i32 = label
                .chars()
                .map(|c| self.font.metrics(c, MENU_FONT_SIZE).advance_width.round() as i32)
                .sum();
            max_width = max_width.max(w);
        }
        let width = (max_width + MENU_H_PADDING * 2).max(80) as u32;
        let rows = items.len() as i32 + if title.is_some() { 1 } else { 0 };
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
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();

        self.popup = Some(MenuPopup { layer, items, title, width, height, hovered: None });
    }

    fn close_menu(&mut self) {
        // Just drop it -- sctk's LayerSurface::Drop already destroys the
        // zwlr_layer_surface_v1 role object before the wl_surface, in the
        // order the protocol requires. Destroying the wl_surface ourselves
        // first (as this used to do) is a protocol violation: "surface was
        // destroyed before its role object".
        self.popup = None;
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

    let mut row = 0;
    if let Some(title) = &popup.title {
        // draw_text centers in the *whole* canvas height, not just this
        // row -- for a multi-row popup that puts the title text down in
        // the first item's row instead of its own, leaving row 0 blank
        // and garbling whatever's hovered in row 1. Row-centered instead.
        draw_text_row_centered(
            canvas, width, 0, MENU_ROW_HEIGHT, MENU_H_PADDING,
            title, font, MENU_FONT_SIZE, MENU_TITLE_COLOR,
        );
        row += 1;
    }

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

/// Rasterizes `text` starting at `start_x`, vertically centered in a canvas
/// of the given height, alpha-blended over whatever's already there.
/// Returns the x position just past the last glyph drawn.
fn draw_text(
    canvas: &mut [u8],
    canvas_width: i32,
    canvas_height: i32,
    start_x: i32,
    text: &str,
    font: &fontdue::Font,
    size: f32,
    color: (u8, u8, u8),
) -> i32 {
    let baseline_y = canvas_height / 2 + (size as i32) / 3;
    draw_text_at(canvas, canvas_width, canvas_height, start_x, baseline_y, text, font, size, color)
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
            let on_popup = self
                .popup
                .as_ref()
                .is_some_and(|p| event.surface == *p.layer.wl_surface());

            match event.kind {
                PointerEventKind::Press { button, .. } if on_background && button == BTN_RIGHT => {
                    self.open_menu(qh, event.position.0, event.position.1);
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
                    if on_popup {
                        let popup = self.popup.as_ref().unwrap();
                        if let Some(index) = popup.item_at(event.position.1) {
                            if let MenuNode::Item { command, .. } = &popup.items[index] {
                                Self::run_command(command);
                            } else {
                                log::info!("root menu: submenus aren't interactive yet");
                            }
                        }
                    }
                    if self.popup.is_some() {
                        self.close_menu();
                    }
                }
                _ => {}
            }
        }
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
delegate_layer!(Olshell);
delegate_registry!(Olshell);

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
            state.toplevels.insert(toplevel.id(), ToplevelInfo::default());
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
        _qh: &QueueHandle<Self>,
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
            }
            Event::Closed => {
                state.toplevels.remove(&proxy.id());
                proxy.destroy();
                state.draw_panel();
            }
            Event::OutputEnter { .. } | Event::OutputLeave { .. } | Event::Parent { .. } => {}
            _ => {}
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
            }
            Event::ActiveChanged { index } => {
                state.active_workspace = index;
                log::info!("workspaces: active = {index}");
            }
            Event::ToplevelWorkspace { toplevel, index } => {
                let title = state.toplevels.get(&toplevel.id()).map(|i| i.title.clone());
                log::info!("workspaces: toplevel {title:?} -> workspace {index}");
            }
        }
    }
}
