project(
  'olcore',
  'c',
  version: '0.1.0',
  license: 'MIT',
  meson_version: '>=0.59.0',
  default_options: [
    'c_std=gnu11',
    'warning_level=2',
    'werror=false',
  ],
)

add_project_arguments('-DWLR_USE_UNSTABLE', language: 'c')

cc = meson.get_compiler('c')

# Arch (and some other distros) ship wlroots split by SONAME/API version
# rather than a single unversioned package, so probe newest-first.
wlroots = dependency('', required: false)
foreach name : ['wlroots-0.20', 'wlroots-0.19', 'wlroots-0.18']
  if not wlroots.found()
    wlroots = dependency(name, required: false)
  endif
endforeach
if not wlroots.found()
  wlroots = dependency('wlroots', version: '>=0.18.0')
endif

wayland_server = dependency('wayland-server')
wayland_scanner_dep = dependency('wayland-scanner', native: true)
wlr_protocols = dependency('wlr-protocols')
xkbcommon = dependency('xkbcommon')
libinput = dependency('libinput')
pixman = dependency('pixman-1')
libm = cc.find_library('m', required: false)

wayland_scanner = find_program(
  wayland_scanner_dep.get_variable(pkgconfig: 'wayland_scanner'),
  native: true,
)

# wlr_layer_shell_v1.h relatively #includes this generated header; wlroots
# expects consuming compositors to generate it themselves from wlr-protocols.
wlr_layer_shell_server_header = custom_target(
  'wlr-layer-shell-unstable-v1-protocol.h',
  input: wlr_protocols.get_variable(pkgconfig: 'pkgdatadir') / 'unstable/wlr-layer-shell-unstable-v1.xml',
  output: 'wlr-layer-shell-unstable-v1-protocol.h',
  command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
)

# openlook-workspaces is our own protocol (protocol/ at the repo root), so
# unlike wlr-layer-shell nothing else provides its compiled wl_interface
# objects -- we generate and build the private-code ourselves.
openlook_workspaces_xml = files('../protocol/openlook-workspaces-unstable-v1.xml')

openlook_workspaces_header = custom_target(
  'openlook-workspaces-unstable-v1-protocol.h',
  input: openlook_workspaces_xml,
  output: 'openlook-workspaces-unstable-v1-protocol.h',
  command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
)

openlook_workspaces_code = custom_target(
  'openlook-workspaces-unstable-v1-protocol.c',
  input: openlook_workspaces_xml,
  output: 'openlook-workspaces-unstable-v1-protocol.c',
  command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
)

# openlook-workspaces's toplevel_workspace event carries a
# zwlr_foreign_toplevel_handle_v1 object argument. wlroots implements that
# protocol's C API but doesn't export its wl_interface symbol, so we need
# our own compiled copy purely to satisfy the linker -- olcore doesn't
# implement wlr-foreign-toplevel-management itself (yet).
wlr_protocols_datadir = wlr_protocols.get_variable(pkgconfig: 'pkgdatadir')
wlr_foreign_toplevel_xml = wlr_protocols_datadir / 'unstable/wlr-foreign-toplevel-management-unstable-v1.xml'

wlr_foreign_toplevel_code = custom_target(
  'wlr-foreign-toplevel-management-unstable-v1-protocol.c',
  input: wlr_foreign_toplevel_xml,
  output: 'wlr-foreign-toplevel-management-unstable-v1-protocol.c',
  command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
)

# openlook-decoration is likewise our own protocol; its get_decoration
# request references zwlr_foreign_toplevel_handle_v1, whose wl_interface
# symbol wlr_foreign_toplevel_code (above) already provides.
openlook_decoration_xml = files('../protocol/openlook-decoration-unstable-v1.xml')

openlook_decoration_header = custom_target(
  'openlook-decoration-unstable-v1-protocol.h',
  input: openlook_decoration_xml,
  output: 'openlook-decoration-unstable-v1-protocol.h',
  command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
)

openlook_decoration_code = custom_target(
  'openlook-decoration-unstable-v1-protocol.c',
  input: openlook_decoration_xml,
  output: 'openlook-decoration-unstable-v1-protocol.c',
  command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
)

# openlook-session is likewise our own protocol; unlike workspaces/
# decoration it references no foreign objects at all, just a bare exit
# request, so it needs nothing beyond its own generated header/code.
openlook_session_xml = files('../protocol/openlook-session-unstable-v1.xml')

openlook_session_header = custom_target(
  'openlook-session-unstable-v1-protocol.h',
  input: openlook_session_xml,
  output: 'openlook-session-unstable-v1-protocol.h',
  command: [wayland_scanner, 'server-header', '@INPUT@', '@OUTPUT@'],
)

openlook_session_code = custom_target(
  'openlook-session-unstable-v1-protocol.c',
  input: openlook_session_xml,
  output: 'openlook-session-unstable-v1-protocol.c',
  command: [wayland_scanner, 'private-code', '@INPUT@', '@OUTPUT@'],
)

olcore_deps = [
  wayland_server,
  wlroots,
  xkbcommon,
  libinput,
  pixman,
  libm,
]

olcore_sources = files(
  'main.c',
)

executable(
  'olcore',
  olcore_sources,
  wlr_layer_shell_server_header,
  openlook_workspaces_header,
  openlook_workspaces_code,
  openlook_decoration_header,
  openlook_decoration_code,
  openlook_session_header,
  openlook_session_code,
  wlr_foreign_toplevel_code,
  dependencies: olcore_deps,
  install: true,
)
