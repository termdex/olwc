// Parser for an olwm-compatible root menu config (traditionally
// `~/.openwin-menu`). This is a deliberately lenient subset of the
// original format, not a full reimplementation:
//
//   ! a comment line                  -- ignored
//   "Label" TITLE                     -- sets the menu's title (first wins)
//   "Label" exec <command...>         -- leaf item; <command> runs via `sh -c`
//   "Label" MENU                      -- opens a submenu; following lines
//       ...                              are its items, until a line that
//   END [MENU]                        -- is exactly END (optionally "END MENU")
//   "Label" EXIT                      -- leaf item; terminates the compositor
//                                         session (see MenuNode::Exit)
//   "Label" REREAD_MENU_FILE          -- leaf item; reloads the root menu
//                                         from disk (see MenuNode::ReloadMenu)
//   "Label" INCLUDE <file>            -- submenu whose items come from <file>
//                                         (resolved relative to this file)
//   "Label" APPMENU                   -- submenu auto-populated from the
//                                         system's .desktop files, grouped
//                                         by category (see appmenu.rs)
//   "Label" DEFAULT <action>          -- any of the above, prefixed with
//                                         DEFAULT, marks it the menu's
//                                         default item (pre-highlighted on
//                                         open)
//
// Unrecognized directives (PIN, WINMENU, and friends from the original
// olwm format) are skipped with a warning rather than treated as a parse
// error, since real-world menu files may use them and a missing feature
// shouldn't take down the whole menu.

use std::path::{Path, PathBuf};

/// An INCLUDE chain deeper than this is treated as a cycle and cut off
/// -- a real cycle (A includes B includes A) would otherwise recurse
/// until the stack overflows, and no legitimate menu nests this deep.
const MAX_INCLUDE_DEPTH: usize = 16;

#[derive(Debug, Clone)]
pub enum MenuNode {
    Item { label: String, command: String },
    // A nested submenu, from a `MENU`/`END` block or `INCLUDE <file>`.
    // `default` is the index into `items` of the submenu's own DEFAULT
    // item, if any (see Menu::default's doc comment).
    Submenu { label: String, items: Vec<MenuNode>, default: Option<usize> },
    // Authentic OPEN LOOK: olwm's own default root ("Workspace") menu is
    // exactly a Programs submenu and this, "Exit..." (confirmed from
    // source, clients/olwm/openwin-menu in the historical XView/olwm tree
    // -- see docs/OPENLOOK-REFERENCE.md for where that source lives).
    // Selecting it opens a confirmation Notice (see Notice's own doc
    // comment in shell/src/main.rs), which is what actually asks olcore
    // to terminate the whole compositor session
    // (openlook-session-unstable-v1's exit request) if confirmed -- the
    // Wayland-native equivalent of Exit terminating olwm itself, which
    // (as the X session's leader) normally returned to a display
    // manager. Distinct from Item since it isn't a shell command olshell
    // spawns; olcore does the actual work.
    Exit { label: String },
    // Authentic: REREAD_MENU_FILE is a real user-facing olwm menu-file
    // token (usermenu.c's token table: "REREAD_MENU_FILE",
    // ReReadUserMenuFunc, ServiceToken -- alongside EXIT above), not
    // just one of the hardcoded default menu's own buttons. Reloads
    // olwc's root menu from disk without restarting olcore -- unlike
    // real olwm's own "Restart WM" (a full WM re-exec that survives
    // only because X11 lets client windows be reparented back to the
    // root window first), which has no Wayland equivalent at all (a
    // client's connection belongs to the compositor's own wl_display,
    // so restarting olcore would drop every client outright) and so
    // isn't offered here.
    ReloadMenu { label: String },
    // A placeholder for a submenu auto-populated from the system's
    // `.desktop` application files, grouped by freedesktop category --
    // olwc's modern stand-in for the hand-maintained `Programs` submenu
    // (and the closest thing to real olvwm's dynamic `DIRMENU`, which
    // likewise generated a submenu's contents rather than spelling them
    // out). Present only between parsing and `Menu::expand_appmenus`,
    // which replaces every one with the `Submenu` that `appmenu::
    // generate` builds by scanning the filesystem; nothing downstream
    // of `load_default` ever sees this variant.
    AppMenu { label: String },
}

impl MenuNode {
    pub fn label(&self) -> &str {
        match self {
            MenuNode::Item { label, .. } => label,
            MenuNode::Submenu { label, .. } => label,
            MenuNode::Exit { label } => label,
            MenuNode::ReloadMenu { label } => label,
            MenuNode::AppMenu { label } => label,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub title: Option<String>,
    pub items: Vec<MenuNode>,
    /// Index into `items` of the item marked `DEFAULT`, if any. Real
    /// olwm activates the default item on a plain click of the menu
    /// button without traversing into the menu, and rings it. olwc's
    /// root menu has no such button (it opens from a background
    /// right-click), so instead the default row is pre-highlighted and
    /// ringed when the menu opens -- a quick release without moving
    /// activates it.
    pub default: Option<usize>,
}

impl Menu {
    fn default_menu() -> Menu {
        // konsole, not xterm: olwc has no Xwayland support, so an X11 app
        // like xterm can never launch here regardless of whether it's
        // installed -- it would just fail silently inside the spawned
        // shell, with nothing to show for it. This is only a fallback for
        // when no .openwin-menu config exists; real setups should have one
        // pointing at whatever terminal is actually installed.
        //
        // --separate: without it, KDE's single-instance activation can
        // hand off to an *existing* Konsole process (e.g. one on a host
        // desktop olcore is nested inside for testing) instead of opening
        // a window here -- but a window manager's "launch a terminal"
        // action should always open a fresh window regardless of
        // environment, so this is the right default even outside testing.
        Menu {
            // Real olwm's own default root menu file (clients/olwm/
            // openwin-menu in the historical XView/olwm tree) is titled
            // "Workspace" (`"Workspace" TITLE`) and is, in essence, a
            // Programs submenu plus "Exit..." -- its actual first line is
            // `"Programs" DEFAULT INCLUDE openwin-menu-programs`. This
            // fallback mirrors that shape: a DEFAULT Programs submenu
            // (APPMENU rather than an INCLUDE of a file olwc doesn't
            // ship -- see MenuNode::AppMenu and load_default, which
            // expands it) and Exit..., with a Terminal shortcut and
            // Reread Menu File kept for the zero-config case.
            title: Some("Workspace".to_string()),
            items: vec![
                MenuNode::AppMenu { label: "Programs".into() },
                MenuNode::Item { label: "Terminal".into(), command: "konsole --separate".into() },
                // Real olwm's own hardcoded default menu (usermenu.c)
                // pairs Restart WM with this -- Restart WM itself has no
                // Wayland equivalent (see MenuNode::ReloadMenu's doc
                // comment), but this half is genuinely useful and has no
                // such obstacle, so it's kept.
                MenuNode::ReloadMenu { label: "Reread Menu File".into() },
                // Matches authentic olwm's own default root menu, which
                // pairs a Programs submenu with exactly this -- see
                // MenuNode::Exit's doc comment.
                MenuNode::Exit { label: "Exit...".into() },
            ],
            // Programs, matching the real file's `"Programs" DEFAULT`.
            default: Some(0),
        }
    }

    /// Loads `$OLWC_MENU` if set, else `~/.openwin-menu`, falling back to a
    /// small built-in default if neither exists or parsing fails. Every
    /// path here ends with `expand_appmenus` -- the built-in default has
    /// an `APPMENU` node of its own (see `default_menu`), not just the
    /// parsed-from-file case.
    pub fn load_default() -> Menu {
        let mut menu = Self::load_raw();
        menu.expand_appmenus();
        menu
    }

    fn load_raw() -> Menu {
        let path = std::env::var_os("OLWC_MENU").map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".openwin-menu"))
        });

        let Some(path) = path.filter(|p| p.exists()) else {
            log::info!("root menu: no config found, using built-in default");
            return Menu::default_menu();
        };

        match Menu::parse_file(&path) {
            Ok(menu) => {
                log::info!(
                    "root menu: loaded {} top-level item(s) from {}",
                    menu.items.len(),
                    path.display()
                );
                menu
            }
            Err(e) => {
                log::warn!("root menu: failed to parse {}: {e} -- using built-in default", path.display());
                Menu::default_menu()
            }
        }
    }

    /// Replaces every `MenuNode::AppMenu` in the tree (top level and
    /// inside any `Submenu`, including ones pulled in by `INCLUDE`) with
    /// the category-grouped `Submenu` that `appmenu::generate` builds
    /// from the system's `.desktop` files. Done here, after parsing,
    /// rather than in the parser itself so the parser stays a pure
    /// text-to-tree transform with no filesystem scan -- and so a
    /// "Reread Menu File" (which re-runs `load_default`) picks up
    /// newly-installed applications for free.
    fn expand_appmenus(&mut self) {
        fn walk(items: &mut [MenuNode]) {
            for node in items.iter_mut() {
                match node {
                    MenuNode::AppMenu { label } => {
                        *node = crate::appmenu::generate(std::mem::take(label));
                    }
                    MenuNode::Submenu { items, .. } => walk(items),
                    _ => {}
                }
            }
        }
        walk(&mut self.items);
    }

    pub fn parse_file(path: &Path) -> Result<Menu, String> {
        let contents = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Ok(Menu::parse_inner(&contents, path.parent(), 0))
    }

    /// Parses menu text with no base directory -- a relative `INCLUDE`
    /// can't be resolved without one, so it just warns and is skipped.
    /// Only the tests need this entry point; real use always has a file
    /// path and goes through `parse_file`.
    #[cfg(test)]
    pub fn parse(contents: &str) -> Result<Menu, String> {
        Ok(Menu::parse_inner(contents, None, 0))
    }

    fn parse_inner(contents: &str, base_dir: Option<&Path>, depth: usize) -> Menu {
        let mut lines = contents.lines().peekable();
        let mut title = None;
        let mut default = None;
        let items = parse_items(&mut lines, base_dir, depth, &mut title, &mut default);
        Menu { title, items, default }
    }

    /// Reads and parses an `INCLUDE`d file. A read error yields an empty
    /// menu (warned) rather than propagating -- one bad INCLUDE
    /// shouldn't take down the whole root menu, same leniency the rest
    /// of the parser has.
    fn parse_include(path: &Path, depth: usize) -> Menu {
        match std::fs::read_to_string(path) {
            Ok(contents) => Menu::parse_inner(&contents, path.parent(), depth),
            Err(e) => {
                log::warn!("root menu: can't read INCLUDEd {}: {e}", path.display());
                Menu::default()
            }
        }
    }
}

/// Resolves an `INCLUDE <file>` argument: an absolute path used
/// directly, otherwise relative to the including file's own directory,
/// otherwise `$HOME/<file>`. Real olwm has a longer search path
/// (`$OPENWINHOME/lib` and friends); olwc has no equivalent of those
/// locations, so this is the practical subset.
fn resolve_include_path(file: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let p = Path::new(file);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    if let Some(dir) = base_dir {
        let candidate = dir.join(file);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join(file);
    candidate.exists().then_some(candidate)
}

fn parse_items<'a, I: Iterator<Item = &'a str>>(
    lines: &mut std::iter::Peekable<I>,
    base_dir: Option<&Path>,
    depth: usize,
    title: &mut Option<String>,
    default: &mut Option<usize>,
) -> Vec<MenuNode> {
    let mut items = Vec::new();
    while let Some(raw_line) = lines.next() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('!') {
            continue;
        }
        if line == "END" || line == "END MENU" {
            return items;
        }
        let Some((label, rest)) = parse_label(line) else {
            log::warn!("root menu: skipping unparseable line: {raw_line:?}");
            continue;
        };
        // DEFAULT is a leading modifier on any other action, not an
        // action itself -- `"Programs" DEFAULT INCLUDE ...`. DEFAULT
        // with nothing after it falls through to the warn branch below.
        let (is_default, rest) = match rest.trim().strip_prefix("DEFAULT ") {
            Some(after) => (true, after.trim()),
            None => (false, rest.trim()),
        };
        let before_len = items.len();
        if rest == "TITLE" {
            *title = Some(label);
        } else if rest == "MENU" {
            let mut child_title = None;
            let mut child_default = None;
            let children =
                parse_items(lines, base_dir, depth, &mut child_title, &mut child_default);
            items.push(MenuNode::Submenu { label, items: children, default: child_default });
        } else if let Some(file) = rest.strip_prefix("INCLUDE ") {
            let sub = if depth >= MAX_INCLUDE_DEPTH {
                log::warn!("root menu: INCLUDE nesting too deep near {file:?} -- possible cycle, stopping");
                Menu::default()
            } else if let Some(path) = resolve_include_path(file.trim(), base_dir) {
                Menu::parse_include(&path, depth + 1)
            } else {
                log::warn!("root menu: INCLUDE {file:?} not found");
                Menu::default()
            };
            items.push(MenuNode::Submenu { label, items: sub.items, default: sub.default });
        } else if let Some(command) = rest.strip_prefix("exec ") {
            items.push(MenuNode::Item { label, command: command.trim().to_string() });
        } else if rest == "EXIT" {
            items.push(MenuNode::Exit { label });
        } else if rest == "REREAD_MENU_FILE" {
            items.push(MenuNode::ReloadMenu { label });
        } else if rest == "APPMENU" {
            items.push(MenuNode::AppMenu { label });
        } else {
            log::warn!("root menu: skipping item {label:?} with unsupported action {rest:?}");
        }
        if is_default && items.len() == before_len + 1 {
            *default = Some(before_len);
        }
    }
    items
}

/// Parses a leading `"quoted label"` from a line (`\"` escapes a literal
/// quote), returning the label and the remainder of the line after it.
fn parse_label(line: &str) -> Option<(String, &str)> {
    let line = line.strip_prefix('"')?;
    let mut label = String::new();
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, next)) = chars.next() {
                    label.push(next);
                }
            }
            '"' => return Some((label, &line[i + 1..])),
            _ => label.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_items() {
        let menu = Menu::parse(
            r#"
                ! a comment
                "Root Menu" TITLE
                "Terminal" exec xterm
                "Files" exec nautilus
            "#,
        )
        .unwrap();
        assert_eq!(menu.title.as_deref(), Some("Root Menu"));
        assert_eq!(menu.items.len(), 2);
        assert_eq!(menu.items[0].label(), "Terminal");
        let MenuNode::Item { command, .. } = &menu.items[0] else { panic!("expected item") };
        assert_eq!(command, "xterm");
    }

    #[test]
    fn parses_nested_submenu() {
        let menu = Menu::parse(
            r#"
                "Programs" MENU
                    "Emacs" exec emacs
                    "XTerm" exec xterm
                END MENU
                "Exit" exec "true"
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 2);
        let MenuNode::Submenu { label, items, .. } = &menu.items[0] else { panic!("expected submenu") };
        assert_eq!(label, "Programs");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].label(), "XTerm");
    }

    #[test]
    fn parses_exit() {
        let menu = Menu::parse(
            r#"
                "Terminal" exec xterm
                "Exit..." EXIT
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 2);
        assert_eq!(menu.items[1].label(), "Exit...");
        assert!(matches!(&menu.items[1], MenuNode::Exit { .. }));
    }

    #[test]
    fn parses_reread_menu_file() {
        let menu = Menu::parse(
            r#"
                "Reread Menu File" REREAD_MENU_FILE
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label(), "Reread Menu File");
        assert!(matches!(&menu.items[0], MenuNode::ReloadMenu { .. }));
    }

    #[test]
    fn skips_unsupported_directives() {
        let menu = Menu::parse(
            r#"
                "Weird" WINMENU
                "Bare" DEFAULT
                "Terminal" exec xterm
            "#,
        )
        .unwrap();
        // WINMENU isn't supported; "Bare" DEFAULT has no action after
        // the modifier so there's nothing to mark default -- both
        // skipped, leaving just the one real item.
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label(), "Terminal");
        assert_eq!(menu.default, None);
    }

    #[test]
    fn handles_escaped_quote_in_label() {
        let menu = Menu::parse(r#""Say \"Hi\"" exec echo"#).unwrap();
        assert_eq!(menu.items[0].label(), "Say \"Hi\"");
    }

    #[test]
    fn builtin_default_has_expandable_programs() {
        let mut menu = Menu::default_menu();
        // First item, and the menu's DEFAULT -- matching the real
        // openwin-menu file's `"Programs" DEFAULT ...` first line.
        assert_eq!(menu.default, Some(0));
        assert!(matches!(&menu.items[0], MenuNode::AppMenu { label } if label == "Programs"));
        // expand_appmenus (which load_default always runs) turns it into
        // a real submenu; index 0 and the DEFAULT stay put.
        menu.expand_appmenus();
        assert!(matches!(&menu.items[0], MenuNode::Submenu { label, .. } if label == "Programs"));
        assert_eq!(menu.default, Some(0));
    }

    #[test]
    fn parses_appmenu() {
        // The parser only produces the marker node -- the actual
        // .desktop scan happens later, in Menu::expand_appmenus, and is
        // covered by appmenu.rs's own tests.
        let menu = Menu::parse(
            r#"
                "Programs" DEFAULT APPMENU
                "Exit" EXIT
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 2);
        assert!(matches!(&menu.items[0], MenuNode::AppMenu { .. }));
        assert_eq!(menu.items[0].label(), "Programs");
        assert_eq!(menu.default, Some(0));
    }

    #[test]
    fn parses_default_modifier() {
        let menu = Menu::parse(
            r#"
                "First" exec a
                "Second" DEFAULT exec b
                "Third" exec c
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 3);
        assert_eq!(menu.default, Some(1));

        // DEFAULT also works on a submenu, tracked per level.
        let menu = Menu::parse(
            r#"
                "Progs" DEFAULT MENU
                    "X" exec x
                    "Y" DEFAULT exec y
                END
            "#,
        )
        .unwrap();
        assert_eq!(menu.default, Some(0));
        let MenuNode::Submenu { default, .. } = &menu.items[0] else { panic!("expected submenu") };
        assert_eq!(*default, Some(1));
    }

    #[test]
    fn parses_include() {
        let dir = std::env::temp_dir().join(format!("olwc-menu-include-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sub"), "\"Emacs\" exec emacs\n\"Vim\" DEFAULT exec vim\n").unwrap();
        std::fs::write(dir.join("root"), "\"Workspace\" TITLE\n\"Programs\" INCLUDE sub\n\"Exit\" EXIT\n").unwrap();

        let menu = Menu::parse_file(&dir.join("root")).unwrap();
        assert_eq!(menu.title.as_deref(), Some("Workspace"));
        assert_eq!(menu.items.len(), 2);
        let MenuNode::Submenu { label, items, default } = &menu.items[0] else { panic!("expected submenu") };
        assert_eq!(label, "Programs");
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].label(), "Vim");
        assert_eq!(*default, Some(1)); // the INCLUDEd file's own DEFAULT carries through
    }

    #[test]
    fn include_cycle_terminates() {
        let dir = std::env::temp_dir().join(format!("olwc-menu-cycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), "\"B\" INCLUDE b\n").unwrap();
        std::fs::write(dir.join("b"), "\"A\" INCLUDE a\n").unwrap();

        // Just needs to return rather than stack-overflow.
        let menu = Menu::parse_file(&dir.join("a")).unwrap();
        assert_eq!(menu.items.len(), 1);
    }
}
