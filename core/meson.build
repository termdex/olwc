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
  dependencies: olcore_deps,
  install: true,
)
