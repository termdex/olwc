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
//
// Unrecognized directives (DEFAULT, PIN, and friends from the original
// olwm format) are skipped with a warning rather than treated as a parse
// error, since real-world menu files may use them and a missing feature
// shouldn't take down the whole menu.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum MenuNode {
    Item { label: String, command: String },
    // items: parsed but not yet read -- olshell doesn't open nested popups
    // on hover yet, see MenuPopup's doc comment.
    #[allow(dead_code)]
    Submenu { label: String, items: Vec<MenuNode> },
    // Authentic OPEN LOOK: olwm's own default root ("Workspace") menu is
    // exactly a Programs submenu and this, "Exit..." (confirmed from
    // source, clients/olwm/openwin-menu in the historical XView/olwm tree
    // -- see docs/OPENLOOK-REFERENCE.md for where that source lives).
    // Selecting it asks olcore to terminate the whole compositor session
    // (openlook-session-unstable-v1's exit request) -- the Wayland-native
    // equivalent of Exit terminating olwm itself, which (as the X
    // session's leader) normally returned to a display manager. Distinct
    // from Item since it isn't a shell command olshell spawns; olcore
    // does the actual work.
    Exit { label: String },
}

impl MenuNode {
    pub fn label(&self) -> &str {
        match self {
            MenuNode::Item { label, .. } => label,
            MenuNode::Submenu { label, .. } => label,
            MenuNode::Exit { label } => label,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Menu {
    pub title: Option<String>,
    pub items: Vec<MenuNode>,
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
            title: Some("olwc".to_string()),
            items: vec![
                MenuNode::Item { label: "Terminal".into(), command: "konsole --separate".into() },
                MenuNode::Item { label: "Refresh".into(), command: "true".into() },
                // Matches authentic olwm's own default root menu, which
                // pairs a Programs submenu with exactly this -- see
                // MenuNode::Exit's doc comment.
                MenuNode::Exit { label: "Exit...".into() },
            ],
        }
    }

    /// Loads `$OLWC_MENU` if set, else `~/.openwin-menu`, falling back to a
    /// small built-in default if neither exists or parsing fails.
    pub fn load_default() -> Menu {
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

    pub fn parse_file(path: &Path) -> Result<Menu, String> {
        let contents = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Menu::parse(&contents)
    }

    pub fn parse(contents: &str) -> Result<Menu, String> {
        let mut lines = contents.lines().peekable();
        let mut title = None;
        let items = parse_items(&mut lines, &mut title);
        Ok(Menu { title, items })
    }
}

fn parse_items<'a, I: Iterator<Item = &'a str>>(
    lines: &mut std::iter::Peekable<I>,
    title: &mut Option<String>,
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
        let rest = rest.trim();
        if rest == "TITLE" {
            *title = Some(label);
        } else if rest == "MENU" {
            let children = parse_items(lines, title);
            items.push(MenuNode::Submenu { label, items: children });
        } else if let Some(command) = rest.strip_prefix("exec ") {
            items.push(MenuNode::Item { label, command: command.trim().to_string() });
        } else if rest == "EXIT" {
            items.push(MenuNode::Exit { label });
        } else {
            log::warn!("root menu: skipping item {label:?} with unsupported action {rest:?}");
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
        let MenuNode::Submenu { label, items } = &menu.items[0] else { panic!("expected submenu") };
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
    fn skips_unsupported_directives() {
        let menu = Menu::parse(
            r#"
                "Weird" DEFAULT
                "Terminal" exec xterm
            "#,
        )
        .unwrap();
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0].label(), "Terminal");
    }

    #[test]
    fn handles_escaped_quote_in_label() {
        let menu = Menu::parse(r#""Say \"Hi\"" exec echo"#).unwrap();
        assert_eq!(menu.items[0].label(), "Say \"Hi\"");
    }
}
