// olcore - the privileged compositor half of olwc (OpenLook Wayland Compositor).
//
// This is a minimal wlroots-based compositor skeleton, structured after
// wlroots' tinywl example. It brings up a wlroots backend/renderer, manages
// outputs and xdg-shell toplevels via wlr_scene, and handles basic
// keyboard/pointer input (including interactive move/resize) so that a
// client window can be mapped and displayed. It is meant as a scaffold for
// olcore to grow OpenLook-specific window management on top of; the actual
// menu/decoration UI lives out-of-process in olshell.

#define _POSIX_C_SOURCE 200809L

#include <assert.h>
#include <getopt.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <wayland-server-core.h>
#include <wlr/backend.h>
#include <wlr/render/allocator.h>
#include <wlr/render/wlr_renderer.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_data_device.h>
#include <wlr/types/wlr_foreign_toplevel_management_v1.h>
#include <wlr/types/wlr_input_device.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_layer_shell_v1.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_subcompositor.h>
#include <wlr/types/wlr_xcursor_manager.h>
#include <wlr/types/wlr_xdg_decoration_v1.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>
#include <xkbcommon/xkbcommon.h>

#include "openlook-decoration-unstable-v1-protocol.h"
#include "openlook-workspaces-unstable-v1-protocol.h"

// Fixed linear workspace count for this scaffold. A real implementation
// would make this configurable; olcore's only job is to track "how many"
// and "which is active", per the openlook-workspaces protocol.
#define OLC_WORKSPACE_COUNT 4

enum olc_cursor_mode {
	OLC_CURSOR_PASSTHROUGH,
	OLC_CURSOR_MOVE,
	OLC_CURSOR_RESIZE,
};

// Scene-graph stacking order, bottom to top. Toplevels sit between the
// wlr-layer-shell BOTTOM and TOP layers, per the layer-shell protocol.
enum olc_scene_layer {
	OLC_LAYER_BACKGROUND,
	OLC_LAYER_BOTTOM,
	OLC_LAYER_TOPLEVELS,
	OLC_LAYER_TOP,
	OLC_LAYER_OVERLAY,
	OLC_NUM_LAYERS,
};

struct olc_server {
	struct wl_display *wl_display;
	struct wlr_backend *backend;
	struct wlr_session *session;
	struct wlr_renderer *renderer;
	struct wlr_allocator *allocator;
	struct wlr_scene *scene;
	struct wlr_scene_output_layout *scene_layout;
	struct wlr_scene_tree *layers[OLC_NUM_LAYERS];

	struct wlr_xdg_shell *xdg_shell;
	struct wl_listener new_xdg_toplevel;
	struct wl_listener new_xdg_popup;
	struct wl_list toplevels; // olc_toplevel::link
	struct wlr_foreign_toplevel_manager_v1 *foreign_toplevel_manager;

	// xdg-decoration: tells well-behaved clients to skip drawing their own
	// title bar, since olshell draws one via openlook-decoration below.
	struct wlr_xdg_decoration_manager_v1 *xdg_decoration_manager;
	struct wl_listener new_xdg_toplevel_decoration;

	// openlook-decoration: lets olshell attach header chrome to a toplevel
	// it doesn't own. See protocol/openlook-decoration-unstable-v1.xml.
	struct wl_global *decoration_manager_global;

	struct wlr_layer_shell_v1 *layer_shell;
	struct wl_listener new_layer_surface;
	struct wl_list layer_surfaces; // olc_layer_surface::link

	struct wl_global *workspaces_manager_global;
	struct wl_list workspace_resources; // olc_workspaces_resource::link
	uint32_t workspace_count;
	uint32_t active_workspace;

	struct wlr_cursor *cursor;
	struct wlr_xcursor_manager *cursor_mgr;
	struct wl_listener cursor_motion;
	struct wl_listener cursor_motion_absolute;
	struct wl_listener cursor_button;
	struct wl_listener cursor_axis;
	struct wl_listener cursor_frame;

	struct wlr_seat *seat;
	struct wl_listener new_input;
	struct wl_listener request_cursor;
	struct wl_listener request_set_selection;
	struct wl_list keyboards; // olc_keyboard::link

	enum olc_cursor_mode cursor_mode;
	struct olc_toplevel *grabbed_toplevel;
	double grab_x, grab_y;
	struct wlr_box grab_geobox;
	uint32_t resize_edges;
	// A move/resize grab started with a button already held (dragging the
	// header, or a client's own CSD) naturally ends when that button is
	// released. One started with nothing held (the window menu's Move/
	// Resize items: by the time their request reaches olcore, the click
	// that selected them has already fully released) has no held button
	// to end on, so it instead ends on the *next* press, click-to-arm/
	// click-to-confirm style. See start_move_grab/start_resize_grab.
	bool cursor_end_on_press;

	struct wlr_output_layout *output_layout;
	struct wl_list outputs; // olc_output::link
	struct wl_listener new_output;
	struct wl_event_source *render_timer;
};

struct olc_output {
	struct wl_list link;
	struct olc_server *server;
	struct wlr_output *wlr_output;
	struct wl_listener frame;
	struct wl_listener request_state;
	struct wl_listener destroy;

	// The output's box minus space claimed by exclusive-zone layer-shell
	// surfaces (the panel, primarily). Updated by arrange_output_layers();
	// zeroed (and thus not yet meaningful) until that's run at least once.
	struct wlr_box usable_area;
};

struct olc_toplevel {
	struct wl_list link;
	struct olc_server *server;
	struct wlr_xdg_toplevel *xdg_toplevel;
	struct wlr_scene_tree *scene_tree;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener commit;
	struct wl_listener destroy;
	struct wl_listener request_move;
	struct wl_listener request_resize;
	struct wl_listener request_maximize;
	struct wl_listener request_fullscreen;
	struct wl_listener set_title;
	struct wl_listener set_app_id;

	// wlr-foreign-toplevel-management: only valid while mapped (created in
	// xdg_toplevel_map, destroyed in xdg_toplevel_unmap).
	struct wlr_foreign_toplevel_handle_v1 *foreign_handle;
	struct wl_listener foreign_request_maximize;
	struct wl_listener foreign_request_minimize;
	struct wl_listener foreign_request_activate;
	struct wl_listener foreign_request_fullscreen;
	struct wl_listener foreign_request_close;
	uint32_t workspace_index;
	// Tracked so update_toplevel_visibility() can combine it with workspace
	// membership -- minimize and workspace-switching are two independent
	// reasons a toplevel might need to be hidden, and applying one must
	// not clobber the other (e.g. switching to a minimized window's
	// workspace shouldn't un-minimize it).
	bool minimized;
	// A sticky toplevel stays visible regardless of workspace_index --
	// exempt from the hiding switching workspaces normally causes.
	// workspace_index itself is left alone by stickiness (still its
	// nominal "home" workspace), so un-sticking it later just resumes
	// normal per-workspace visibility rather than needing to remember or
	// restore anything.
	bool sticky;

	// openlook-decoration: at most one header decoration, requested by
	// olshell. NULL until olshell asks for one; see get_decoration.
	struct olc_decoration *decoration;

	// xdg-decoration: set in server_new_xdg_toplevel_decoration when a
	// client asks for a decoration object before its xdg_surface is ready
	// to receive configures (the common case -- clients typically do this
	// right after creating the toplevel, before their first commit).
	// Consumed (and cleared) by xdg_toplevel_commit once it's ready to
	// call wlr_xdg_toplevel_decoration_v1_set_mode() safely.
	struct wlr_xdg_toplevel_decoration_v1 *pending_xdg_decoration;
};

// One olshell-supplied surface acting as the header decoration for a single
// toplevel. Heap-allocated separately from olc_toplevel (rather than
// embedded) so it can safely outlive the toplevel being destroyed out from
// under it -- see decoration_send_closed().
struct olc_decoration {
	struct wl_resource *resource; // zopenlook_decoration_v1; never NULL
	struct olc_toplevel *toplevel; // NULL once the toplevel is gone
	struct wlr_surface *surface;
	struct wlr_scene_tree *scene_tree; // child of toplevel->scene_tree
	struct wl_listener surface_destroy;
	struct wl_listener surface_commit;
	uint32_t height;
	uint32_t configured_width;
	bool configured;
};

struct olc_popup {
	struct wlr_xdg_popup *xdg_popup;
	struct wl_listener commit;
	struct wl_listener destroy;
};

struct olc_layer_surface {
	struct wl_list link;
	struct olc_server *server;
	struct wlr_layer_surface_v1 *layer_surface;
	struct wlr_scene_layer_surface_v1 *scene_layer_surface;
	struct wl_listener map;
	struct wl_listener commit;
	struct wl_listener destroy;
};

// One per client binding of zopenlook_workspaces_manager_v1. wl_resource
// has no public link field of its own, so we wrap it to keep a list we can
// broadcast active_changed to.
struct olc_workspaces_resource {
	struct wl_resource *resource;
	struct olc_server *server;
	struct wl_list link;
};

struct olc_keyboard {
	struct wl_list link;
	struct olc_server *server;
	struct wlr_keyboard *wlr_keyboard;
	struct wl_listener modifiers;
	struct wl_listener key;
	struct wl_listener destroy;
};

static struct olc_toplevel *olc_toplevel_from_xdg_toplevel(struct wlr_xdg_toplevel *xdg_toplevel) {
	struct wlr_scene_tree *tree = xdg_toplevel->base->data;
	return tree->node.data;
}

// A toplevel is visible iff it's not minimized AND it's on the active
// workspace -- two independent reasons to be hidden, combined here so
// setting one doesn't accidentally clobber the other. The decoration
// header is a child of scene_tree, so disabling the toplevel already
// takes it along for free.
static bool toplevel_is_visible(struct olc_toplevel *toplevel) {
	return !toplevel->minimized &&
		(toplevel->sticky || toplevel->workspace_index == toplevel->server->active_workspace);
}

static void update_toplevel_visibility(struct olc_toplevel *toplevel) {
	wlr_scene_node_set_enabled(&toplevel->scene_tree->node, toplevel_is_visible(toplevel));
}

static void focus_toplevel(struct olc_toplevel *toplevel) {
	if (toplevel == NULL) {
		return;
	}
	struct olc_server *server = toplevel->server;
	struct wlr_seat *seat = server->seat;
	struct wlr_surface *prev_surface = seat->keyboard_state.focused_surface;
	struct wlr_surface *surface = toplevel->xdg_toplevel->base->surface;
	if (prev_surface == surface) {
		return;
	}
	if (prev_surface) {
		struct wlr_xdg_toplevel *prev_toplevel =
			wlr_xdg_toplevel_try_from_wlr_surface(prev_surface);
		if (prev_toplevel != NULL) {
			wlr_xdg_toplevel_set_activated(prev_toplevel, false);
			struct olc_toplevel *prev_olc = olc_toplevel_from_xdg_toplevel(prev_toplevel);
			if (prev_olc->foreign_handle != NULL) {
				wlr_foreign_toplevel_handle_v1_set_activated(prev_olc->foreign_handle, false);
			}
		}
	}
	struct wlr_keyboard *keyboard = wlr_seat_get_keyboard(seat);

	wlr_scene_node_raise_to_top(&toplevel->scene_tree->node);
	wl_list_remove(&toplevel->link);
	wl_list_insert(&server->toplevels, &toplevel->link);

	wlr_xdg_toplevel_set_activated(toplevel->xdg_toplevel, true);
	if (toplevel->foreign_handle != NULL) {
		wlr_foreign_toplevel_handle_v1_set_activated(toplevel->foreign_handle, true);
	}
	if (keyboard != NULL) {
		wlr_seat_keyboard_notify_enter(seat, surface, keyboard->keycodes,
			keyboard->num_keycodes, &keyboard->modifiers);
	}
}

static void keyboard_handle_modifiers(struct wl_listener *listener, void *data) {
	struct olc_keyboard *keyboard = wl_container_of(listener, keyboard, modifiers);
	wlr_seat_set_keyboard(keyboard->server->seat, keyboard->wlr_keyboard);
	wlr_seat_keyboard_notify_modifiers(keyboard->server->seat,
		&keyboard->wlr_keyboard->modifiers);
}

static bool handle_keybinding(struct olc_server *server, xkb_keysym_t sym) {
	switch (sym) {
	case XKB_KEY_Escape:
		wl_display_terminate(server->wl_display);
		break;
	default:
		return false;
	}
	return true;
}

static void keyboard_handle_key(struct wl_listener *listener, void *data) {
	struct olc_keyboard *keyboard = wl_container_of(listener, keyboard, key);
	struct olc_server *server = keyboard->server;
	struct wlr_keyboard_key_event *event = data;
	struct wlr_seat *seat = server->seat;

	uint32_t keycode = event->keycode + 8;
	const xkb_keysym_t *syms;
	int nsyms = xkb_state_key_get_syms(keyboard->wlr_keyboard->xkb_state, keycode, &syms);

	bool handled = false;
	uint32_t modifiers = wlr_keyboard_get_modifiers(keyboard->wlr_keyboard);
	if ((modifiers & WLR_MODIFIER_ALT) && event->state == WL_KEYBOARD_KEY_STATE_PRESSED) {
		for (int i = 0; i < nsyms; i++) {
			handled = handle_keybinding(server, syms[i]);
		}
	}

	if (!handled) {
		wlr_seat_set_keyboard(seat, keyboard->wlr_keyboard);
		wlr_seat_keyboard_notify_key(seat, event->time_msec, event->keycode, event->state);
	}
}

static void keyboard_handle_destroy(struct wl_listener *listener, void *data) {
	struct olc_keyboard *keyboard = wl_container_of(listener, keyboard, destroy);
	wl_list_remove(&keyboard->modifiers.link);
	wl_list_remove(&keyboard->key.link);
	wl_list_remove(&keyboard->destroy.link);
	wl_list_remove(&keyboard->link);
	free(keyboard);
}

static void server_new_keyboard(struct olc_server *server, struct wlr_input_device *device) {
	struct wlr_keyboard *wlr_keyboard = wlr_keyboard_from_input_device(device);

	struct olc_keyboard *keyboard = calloc(1, sizeof(*keyboard));
	keyboard->server = server;
	keyboard->wlr_keyboard = wlr_keyboard;

	struct xkb_context *context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	struct xkb_keymap *keymap = xkb_keymap_new_from_names(context, NULL, XKB_KEYMAP_COMPILE_NO_FLAGS);

	wlr_keyboard_set_keymap(wlr_keyboard, keymap);
	xkb_keymap_unref(keymap);
	xkb_context_unref(context);
	wlr_keyboard_set_repeat_info(wlr_keyboard, 25, 600);

	keyboard->modifiers.notify = keyboard_handle_modifiers;
	wl_signal_add(&wlr_keyboard->events.modifiers, &keyboard->modifiers);
	keyboard->key.notify = keyboard_handle_key;
	wl_signal_add(&wlr_keyboard->events.key, &keyboard->key);
	keyboard->destroy.notify = keyboard_handle_destroy;
	wl_signal_add(&device->events.destroy, &keyboard->destroy);

	wlr_seat_set_keyboard(server->seat, keyboard->wlr_keyboard);
	wl_list_insert(&server->keyboards, &keyboard->link);
}

static void server_new_pointer(struct olc_server *server, struct wlr_input_device *device) {
	wlr_cursor_attach_input_device(server->cursor, device);
}

static void server_new_input(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, new_input);
	struct wlr_input_device *device = data;
	switch (device->type) {
	case WLR_INPUT_DEVICE_KEYBOARD:
		server_new_keyboard(server, device);
		break;
	case WLR_INPUT_DEVICE_POINTER:
		server_new_pointer(server, device);
		break;
	default:
		break;
	}

	uint32_t caps = WL_SEAT_CAPABILITY_POINTER;
	if (!wl_list_empty(&server->keyboards)) {
		caps |= WL_SEAT_CAPABILITY_KEYBOARD;
	}
	wlr_seat_set_capabilities(server->seat, caps);
}

static void seat_request_cursor(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, request_cursor);
	struct wlr_seat_pointer_request_set_cursor_event *event = data;
	struct wlr_seat_client *focused_client = server->seat->pointer_state.focused_client;
	if (focused_client == event->seat_client) {
		wlr_cursor_set_surface(server->cursor, event->surface, event->hotspot_x, event->hotspot_y);
	}
}

static void seat_request_set_selection(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, request_set_selection);
	struct wlr_seat_request_set_selection_event *event = data;
	wlr_seat_set_selection(server->seat, event->source, event->serial);
}

static struct olc_toplevel *desktop_toplevel_at(
		struct olc_server *server, double lx, double ly,
		struct wlr_surface **surface, double *sx, double *sy) {
	struct wlr_scene_node *node = wlr_scene_node_at(&server->scene->tree.node, lx, ly, sx, sy);
	if (node == NULL || node->type != WLR_SCENE_NODE_BUFFER) {
		return NULL;
	}
	struct wlr_scene_buffer *scene_buffer = wlr_scene_buffer_from_node(node);
	struct wlr_scene_surface *scene_surface = wlr_scene_surface_try_from_buffer(scene_buffer);
	if (!scene_surface) {
		return NULL;
	}

	*surface = scene_surface->surface;
	struct wlr_scene_tree *tree = node->parent;
	while (tree != NULL && tree->node.data == NULL) {
		tree = tree->node.parent;
	}
	return tree ? tree->node.data : NULL;
}

static void reset_cursor_mode(struct olc_server *server) {
	server->cursor_mode = OLC_CURSOR_PASSTHROUGH;
	server->grabbed_toplevel = NULL;
}

static void process_cursor_move(struct olc_server *server) {
	struct olc_toplevel *toplevel = server->grabbed_toplevel;
	wlr_scene_node_set_position(&toplevel->scene_tree->node,
		server->cursor->x - server->grab_x, server->cursor->y - server->grab_y);
}

static void process_cursor_resize(struct olc_server *server) {
	struct olc_toplevel *toplevel = server->grabbed_toplevel;
	double border_x = server->cursor->x - server->grab_x;
	double border_y = server->cursor->y - server->grab_y;
	int new_left = server->grab_geobox.x;
	int new_right = server->grab_geobox.x + server->grab_geobox.width;
	int new_top = server->grab_geobox.y;
	int new_bottom = server->grab_geobox.y + server->grab_geobox.height;

	if (server->resize_edges & WLR_EDGE_TOP) {
		new_top = border_y;
		if (new_top >= new_bottom) {
			new_top = new_bottom - 1;
		}
	} else if (server->resize_edges & WLR_EDGE_BOTTOM) {
		new_bottom = border_y;
		if (new_bottom <= new_top) {
			new_bottom = new_top + 1;
		}
	}
	if (server->resize_edges & WLR_EDGE_LEFT) {
		new_left = border_x;
		if (new_left >= new_right) {
			new_left = new_right - 1;
		}
	} else if (server->resize_edges & WLR_EDGE_RIGHT) {
		new_right = border_x;
		if (new_right <= new_left) {
			new_right = new_left + 1;
		}
	}

	struct wlr_box geo_box = toplevel->xdg_toplevel->base->geometry;
	wlr_scene_node_set_position(&toplevel->scene_tree->node,
		new_left - geo_box.x, new_top - geo_box.y);

	int new_width = new_right - new_left;
	int new_height = new_bottom - new_top;
	wlr_xdg_toplevel_set_size(toplevel->xdg_toplevel, new_width, new_height);
}

static void process_cursor_motion(struct olc_server *server, uint32_t time) {
	if (server->cursor_mode == OLC_CURSOR_MOVE) {
		process_cursor_move(server);
		return;
	} else if (server->cursor_mode == OLC_CURSOR_RESIZE) {
		process_cursor_resize(server);
		return;
	}

	double sx, sy;
	struct wlr_seat *seat = server->seat;
	struct wlr_surface *surface = NULL;
	struct olc_toplevel *toplevel = desktop_toplevel_at(server,
		server->cursor->x, server->cursor->y, &surface, &sx, &sy);
	if (!toplevel) {
		wlr_cursor_set_xcursor(server->cursor, server->cursor_mgr, "default");
	}
	if (surface) {
		wlr_seat_pointer_notify_enter(seat, surface, sx, sy);
		wlr_seat_pointer_notify_motion(seat, time, sx, sy);
	} else {
		wlr_seat_pointer_clear_focus(seat);
	}
}

static void server_cursor_motion(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, cursor_motion);
	struct wlr_pointer_motion_event *event = data;
	wlr_cursor_move(server->cursor, &event->pointer->base, event->delta_x, event->delta_y);
	process_cursor_motion(server, event->time_msec);
}

static void server_cursor_motion_absolute(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, cursor_motion_absolute);
	struct wlr_pointer_motion_absolute_event *event = data;
	wlr_cursor_warp_absolute(server->cursor, &event->pointer->base, event->x, event->y);
	process_cursor_motion(server, event->time_msec);
}

static void server_cursor_button(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, cursor_button);
	struct wlr_pointer_button_event *event = data;
	wlr_seat_pointer_notify_button(server->seat, event->time_msec, event->button, event->state);

	double sx, sy;
	struct wlr_surface *surface = NULL;
	struct olc_toplevel *toplevel = desktop_toplevel_at(server,
		server->cursor->x, server->cursor->y, &surface, &sx, &sy);
	bool grabbing = server->cursor_mode != OLC_CURSOR_PASSTHROUGH;
	if (event->state == WL_POINTER_BUTTON_STATE_RELEASED) {
		if (grabbing && !server->cursor_end_on_press) {
			reset_cursor_mode(server);
		}
	} else {
		if (grabbing && server->cursor_end_on_press) {
			reset_cursor_mode(server);
		}
		focus_toplevel(toplevel);
	}
}

static void server_cursor_axis(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, cursor_axis);
	struct wlr_pointer_axis_event *event = data;
	wlr_seat_pointer_notify_axis(server->seat, event->time_msec, event->orientation,
		event->delta, event->delta_discrete, event->source, event->relative_direction);
}

static void server_cursor_frame(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, cursor_frame);
	wlr_seat_pointer_notify_frame(server->seat);
}

static void render_output(struct olc_output *output) {
	struct wlr_scene_output *scene_output =
		wlr_scene_get_scene_output(output->server->scene, output->wlr_output);
	if (scene_output == NULL) {
		return;
	}
	wlr_scene_output_commit(scene_output, NULL);

	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	wlr_scene_output_send_frame_done(scene_output, &now);
}

static void output_frame(struct wl_listener *listener, void *data) {
	struct olc_output *output = wl_container_of(listener, output, frame);
	render_output(output);
}

#define OLC_RENDER_INTERVAL_MS 16

static int render_timer_handle(void *data) {
	struct olc_server *server = data;
	struct olc_output *output;
	wl_list_for_each(output, &server->outputs, link) {
		render_output(output);
	}
	wl_event_source_timer_update(server->render_timer, OLC_RENDER_INTERVAL_MS);
	return 0;
}

static void output_request_state(struct wl_listener *listener, void *data) {
	struct olc_output *output = wl_container_of(listener, output, request_state);
	const struct wlr_output_event_request_state *event = data;
	wlr_output_commit_state(output->wlr_output, event->state);
}

static void output_destroy(struct wl_listener *listener, void *data) {
	struct olc_output *output = wl_container_of(listener, output, destroy);
	wl_list_remove(&output->frame.link);
	wl_list_remove(&output->request_state.link);
	wl_list_remove(&output->destroy.link);
	wl_list_remove(&output->link);
	free(output);
}

static void server_new_output(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, new_output);
	struct wlr_output *wlr_output = data;

	wlr_output_init_render(wlr_output, server->allocator, server->renderer);

	struct wlr_output_state state;
	wlr_output_state_init(&state);
	wlr_output_state_set_enabled(&state, true);
	struct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);
	if (mode != NULL) {
		wlr_output_state_set_mode(&state, mode);
	}
	wlr_output_commit_state(wlr_output, &state);
	wlr_output_state_finish(&state);

	struct olc_output *output = calloc(1, sizeof(*output));
	output->wlr_output = wlr_output;
	output->server = server;

	output->frame.notify = output_frame;
	wl_signal_add(&wlr_output->events.frame, &output->frame);
	output->request_state.notify = output_request_state;
	wl_signal_add(&wlr_output->events.request_state, &output->request_state);
	output->destroy.notify = output_destroy;
	wl_signal_add(&wlr_output->events.destroy, &output->destroy);

	wl_list_insert(&server->outputs, &output->link);

	struct wlr_output_layout_output *l_output =
		wlr_output_layout_add_auto(server->output_layout, wlr_output);
	struct wlr_scene_output *scene_output = wlr_scene_output_create(server->scene, wlr_output);
	wlr_scene_output_layout_add_output(server->scene_layout, l_output, scene_output);
}

static struct olc_output *output_from_wlr_output(
		struct olc_server *server, struct wlr_output *wlr_output) {
	struct olc_output *output;
	wl_list_for_each(output, &server->outputs, link) {
		if (output->wlr_output == wlr_output) {
			return output;
		}
	}
	return NULL;
}

static struct wlr_scene_tree *layer_scene_tree(
		struct olc_server *server, enum zwlr_layer_shell_v1_layer layer) {
	switch (layer) {
	case ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND:
		return server->layers[OLC_LAYER_BACKGROUND];
	case ZWLR_LAYER_SHELL_V1_LAYER_BOTTOM:
		return server->layers[OLC_LAYER_BOTTOM];
	case ZWLR_LAYER_SHELL_V1_LAYER_TOP:
		return server->layers[OLC_LAYER_TOP];
	case ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY:
	default:
		return server->layers[OLC_LAYER_OVERLAY];
	}
}

// Recomputes size/position for every layer-shell surface docked to this
// output, per the wlr-layer-shell anchor/exclusive-zone rules.
static void arrange_output_layers(struct olc_output *output) {
	struct olc_server *server = output->server;
	struct wlr_box output_box;
	wlr_output_layout_get_box(server->output_layout, output->wlr_output, &output_box);
	if (output_box.width <= 0 || output_box.height <= 0) {
		return;
	}

	struct wlr_box full_area = {
		.x = 0, .y = 0, .width = output_box.width, .height = output_box.height,
	};
	struct wlr_box usable_area = full_area;

	struct olc_layer_surface *layer_surface;
	wl_list_for_each(layer_surface, &server->layer_surfaces, link) {
		if (layer_surface->layer_surface->output != output->wlr_output) {
			continue;
		}
		wlr_scene_layer_surface_v1_configure(
			layer_surface->scene_layer_surface, &full_area, &usable_area);
		wlr_scene_node_set_position(&layer_surface->scene_layer_surface->tree->node,
			layer_surface->scene_layer_surface->tree->node.x + output_box.x,
			layer_surface->scene_layer_surface->tree->node.y + output_box.y);
	}

	output->usable_area = usable_area;
}

static void layer_surface_map(struct wl_listener *listener, void *data) {
	struct olc_layer_surface *layer_surface = wl_container_of(listener, layer_surface, map);
	struct olc_output *output =
		output_from_wlr_output(layer_surface->server, layer_surface->layer_surface->output);
	if (output != NULL) {
		arrange_output_layers(output);
	}

	// Per wlr-layer-shell, "exclusive" keyboard interactivity on the top or
	// overlay layer means this surface should get keyboard focus for as
	// long as it's mapped (e.g. a menu popup that Escape should be able to
	// dismiss). keyboard_handle_key() already forwards key events to
	// whatever the seat considers focused, so granting focus here is the
	// only piece needed to make that work.
	struct wlr_layer_surface_v1 *wlr_layer_surface = layer_surface->layer_surface;
	if (wlr_layer_surface->current.keyboard_interactive ==
			ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_EXCLUSIVE) {
		struct wlr_seat *seat = layer_surface->server->seat;
		struct wlr_keyboard *keyboard = wlr_seat_get_keyboard(seat);
		wlr_seat_keyboard_notify_enter(seat, wlr_layer_surface->surface,
			keyboard ? keyboard->keycodes : NULL,
			keyboard ? keyboard->num_keycodes : 0,
			keyboard ? &keyboard->modifiers : NULL);
	}
}

static void layer_surface_commit(struct wl_listener *listener, void *data) {
	struct olc_layer_surface *layer_surface = wl_container_of(listener, layer_surface, commit);
	struct wlr_layer_surface_v1 *wlr_layer_surface = layer_surface->layer_surface;

	// Only re-arrange when this commit actually changed layer-shell state
	// (anchor, size, exclusive zone, ...). Re-arranging unconditionally on
	// every commit -- including the ones the client sends purely to redraw
	// a buffer at the size we already gave it -- re-sends a configure each
	// time, which makes the client redraw again: an infinite ping-pong.
	if (!wlr_layer_surface->initialized || wlr_layer_surface->current.committed == 0) {
		return;
	}

	struct olc_output *output =
		output_from_wlr_output(layer_surface->server, wlr_layer_surface->output);
	if (output != NULL) {
		arrange_output_layers(output);
	}
}

static void layer_surface_destroy(struct wl_listener *listener, void *data) {
	struct olc_layer_surface *layer_surface = wl_container_of(listener, layer_surface, destroy);
	struct olc_server *server = layer_surface->server;
	struct olc_output *output =
		output_from_wlr_output(server, layer_surface->layer_surface->output);

	bool had_keyboard_focus =
		server->seat->keyboard_state.focused_surface == layer_surface->layer_surface->surface;

	wl_list_remove(&layer_surface->map.link);
	wl_list_remove(&layer_surface->commit.link);
	wl_list_remove(&layer_surface->destroy.link);
	wl_list_remove(&layer_surface->link);
	free(layer_surface);

	if (had_keyboard_focus) {
		wlr_seat_keyboard_notify_clear_focus(server->seat);
		struct olc_toplevel *toplevel;
		wl_list_for_each(toplevel, &server->toplevels, link) {
			focus_toplevel(toplevel);
			break;
		}
	}

	if (output != NULL) {
		arrange_output_layers(output);
	}
}

static void server_new_layer_surface(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, new_layer_surface);
	struct wlr_layer_surface_v1 *wlr_layer_surface = data;

	if (wlr_layer_surface->output == NULL) {
		if (wl_list_empty(&server->outputs)) {
			wlr_layer_surface_v1_destroy(wlr_layer_surface);
			return;
		}
		struct olc_output *first_output =
			wl_container_of(server->outputs.next, first_output, link);
		wlr_layer_surface->output = first_output->wlr_output;
	}

	struct olc_layer_surface *layer_surface = calloc(1, sizeof(*layer_surface));
	layer_surface->server = server;
	layer_surface->layer_surface = wlr_layer_surface;
	layer_surface->scene_layer_surface = wlr_scene_layer_surface_v1_create(
		layer_scene_tree(server, wlr_layer_surface->pending.layer), wlr_layer_surface);
	wlr_layer_surface->data = layer_surface->scene_layer_surface;

	layer_surface->map.notify = layer_surface_map;
	wl_signal_add(&wlr_layer_surface->surface->events.map, &layer_surface->map);
	layer_surface->commit.notify = layer_surface_commit;
	wl_signal_add(&wlr_layer_surface->surface->events.commit, &layer_surface->commit);
	layer_surface->destroy.notify = layer_surface_destroy;
	wl_signal_add(&wlr_layer_surface->events.destroy, &layer_surface->destroy);

	wl_list_insert(&server->layer_surfaces, &layer_surface->link);
	// Not arranged/configured here: wlr_layer_surface_v1_configure() requires
	// the surface to already be initialized, which only happens once the
	// client's first commit lands. layer_surface_commit() handles that.
}

// Sends toplevel_workspace to workspaces_resource only if its client also
// holds a wlr-foreign-toplevel-management resource for this handle -- a
// client that never bound that protocol has no object to reference.
static void send_toplevel_workspace(struct wl_resource *workspaces_resource,
		struct wlr_foreign_toplevel_handle_v1 *handle, uint32_t index) {
	struct wl_client *client = wl_resource_get_client(workspaces_resource);
	struct wl_resource *handle_resource;
	wl_resource_for_each(handle_resource, &handle->resources) {
		if (wl_resource_get_client(handle_resource) == client) {
			zopenlook_workspaces_manager_v1_send_toplevel_workspace(
				workspaces_resource, handle_resource, index);
			return;
		}
	}
}

static void broadcast_toplevel_workspace(struct olc_server *server,
		struct wlr_foreign_toplevel_handle_v1 *handle, uint32_t index) {
	struct olc_workspaces_resource *wr;
	wl_list_for_each(wr, &server->workspace_resources, link) {
		send_toplevel_workspace(wr->resource, handle, index);
	}
}

// Finds the olc_toplevel a client-supplied zwlr_foreign_toplevel_handle_v1
// resource refers to. Deliberately avoids relying on that resource's
// wl_resource user-data layout (an internal wlroots detail we don't
// control) and instead walks the per-client resource lists we already
// maintain a reference into via olc_toplevel::foreign_handle.
static struct olc_toplevel *toplevel_from_foreign_handle_resource(
		struct olc_server *server, struct wl_resource *handle_resource) {
	struct olc_toplevel *toplevel;
	wl_list_for_each(toplevel, &server->toplevels, link) {
		if (toplevel->foreign_handle == NULL) {
			continue;
		}
		struct wl_resource *r;
		wl_resource_for_each(r, &toplevel->foreign_handle->resources) {
			if (r == handle_resource) {
				return toplevel;
			}
		}
	}
	return NULL;
}

static void toplevel_handle_request_maximize(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel =
		wl_container_of(listener, toplevel, foreign_request_maximize);
	struct wlr_foreign_toplevel_handle_v1_maximized_event *event = data;
	wlr_xdg_toplevel_set_maximized(toplevel->xdg_toplevel, event->maximized);
	wlr_foreign_toplevel_handle_v1_set_maximized(toplevel->foreign_handle, event->maximized);
}

static void toplevel_handle_request_minimize(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel =
		wl_container_of(listener, toplevel, foreign_request_minimize);
	struct wlr_foreign_toplevel_handle_v1_minimized_event *event = data;
	toplevel->minimized = event->minimized;
	update_toplevel_visibility(toplevel);
	wlr_foreign_toplevel_handle_v1_set_minimized(toplevel->foreign_handle, event->minimized);
}

static void toplevel_handle_request_activate(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel =
		wl_container_of(listener, toplevel, foreign_request_activate);
	focus_toplevel(toplevel);
}

static void toplevel_handle_request_fullscreen(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel =
		wl_container_of(listener, toplevel, foreign_request_fullscreen);
	struct wlr_foreign_toplevel_handle_v1_fullscreen_event *event = data;
	wlr_xdg_toplevel_set_fullscreen(toplevel->xdg_toplevel, event->fullscreen);
	wlr_foreign_toplevel_handle_v1_set_fullscreen(toplevel->foreign_handle, event->fullscreen);
}

static void toplevel_handle_request_close(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel =
		wl_container_of(listener, toplevel, foreign_request_close);
	wlr_xdg_toplevel_send_close(toplevel->xdg_toplevel);
}

static void toplevel_handle_set_title(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, set_title);
	if (toplevel->foreign_handle != NULL) {
		wlr_foreign_toplevel_handle_v1_set_title(
			toplevel->foreign_handle, toplevel->xdg_toplevel->title ?: "");
	}
}

static void toplevel_handle_set_app_id(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, set_app_id);
	if (toplevel->foreign_handle != NULL) {
		wlr_foreign_toplevel_handle_v1_set_app_id(
			toplevel->foreign_handle, toplevel->xdg_toplevel->app_id ?: "");
	}
}

// Places a newly-mapped toplevel at the top-left corner of the first
// output's usable area (the output box minus space claimed by exclusive-
// zone layer-shell surfaces, i.e. below/beside the panel). No cascading or
// centering -- just enough that new windows don't spawn with their top
// edge, and any header decoration olshell later attaches above it, hidden
// under the panel. Falls back to (0,0) if there's no output yet or layers
// haven't been arranged yet (usable_area still its zeroed initial value).
static void place_new_toplevel(struct olc_toplevel *toplevel) {
	if (wl_list_empty(&toplevel->server->outputs)) {
		return;
	}
	struct olc_output *output =
		wl_container_of(toplevel->server->outputs.next, output, link);
	if (output->usable_area.width <= 0 || output->usable_area.height <= 0) {
		return;
	}
	wlr_scene_node_set_position(&toplevel->scene_tree->node,
		output->usable_area.x, output->usable_area.y);
}

static void xdg_toplevel_map(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, map);
	wl_list_insert(&toplevel->server->toplevels, &toplevel->link);
	place_new_toplevel(toplevel);

	toplevel->foreign_handle =
		wlr_foreign_toplevel_handle_v1_create(toplevel->server->foreign_toplevel_manager);
	wlr_foreign_toplevel_handle_v1_set_title(
		toplevel->foreign_handle, toplevel->xdg_toplevel->title ?: "");
	wlr_foreign_toplevel_handle_v1_set_app_id(
		toplevel->foreign_handle, toplevel->xdg_toplevel->app_id ?: "");

	toplevel->foreign_request_maximize.notify = toplevel_handle_request_maximize;
	wl_signal_add(&toplevel->foreign_handle->events.request_maximize,
		&toplevel->foreign_request_maximize);
	toplevel->foreign_request_minimize.notify = toplevel_handle_request_minimize;
	wl_signal_add(&toplevel->foreign_handle->events.request_minimize,
		&toplevel->foreign_request_minimize);
	toplevel->foreign_request_activate.notify = toplevel_handle_request_activate;
	wl_signal_add(&toplevel->foreign_handle->events.request_activate,
		&toplevel->foreign_request_activate);
	toplevel->foreign_request_fullscreen.notify = toplevel_handle_request_fullscreen;
	wl_signal_add(&toplevel->foreign_handle->events.request_fullscreen,
		&toplevel->foreign_request_fullscreen);
	toplevel->foreign_request_close.notify = toplevel_handle_request_close;
	wl_signal_add(&toplevel->foreign_handle->events.request_close,
		&toplevel->foreign_request_close);

	focus_toplevel(toplevel);

	toplevel->workspace_index = toplevel->server->active_workspace;
	broadcast_toplevel_workspace(toplevel->server, toplevel->foreign_handle, toplevel->workspace_index);
}

// Tells olshell this decoration's toplevel is gone and drops olcore's side
// of the relationship. The wl_resource itself lives on until olshell
// destroys it in response (or its client disconnects); until then,
// decoration->toplevel == NULL marks it as inert.
static void decoration_detach(struct olc_decoration *decoration) {
	if (decoration->toplevel == NULL) {
		return;
	}
	decoration->toplevel->decoration = NULL;
	decoration->toplevel = NULL;
	wlr_scene_node_destroy(&decoration->scene_tree->node);
	zopenlook_decoration_v1_send_closed(decoration->resource);
}

static void xdg_toplevel_unmap(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, unmap);
	if (toplevel == toplevel->server->grabbed_toplevel) {
		reset_cursor_mode(toplevel->server);
	}
	wl_list_remove(&toplevel->link);

	if (toplevel->decoration != NULL) {
		decoration_detach(toplevel->decoration);
	}

	if (toplevel->foreign_handle != NULL) {
		wl_list_remove(&toplevel->foreign_request_maximize.link);
		wl_list_remove(&toplevel->foreign_request_minimize.link);
		wl_list_remove(&toplevel->foreign_request_activate.link);
		wl_list_remove(&toplevel->foreign_request_fullscreen.link);
		wl_list_remove(&toplevel->foreign_request_close.link);
		wlr_foreign_toplevel_handle_v1_destroy(toplevel->foreign_handle);
		toplevel->foreign_handle = NULL;
	}
}

// Sends a fresh configure to a toplevel's decoration whenever the
// toplevel's width has changed since the last one, so the header always
// tracks the window it belongs to. Cheap no-op otherwise.
static void configure_decoration(struct olc_decoration *decoration) {
	struct wlr_box geo = decoration->toplevel->xdg_toplevel->base->geometry;
	uint32_t width = geo.width > 0 ? (uint32_t)geo.width : 0;
	if (decoration->configured && width == decoration->configured_width) {
		return;
	}
	decoration->configured = true;
	decoration->configured_width = width;
	uint32_t serial = wl_display_next_serial(decoration->toplevel->server->wl_display);
	wlr_log(WLR_INFO, "openlook-decoration: configure %ux%u (toplevel geo %dx%d)",
		width, decoration->height, geo.width, geo.height);
	zopenlook_decoration_v1_send_configure(decoration->resource, serial, width, decoration->height);
}

static void xdg_toplevel_commit(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, commit);
	if (toplevel->xdg_toplevel->base->initial_commit) {
		wlr_xdg_toplevel_set_size(toplevel->xdg_toplevel, 0, 0);
	}
	if (toplevel->pending_xdg_decoration != NULL && toplevel->xdg_toplevel->base->initialized) {
		wlr_xdg_toplevel_decoration_v1_set_mode(
			toplevel->pending_xdg_decoration, WLR_XDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE);
		toplevel->pending_xdg_decoration = NULL;
	}
	if (toplevel->decoration != NULL) {
		configure_decoration(toplevel->decoration);
	}
}

static void xdg_toplevel_destroy(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, destroy);
	wl_list_remove(&toplevel->map.link);
	wl_list_remove(&toplevel->unmap.link);
	wl_list_remove(&toplevel->commit.link);
	wl_list_remove(&toplevel->destroy.link);
	wl_list_remove(&toplevel->request_move.link);
	wl_list_remove(&toplevel->request_resize.link);
	wl_list_remove(&toplevel->request_maximize.link);
	wl_list_remove(&toplevel->request_fullscreen.link);
	wl_list_remove(&toplevel->set_title.link);
	wl_list_remove(&toplevel->set_app_id.link);
	free(toplevel);
}

// Sets up an interactive move grab for toplevel, unconditionally -- no
// pointer-focus check, since callers are expected to have already
// established that starting a move on behalf of this toplevel right now is
// legitimate (see begin_interactive's guard for the normal client-initiated
// case, and decoration_handle_move for the header-drag case, which can't
// use that guard since the pointer is focused on the header's own surface,
// not the toplevel's).
//
// held: whether the grab's triggering button is still down, so the grab
// should end when it's released (a drag) -- vs. already gone by the time
// this runs, so the grab should instead end on the next fresh press
// (click-to-arm/click-to-confirm; see decoration_handle_move/resize).
// Deliberately a caller-supplied fact, not inferred from current input
// state here: checking seat->pointer_state.button_count at this point
// turned out to unconditionally read "held", live-tested, regardless of
// which gesture actually happened -- the IPC round-trip for a
// menu-triggered request is evidently not reliably slower than a human
// click's press-to-release gap, so that signal doesn't distinguish the
// two cases it needs to.
static void start_move_grab(struct olc_toplevel *toplevel, bool held) {
	struct olc_server *server = toplevel->server;
	server->grabbed_toplevel = toplevel;
	server->cursor_mode = OLC_CURSOR_MOVE;
	server->cursor_end_on_press = !held;
	server->grab_x = server->cursor->x - toplevel->scene_tree->node.x;
	server->grab_y = server->cursor->y - toplevel->scene_tree->node.y;
}

// Mirrors start_move_grab above, for the resize case -- see its comment.
static void start_resize_grab(struct olc_toplevel *toplevel, uint32_t edges, bool held) {
	struct olc_server *server = toplevel->server;
	server->grabbed_toplevel = toplevel;
	server->cursor_mode = OLC_CURSOR_RESIZE;
	server->cursor_end_on_press = !held;
	struct wlr_box geo_box = toplevel->xdg_toplevel->base->geometry;

	double border_x = (toplevel->scene_tree->node.x + geo_box.x) +
		((edges & WLR_EDGE_RIGHT) ? geo_box.width : 0);
	double border_y = (toplevel->scene_tree->node.y + geo_box.y) +
		((edges & WLR_EDGE_BOTTOM) ? geo_box.height : 0);
	server->grab_x = server->cursor->x - border_x;
	server->grab_y = server->cursor->y - border_y;

	server->grab_geobox = geo_box;
	server->grab_geobox.x += toplevel->scene_tree->node.x;
	server->grab_geobox.y += toplevel->scene_tree->node.y;

	server->resize_edges = edges;
}

static void begin_interactive(struct olc_toplevel *toplevel,
		enum olc_cursor_mode mode, uint32_t edges) {
	struct olc_server *server = toplevel->server;
	struct wlr_surface *focused_surface = server->seat->pointer_state.focused_surface;
	if (toplevel->xdg_toplevel->base->surface !=
			wlr_surface_get_root_surface(focused_surface)) {
		return;
	}

	// This path is always client-initiated (the client's own xdg_toplevel
	// requested it), which per xdg-shell only makes sense in response to a
	// still-held button -- there's no click-to-arm concept here.
	if (mode == OLC_CURSOR_MOVE) {
		start_move_grab(toplevel, true);
	} else {
		start_resize_grab(toplevel, edges, true);
	}
}

static void xdg_toplevel_request_move(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, request_move);
	begin_interactive(toplevel, OLC_CURSOR_MOVE, 0);
}

static void xdg_toplevel_request_resize(struct wl_listener *listener, void *data) {
	struct wlr_xdg_toplevel_resize_event *event = data;
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, request_resize);
	begin_interactive(toplevel, OLC_CURSOR_RESIZE, event->edges);
}

static void xdg_toplevel_request_maximize(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, request_maximize);
	if (toplevel->xdg_toplevel->base->initialized) {
		wlr_xdg_surface_schedule_configure(toplevel->xdg_toplevel->base);
	}
}

static void xdg_toplevel_request_fullscreen(struct wl_listener *listener, void *data) {
	struct olc_toplevel *toplevel = wl_container_of(listener, toplevel, request_fullscreen);
	if (toplevel->xdg_toplevel->base->initialized) {
		wlr_xdg_surface_schedule_configure(toplevel->xdg_toplevel->base);
	}
}

static void server_new_xdg_toplevel(struct wl_listener *listener, void *data) {
	struct olc_server *server = wl_container_of(listener, server, new_xdg_toplevel);
	struct wlr_xdg_toplevel *xdg_toplevel = data;

	struct olc_toplevel *toplevel = calloc(1, sizeof(*toplevel));
	toplevel->server = server;
	toplevel->xdg_toplevel = xdg_toplevel;
	toplevel->scene_tree = wlr_scene_xdg_surface_create(
		server->layers[OLC_LAYER_TOPLEVELS], xdg_toplevel->base);
	toplevel->scene_tree->node.data = toplevel;
	xdg_toplevel->base->data = toplevel->scene_tree;

	toplevel->map.notify = xdg_toplevel_map;
	wl_signal_add(&xdg_toplevel->base->surface->events.map, &toplevel->map);
	toplevel->unmap.notify = xdg_toplevel_unmap;
	wl_signal_add(&xdg_toplevel->base->surface->events.unmap, &toplevel->unmap);
	toplevel->commit.notify = xdg_toplevel_commit;
	wl_signal_add(&xdg_toplevel->base->surface->events.commit, &toplevel->commit);

	toplevel->destroy.notify = xdg_toplevel_destroy;
	wl_signal_add(&xdg_toplevel->events.destroy, &toplevel->destroy);

	toplevel->request_move.notify = xdg_toplevel_request_move;
	wl_signal_add(&xdg_toplevel->events.request_move, &toplevel->request_move);
	toplevel->request_resize.notify = xdg_toplevel_request_resize;
	wl_signal_add(&xdg_toplevel->events.request_resize, &toplevel->request_resize);
	toplevel->request_maximize.notify = xdg_toplevel_request_maximize;
	wl_signal_add(&xdg_toplevel->events.request_maximize, &toplevel->request_maximize);
	toplevel->request_fullscreen.notify = xdg_toplevel_request_fullscreen;
	wl_signal_add(&xdg_toplevel->events.request_fullscreen, &toplevel->request_fullscreen);
	toplevel->set_title.notify = toplevel_handle_set_title;
	wl_signal_add(&xdg_toplevel->events.set_title, &toplevel->set_title);
	toplevel->set_app_id.notify = toplevel_handle_set_app_id;
	wl_signal_add(&xdg_toplevel->events.set_app_id, &toplevel->set_app_id);
}

static void xdg_popup_commit(struct wl_listener *listener, void *data) {
	struct olc_popup *popup = wl_container_of(listener, popup, commit);
	if (popup->xdg_popup->base->initial_commit) {
		wlr_xdg_surface_schedule_configure(popup->xdg_popup->base);
	}
}

static void xdg_popup_destroy(struct wl_listener *listener, void *data) {
	struct olc_popup *popup = wl_container_of(listener, popup, destroy);
	wl_list_remove(&popup->commit.link);
	wl_list_remove(&popup->destroy.link);
	free(popup);
}

static void server_new_xdg_popup(struct wl_listener *listener, void *data) {
	struct wlr_xdg_popup *xdg_popup = data;

	struct olc_popup *popup = calloc(1, sizeof(*popup));
	popup->xdg_popup = xdg_popup;

	struct wlr_xdg_surface *parent = wlr_xdg_surface_try_from_wlr_surface(xdg_popup->parent);
	assert(parent != NULL);
	struct wlr_scene_tree *parent_tree = parent->data;
	xdg_popup->base->data = wlr_scene_xdg_surface_create(parent_tree, xdg_popup->base);

	popup->commit.notify = xdg_popup_commit;
	wl_signal_add(&xdg_popup->base->surface->events.commit, &popup->commit);
	popup->destroy.notify = xdg_popup_destroy;
	wl_signal_add(&xdg_popup->events.destroy, &popup->destroy);
}

// If whatever had keyboard focus is now hidden (wrong workspace, or
// minimized), it shouldn't keep receiving keystrokes for a window the user
// can no longer see. server->toplevels is kept most-recently-focused-first
// (see focus_toplevel), so the first visible entry on the active workspace
// is a reasonable "return to where you left off" choice; if there isn't
// one, just drop focus instead of leaving it on a hidden window. Shared by
// switching the active workspace and moving a toplevel to/off of it --
// both can hide whatever currently has focus.
static void refocus_if_hidden(struct olc_server *server) {
	struct wlr_surface *focused = server->seat->keyboard_state.focused_surface;
	struct wlr_xdg_toplevel *focused_xdg_toplevel =
		focused ? wlr_xdg_toplevel_try_from_wlr_surface(focused) : NULL;
	struct olc_toplevel *focused_toplevel =
		focused_xdg_toplevel ? olc_toplevel_from_xdg_toplevel(focused_xdg_toplevel) : NULL;
	if (focused_toplevel != NULL && toplevel_is_visible(focused_toplevel)) {
		return;
	}
	struct olc_toplevel *toplevel;
	wl_list_for_each(toplevel, &server->toplevels, link) {
		if (toplevel_is_visible(toplevel)) {
			focus_toplevel(toplevel);
			return;
		}
	}
	wlr_seat_keyboard_notify_clear_focus(server->seat);
}

static void workspaces_manager_handle_switch_to(
		struct wl_client *client, struct wl_resource *resource, uint32_t index) {
	struct olc_workspaces_resource *r = wl_resource_get_user_data(resource);
	struct olc_server *server = r->server;

	if (index >= server->workspace_count || index == server->active_workspace) {
		return;
	}
	server->active_workspace = index;

	struct olc_toplevel *toplevel;
	wl_list_for_each(toplevel, &server->toplevels, link) {
		update_toplevel_visibility(toplevel);
	}
	refocus_if_hidden(server);

	struct olc_workspaces_resource *other;
	wl_list_for_each(other, &server->workspace_resources, link) {
		zopenlook_workspaces_manager_v1_send_active_changed(other->resource, server->active_workspace);
	}
}

// ADJUST-clicking a workspace strip segment (olshell) calls this to move
// the currently focused toplevel there -- the only caller for now, but
// deliberately not tied to "focused" in the protocol itself, in case a
// future window-menu "Move to Workspace" item wants to target something
// other than the focused toplevel.
static void workspaces_manager_handle_assign_toplevel(struct wl_client *client,
		struct wl_resource *resource, struct wl_resource *toplevel_handle_resource, uint32_t index) {
	struct olc_workspaces_resource *r = wl_resource_get_user_data(resource);
	struct olc_server *server = r->server;

	if (index >= server->workspace_count) {
		return;
	}
	struct olc_toplevel *toplevel = toplevel_from_foreign_handle_resource(server, toplevel_handle_resource);
	if (toplevel == NULL || toplevel->workspace_index == index) {
		return;
	}
	toplevel->workspace_index = index;
	update_toplevel_visibility(toplevel);
	refocus_if_hidden(server);
	broadcast_toplevel_workspace(server, toplevel->foreign_handle, index);
}

static void workspaces_manager_handle_destroy(struct wl_client *client, struct wl_resource *resource) {
	wl_resource_destroy(resource);
}

static const struct zopenlook_workspaces_manager_v1_interface workspaces_manager_impl = {
	.switch_to = workspaces_manager_handle_switch_to,
	.assign_toplevel = workspaces_manager_handle_assign_toplevel,
	.destroy = workspaces_manager_handle_destroy,
};

static void workspaces_manager_resource_destroy(struct wl_resource *resource) {
	struct olc_workspaces_resource *r = wl_resource_get_user_data(resource);
	wl_list_remove(&r->link);
	free(r);
}

static void workspaces_manager_bind(
		struct wl_client *client, void *data, uint32_t version, uint32_t id) {
	struct olc_server *server = data;

	struct wl_resource *resource =
		wl_resource_create(client, &zopenlook_workspaces_manager_v1_interface, (int)version, id);
	if (resource == NULL) {
		wl_client_post_no_memory(client);
		return;
	}

	struct olc_workspaces_resource *r = calloc(1, sizeof(*r));
	if (r == NULL) {
		wl_resource_destroy(resource);
		wl_client_post_no_memory(client);
		return;
	}
	r->resource = resource;
	r->server = server;
	wl_list_insert(&server->workspace_resources, &r->link);
	wl_resource_set_implementation(resource, &workspaces_manager_impl, r, workspaces_manager_resource_destroy);

	zopenlook_workspaces_manager_v1_send_workspace_count(resource, server->workspace_count);
	zopenlook_workspaces_manager_v1_send_active_changed(resource, server->active_workspace);

	// Catch this client up on already-mapped toplevels' workspace
	// assignments. Only takes effect for toplevels this client can also
	// see via wlr-foreign-toplevel-management; see send_toplevel_workspace().
	struct olc_toplevel *toplevel;
	wl_list_for_each(toplevel, &server->toplevels, link) {
		if (toplevel->foreign_handle != NULL) {
			send_toplevel_workspace(resource, toplevel->foreign_handle, toplevel->workspace_index);
		}
	}
}

// xdg-decoration: unconditionally tell any client that asks for
// server-side decoration that it's getting it, since olshell always draws
// a header for mapped toplevels via openlook-decoration. Clients that never
// ask (or that only support client-side) are left alone; olshell simply
// won't have a matching foreign-toplevel handle to decorate them with
// server-side chrome layered on top of whatever they draw themselves.
//
// wlr_xdg_toplevel_decoration_v1_set_mode() internally schedules an
// xdg_surface configure, which asserts the xdg_surface is already
// "initialized" (ready to receive configures). Clients request a
// decoration object right after creating their xdg_toplevel, generally
// before their first commit -- i.e. before that's true -- so the mode has
// to be applied later; see the pending_xdg_decoration field it gets
// stashed in and xdg_toplevel_commit(), which applies it once safe.
static void xdg_toplevel_decoration_handle_destroy(struct wl_listener *listener, void *data) {
	struct wlr_xdg_toplevel_decoration_v1 *decoration = data;
	struct olc_toplevel *toplevel = olc_toplevel_from_xdg_toplevel(decoration->toplevel);
	if (toplevel != NULL && toplevel->pending_xdg_decoration == decoration) {
		toplevel->pending_xdg_decoration = NULL;
	}
	wl_list_remove(&listener->link);
	free(listener);
}

static void server_new_xdg_toplevel_decoration(struct wl_listener *listener, void *data) {
	struct wlr_xdg_toplevel_decoration_v1 *decoration = data;

	struct wl_listener *destroy_listener = calloc(1, sizeof(*destroy_listener));
	destroy_listener->notify = xdg_toplevel_decoration_handle_destroy;
	wl_signal_add(&decoration->events.destroy, destroy_listener);

	struct olc_toplevel *toplevel = olc_toplevel_from_xdg_toplevel(decoration->toplevel);
	if (toplevel == NULL) {
		return;
	}
	if (toplevel->xdg_toplevel->base->initialized) {
		wlr_xdg_toplevel_decoration_v1_set_mode(decoration, WLR_XDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE);
	} else {
		toplevel->pending_xdg_decoration = decoration;
	}
}

static void decoration_handle_ack_configure(
		struct wl_client *client, struct wl_resource *resource, uint32_t serial) {
	// v1 has nothing to validate beyond the serial round-trip itself; only
	// one configure is ever outstanding at a time.
	(void)client;
	(void)resource;
	(void)serial;
}

static void decoration_handle_destroy(struct wl_client *client, struct wl_resource *resource) {
	(void)client;
	wl_resource_destroy(resource);
}

static void decoration_handle_move(
		struct wl_client *client, struct wl_resource *resource, uint32_t held) {
	(void)client;
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL || decoration->toplevel == NULL) {
		return;
	}
	start_move_grab(decoration->toplevel, held != 0);
}

static void decoration_handle_resize(struct wl_client *client, struct wl_resource *resource,
		uint32_t edges, uint32_t held) {
	(void)client;
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL || decoration->toplevel == NULL) {
		return;
	}
	start_resize_grab(decoration->toplevel, edges, held != 0);
}

// Opposite of focus_toplevel's raise: bottom of the scene stack, and moved
// to the tail of server->toplevels (focus_toplevel inserts new front) so
// list-order-dependent fallbacks (e.g. layer_surface_destroy picking a
// toplevel to refocus) also treat it as least-preferred. Doesn't touch
// keyboard focus -- lowering a window you're still typing into shouldn't
// yank focus away from it.
static void decoration_handle_lower(struct wl_client *client, struct wl_resource *resource) {
	(void)client;
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL || decoration->toplevel == NULL) {
		return;
	}
	struct olc_toplevel *toplevel = decoration->toplevel;
	wlr_scene_node_lower_to_bottom(&toplevel->scene_tree->node);
	wl_list_remove(&toplevel->link);
	wl_list_insert(toplevel->server->toplevels.prev, &toplevel->link);
}

// Toggles rather than taking an explicit target state: olcore doesn't
// report sticky state back to olshell (no protocol event for it), so
// olshell has no way to know which direction to ask for -- same reasoning
// as Full Size toggling maximize based on state only olcore tracks.
static void decoration_handle_toggle_sticky(struct wl_client *client, struct wl_resource *resource) {
	(void)client;
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL || decoration->toplevel == NULL) {
		return;
	}
	struct olc_toplevel *toplevel = decoration->toplevel;
	toplevel->sticky = !toplevel->sticky;
	if (!toplevel->sticky) {
		// Un-sticking commits the toplevel to wherever the user currently
		// is, rather than snapping back to whatever workspace_index
		// happened to be from before it was stuck -- likely long
		// forgotten, and surprising to have the window suddenly vanish to
		// (confirmed live: it is).
		toplevel->workspace_index = toplevel->server->active_workspace;
		broadcast_toplevel_workspace(toplevel->server, toplevel->foreign_handle, toplevel->workspace_index);
	}
	update_toplevel_visibility(toplevel);
	zopenlook_decoration_v1_send_sticky_changed(decoration->resource, toplevel->sticky);
}

// Quit's "same application instance" grouping is by wl_client, not app_id:
// app_id only identifies *which app*, not *which running copy of it* --
// two separately-launched instances of the same app share an app_id but
// are different processes/connections, and only one should be affected.
// wl_client is the precise notion of "the same running instance" and,
// unlike app_id, isn't something a client even gets to report -- it's
// however the wire connection this toplevel's surface actually arrived
// on, visible only to olcore.
static void decoration_handle_quit(struct wl_client *client, struct wl_resource *resource) {
	(void)client;
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL || decoration->toplevel == NULL) {
		return;
	}
	struct wl_client *target_client =
		wl_resource_get_client(decoration->toplevel->xdg_toplevel->base->surface->resource);

	struct olc_toplevel *toplevel;
	wl_list_for_each(toplevel, &decoration->toplevel->server->toplevels, link) {
		struct wl_client *toplevel_client =
			wl_resource_get_client(toplevel->xdg_toplevel->base->surface->resource);
		if (toplevel_client == target_client) {
			wlr_xdg_toplevel_send_close(toplevel->xdg_toplevel);
		}
	}
}

static const struct zopenlook_decoration_v1_interface decoration_impl = {
	.ack_configure = decoration_handle_ack_configure,
	.destroy = decoration_handle_destroy,
	.move = decoration_handle_move,
	.resize = decoration_handle_resize,
	.lower = decoration_handle_lower,
	.quit = decoration_handle_quit,
	.toggle_sticky = decoration_handle_toggle_sticky,
};

static void decoration_teardown(struct olc_decoration *decoration) {
	if (decoration->toplevel != NULL) {
		decoration->toplevel->decoration = NULL;
		wlr_scene_node_destroy(&decoration->scene_tree->node);
	}
	wl_list_remove(&decoration->surface_destroy.link);
	wl_list_remove(&decoration->surface_commit.link);
	free(decoration);
}

// Fires if olshell destroys its wl_surface without destroying the
// decoration object first -- otherwise unreachable since olshell is
// well-behaved, but a client-supplied surface can still go away unexpectedly
// (e.g. client crash), so this keeps olc_decoration from outliving it.
static void decoration_surface_handle_destroy(struct wl_listener *listener, void *data) {
	struct olc_decoration *decoration = wl_container_of(listener, decoration, surface_destroy);
	wl_resource_set_user_data(decoration->resource, NULL);
	decoration_teardown(decoration);
}

// wlr_surface_map() asserts the surface already has a buffer, so it can't
// be called at get_decoration time -- olshell hasn't drawn anything yet.
// Map on the first commit that actually has one instead; harmless to check
// on every later commit too, since wlr_scene_subsurface_tree_create needs
// this regardless of whichever commit happens to be the first with content.
static void decoration_surface_handle_commit(struct wl_listener *listener, void *data) {
	struct olc_decoration *decoration = wl_container_of(listener, decoration, surface_commit);
	if (!decoration->surface->mapped && wlr_surface_has_buffer(decoration->surface)) {
		wlr_surface_map(decoration->surface);
	}
}

static void decoration_resource_destroy(struct wl_resource *resource) {
	struct olc_decoration *decoration = wl_resource_get_user_data(resource);
	if (decoration == NULL) {
		// Already torn down via decoration_surface_handle_destroy.
		return;
	}
	decoration_teardown(decoration);
}

static void decoration_manager_handle_get_decoration(
		struct wl_client *client, struct wl_resource *manager_resource, uint32_t id,
		struct wl_resource *surface_resource, struct wl_resource *toplevel_handle_resource,
		uint32_t height) {
	struct olc_server *server = wl_resource_get_user_data(manager_resource);

	struct olc_toplevel *toplevel =
		toplevel_from_foreign_handle_resource(server, toplevel_handle_resource);
	if (toplevel == NULL) {
		wlr_log(WLR_ERROR, "openlook-decoration: get_decoration: no matching toplevel for handle");
		wl_resource_post_error(manager_resource, ZOPENLOOK_DECORATION_MANAGER_V1_ERROR_NO_SUCH_TOPLEVEL,
			"handle does not name a currently-mapped toplevel");
		return;
	}
	wlr_log(WLR_INFO, "openlook-decoration: get_decoration for toplevel %p, height=%u", (void *)toplevel, height);
	if (toplevel->decoration != NULL) {
		wl_resource_post_error(manager_resource, ZOPENLOOK_DECORATION_MANAGER_V1_ERROR_ALREADY_DECORATED,
			"toplevel already has a header decoration");
		return;
	}

	uint32_t version = wl_resource_get_version(manager_resource);
	struct wl_resource *resource =
		wl_resource_create(client, &zopenlook_decoration_v1_interface, (int)version, id);
	if (resource == NULL) {
		wl_client_post_no_memory(client);
		return;
	}

	struct olc_decoration *decoration = calloc(1, sizeof(*decoration));
	if (decoration == NULL) {
		wl_resource_destroy(resource);
		wl_client_post_no_memory(client);
		return;
	}
	decoration->resource = resource;
	decoration->toplevel = toplevel;
	decoration->surface = wlr_surface_from_resource(surface_resource);
	decoration->height = height;
	toplevel->decoration = decoration;

	// subsurface_tree (not plain scene_surface_create) so any subsurfaces
	// olshell attaches to the header -- the window menu popup -- are
	// composited and hit-tested too. olshell owns both surfaces, so a
	// regular wl_subsurface works fine here; no protocol extension needed.
	//
	// Unlike wlr_scene_surface_create, this helper is written for surfaces
	// that go through a normal role lifecycle and won't composite the
	// surface until it's mapped. We don't give this surface a formal
	// wlr_surface_role (no protocol need for one), so nothing else would
	// ever call wlr_surface_map() on its behalf -- decoration_surface_handle_commit
	// does it the first time the surface actually has a buffer to show
	// (mapping any earlier asserts: a surface can't be mapped without one).
	decoration->scene_tree = wlr_scene_subsurface_tree_create(toplevel->scene_tree, decoration->surface);
	wlr_scene_node_set_position(&decoration->scene_tree->node, 0, -(int)height);

	// Push the toplevel itself down by the header height so the header
	// (positioned just above the toplevel's origin, above) lands where the
	// toplevel used to be instead of off the top edge of the output --
	// olcore doesn't reserve headroom for a header up front, since it has
	// no way to know one is coming until olshell actually asks for it.
	wlr_scene_node_set_position(&toplevel->scene_tree->node,
		toplevel->scene_tree->node.x, toplevel->scene_tree->node.y + (int)height);
	wlr_log(WLR_INFO, "openlook-decoration: toplevel now at (%d,%d), header at (%d,%d) rel",
		toplevel->scene_tree->node.x, toplevel->scene_tree->node.y,
		decoration->scene_tree->node.x, decoration->scene_tree->node.y);

	decoration->surface_destroy.notify = decoration_surface_handle_destroy;
	wl_signal_add(&decoration->surface->events.destroy, &decoration->surface_destroy);
	decoration->surface_commit.notify = decoration_surface_handle_commit;
	wl_signal_add(&decoration->surface->events.commit, &decoration->surface_commit);

	wl_resource_set_implementation(resource, &decoration_impl, decoration, decoration_resource_destroy);

	configure_decoration(decoration);
}

static void decoration_manager_handle_destroy(struct wl_client *client, struct wl_resource *resource) {
	(void)client;
	wl_resource_destroy(resource);
}

static const struct zopenlook_decoration_manager_v1_interface decoration_manager_impl = {
	.get_decoration = decoration_manager_handle_get_decoration,
	.destroy = decoration_manager_handle_destroy,
};

static void decoration_manager_bind(
		struct wl_client *client, void *data, uint32_t version, uint32_t id) {
	struct olc_server *server = data;
	struct wl_resource *resource =
		wl_resource_create(client, &zopenlook_decoration_manager_v1_interface, (int)version, id);
	if (resource == NULL) {
		wl_client_post_no_memory(client);
		return;
	}
	wl_resource_set_implementation(resource, &decoration_manager_impl, server, NULL);
}

int main(int argc, char *argv[]) {
	wlr_log_init(WLR_DEBUG, NULL);
	char *startup_cmd = NULL;

	int c;
	while ((c = getopt(argc, argv, "s:h")) != -1) {
		switch (c) {
		case 's':
			startup_cmd = optarg;
			break;
		default:
			printf("Usage: %s [-s startup command]\n", argv[0]);
			return 0;
		}
	}
	if (optind < argc) {
		printf("Usage: %s [-s startup command]\n", argv[0]);
		return 0;
	}

	struct olc_server server = {0};
	server.wl_display = wl_display_create();

	server.backend = wlr_backend_autocreate(wl_display_get_event_loop(server.wl_display), &server.session);
	if (server.backend == NULL) {
		wlr_log(WLR_ERROR, "failed to create wlr_backend");
		return 1;
	}

	server.renderer = wlr_renderer_autocreate(server.backend);
	if (server.renderer == NULL) {
		wlr_log(WLR_ERROR, "failed to create wlr_renderer");
		return 1;
	}
	wlr_renderer_init_wl_display(server.renderer, server.wl_display);

	server.allocator = wlr_allocator_autocreate(server.backend, server.renderer);
	if (server.allocator == NULL) {
		wlr_log(WLR_ERROR, "failed to create wlr_allocator");
		return 1;
	}

	wlr_compositor_create(server.wl_display, 5, server.renderer);
	wlr_subcompositor_create(server.wl_display);
	wlr_data_device_manager_create(server.wl_display);

	server.output_layout = wlr_output_layout_create(server.wl_display);

	wl_list_init(&server.outputs);
	server.new_output.notify = server_new_output;
	wl_signal_add(&server.backend->events.new_output, &server.new_output);

	server.scene = wlr_scene_create();
	server.scene_layout = wlr_scene_attach_output_layout(server.scene, server.output_layout);
	for (size_t i = 0; i < OLC_NUM_LAYERS; i++) {
		server.layers[i] = wlr_scene_tree_create(&server.scene->tree);
	}

	wl_list_init(&server.toplevels);
	server.xdg_shell = wlr_xdg_shell_create(server.wl_display, 3);
	server.new_xdg_toplevel.notify = server_new_xdg_toplevel;
	wl_signal_add(&server.xdg_shell->events.new_toplevel, &server.new_xdg_toplevel);
	server.new_xdg_popup.notify = server_new_xdg_popup;
	wl_signal_add(&server.xdg_shell->events.new_popup, &server.new_xdg_popup);
	server.foreign_toplevel_manager = wlr_foreign_toplevel_manager_v1_create(server.wl_display);

	server.xdg_decoration_manager = wlr_xdg_decoration_manager_v1_create(server.wl_display);
	server.new_xdg_toplevel_decoration.notify = server_new_xdg_toplevel_decoration;
	wl_signal_add(&server.xdg_decoration_manager->events.new_toplevel_decoration,
		&server.new_xdg_toplevel_decoration);

	server.decoration_manager_global = wl_global_create(server.wl_display,
		&zopenlook_decoration_manager_v1_interface, 1, &server, decoration_manager_bind);

	wl_list_init(&server.layer_surfaces);
	server.layer_shell = wlr_layer_shell_v1_create(server.wl_display, 4);
	server.new_layer_surface.notify = server_new_layer_surface;
	wl_signal_add(&server.layer_shell->events.new_surface, &server.new_layer_surface);

	wl_list_init(&server.workspace_resources);
	server.workspace_count = OLC_WORKSPACE_COUNT;
	server.active_workspace = 0;
	server.workspaces_manager_global = wl_global_create(server.wl_display,
		&zopenlook_workspaces_manager_v1_interface, 1, &server, workspaces_manager_bind);

	server.cursor = wlr_cursor_create();
	wlr_cursor_attach_output_layout(server.cursor, server.output_layout);

	server.cursor_mgr = wlr_xcursor_manager_create(NULL, 24);

	server.cursor_mode = OLC_CURSOR_PASSTHROUGH;
	server.cursor_motion.notify = server_cursor_motion;
	wl_signal_add(&server.cursor->events.motion, &server.cursor_motion);
	server.cursor_motion_absolute.notify = server_cursor_motion_absolute;
	wl_signal_add(&server.cursor->events.motion_absolute, &server.cursor_motion_absolute);
	server.cursor_button.notify = server_cursor_button;
	wl_signal_add(&server.cursor->events.button, &server.cursor_button);
	server.cursor_axis.notify = server_cursor_axis;
	wl_signal_add(&server.cursor->events.axis, &server.cursor_axis);
	server.cursor_frame.notify = server_cursor_frame;
	wl_signal_add(&server.cursor->events.frame, &server.cursor_frame);

	wl_list_init(&server.keyboards);
	server.new_input.notify = server_new_input;
	wl_signal_add(&server.backend->events.new_input, &server.new_input);
	server.seat = wlr_seat_create(server.wl_display, "seat0");
	server.request_cursor.notify = seat_request_cursor;
	wl_signal_add(&server.seat->events.request_set_cursor, &server.request_cursor);
	server.request_set_selection.notify = seat_request_set_selection;
	wl_signal_add(&server.seat->events.request_set_selection, &server.request_set_selection);

	const char *socket = wl_display_add_socket_auto(server.wl_display);
	if (!socket) {
		wlr_backend_destroy(server.backend);
		return 1;
	}

	if (!wlr_backend_start(server.backend)) {
		wlr_backend_destroy(server.backend);
		wl_display_destroy(server.wl_display);
		return 1;
	}

	setenv("WAYLAND_DISPLAY", socket, true);
	wlr_log(WLR_INFO, "olcore running on WAYLAND_DISPLAY=%s", socket);
	if (startup_cmd != NULL) {
		if (fork() == 0) {
			execl("/bin/sh", "/bin/sh", "-c", startup_cmd, (void *)NULL);
		}
	}

	// wlr_scene's automatic damage-triggered frame scheduling isn't
	// sufficient to keep every output repainting on its own here (observed
	// under a nested wayland backend: a layer-shell surface that draws once
	// and then goes idle never gets a second output_frame callback). A
	// small fixed-rate timer keeps rendering correct and simple regardless
	// of backend-specific scheduling quirks; wlr_scene_output_commit() is a
	// cheap no-op when there's no damage, so this costs little.
	server.render_timer = wl_event_loop_add_timer(
		wl_display_get_event_loop(server.wl_display), render_timer_handle, &server);
	wl_event_source_timer_update(server.render_timer, OLC_RENDER_INTERVAL_MS);

	wl_display_run(server.wl_display);

	wl_display_destroy_clients(server.wl_display);
	wl_event_source_remove(server.render_timer);
	wlr_scene_node_destroy(&server.scene->tree.node);
	wlr_xcursor_manager_destroy(server.cursor_mgr);
	wlr_cursor_destroy(server.cursor);
	wlr_allocator_destroy(server.allocator);
	wlr_renderer_destroy(server.renderer);
	wlr_backend_destroy(server.backend);
	wl_display_destroy(server.wl_display);
	return 0;
}
