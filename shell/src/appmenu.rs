// Builds a category-grouped submenu of the system's installed
// applications by scanning XDG `.desktop` files -- olwc's modern
// stand-in for a hand-maintained `Programs` submenu, reached from a
// `"Programs" APPMENU` line in `.openwin-menu` (see menu.rs). It's the
// same idea as real olvwm's dynamic `DIRMENU` (virtual.c's
// `GenDirMenuFunc` scanned a directory and turned each file into an
// `exec` item); the `.desktop`/XDG-category machinery is the part
// that's new, since a 1993 window manager had nothing resembling it.
//
// A deliberately lenient subset of the freedesktop Desktop Entry and
// Menu specs, in the same spirit as icon_theme.rs and menu.rs: enough
// to produce a useful, correctly-grouped menu on a normal desktop
// install, not a spec-complete implementation. In particular it ignores
// OnlyShowIn/NotShowIn (olwc isn't a registered desktop environment, so
// honoring them would just hide half the menu), localized `Name[xx]`
// keys, D-Bus activation, and the Menu spec's `.directory` files and
// nested-category rules.

use crate::menu::MenuNode;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Category buckets. Each entry is (menu label, the freedesktop
/// `Categories` values that map to it). This collapses the spec's ~40
/// registered categories down to a small, recognizable set -- a menu
/// with a separate "TerminalEmulator" section would be strange.
///
/// Order matters: an application tagged with several main categories
/// lands in the *first* bucket here that matches, so the more specific
/// buckets come first and the broad catch-alls (Utility, System) last
/// -- e.g. an IDE tagged `Development;Utility;TextEditor` belongs under
/// Development, not Accessories. The menu itself shows the sections
/// alphabetically regardless (see `generate_from`), so this order is
/// purely the tie-break.
const CATEGORIES: &[(&str, &[&str])] = &[
    ("Development", &["Development"]),
    ("Graphics", &["Graphics"]),
    ("Multimedia", &["AudioVideo", "Audio", "Video"]),
    ("Games", &["Game"]),
    ("Office", &["Office"]),
    ("Science", &["Science"]),
    ("Education", &["Education"]),
    ("Internet", &["Network"]),
    ("System", &["System", "Settings"]),
    ("Accessories", &["Utility"]),
];
const OTHER: &str = "Other";

/// Scans the real XDG application directories and returns the
/// `MenuNode::Submenu` a `MenuNode::AppMenu { label }` expands to (see
/// menu.rs's `Menu::expand_appmenus`). The submenu holds one child
/// submenu per non-empty category; if nothing is found at all it's
/// simply empty, and clicking the row does nothing -- the same outcome
/// real olvwm's `DIRMENU` gives for an empty directory.
pub fn generate(label: String) -> MenuNode {
    generate_from(&crate::icon_theme::application_dirs(), label)
}

fn generate_from(dirs: &[PathBuf], label: String) -> MenuNode {
    // Desktop-file IDs already seen. XDG precedence: a file in an
    // earlier directory shadows a same-named one later, so the first
    // win is kept and the rest skipped.
    let mut seen: HashSet<String> = HashSet::new();
    // One Vec of (name, exec) per CATEGORIES entry, plus a trailing one for OTHER.
    let mut buckets: Vec<Vec<(String, String)>> = vec![Vec::new(); CATEGORIES.len() + 1];

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(app) = DesktopApp::parse(&contents) else {
                continue;
            };
            buckets[bucket_for(&app.categories)].push((app.name, app.exec));
        }
    }

    let mut sections: Vec<MenuNode> = Vec::new();
    for (i, (cat_label, _)) in CATEGORIES.iter().enumerate() {
        push_section(&mut sections, cat_label, std::mem::take(&mut buckets[i]));
    }
    // Show the sections alphabetically -- CATEGORIES' own order is just
    // the multi-category tie-break, not a display preference.
    sections.sort_by_key(|s| s.label().to_lowercase());
    // "Other" always sits at the end rather than wherever "O" falls.
    push_section(&mut sections, OTHER, std::mem::take(&mut buckets[CATEGORIES.len()]));

    MenuNode::Submenu { label, items: sections, default: None }
}

/// Index into `buckets` (`CATEGORIES.len()` meaning OTHER) for an app
/// with these `Categories` values.
fn bucket_for(categories: &[String]) -> usize {
    for (i, (_, keys)) in CATEGORIES.iter().enumerate() {
        if categories.iter().any(|c| keys.contains(&c.as_str())) {
            return i;
        }
    }
    CATEGORIES.len()
}

/// Appends one category child submenu to `sections`, skipping it
/// entirely when empty. Apps are sorted by name (case-insensitive, with
/// the exact ordering as a stable tie-break) and de-duplicated by name
/// -- two `.desktop` files presenting the same display name in the same
/// category would otherwise both show.
fn push_section(sections: &mut Vec<MenuNode>, label: &str, mut apps: Vec<(String, String)>) {
    if apps.is_empty() {
        return;
    }
    apps.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    apps.dedup_by(|a, b| a.0 == b.0);
    let items = apps
        .into_iter()
        .map(|(name, exec)| MenuNode::Item { label: name, command: exec })
        .collect();
    sections.push(MenuNode::Submenu { label: label.to_string(), items, default: None });
}

struct DesktopApp {
    name: String,
    exec: String,
    categories: Vec<String>,
}

impl DesktopApp {
    /// Parses one `.desktop` file's `[Desktop Entry]` group, returning
    /// `None` for anything that shouldn't appear in a menu: a non-
    /// `Application` type, `NoDisplay=true` or `Hidden=true`, a
    /// `TryExec` binary that isn't installed, or a missing `Name`/`Exec`.
    fn parse(contents: &str) -> Option<DesktopApp> {
        let mut in_entry = false;
        let mut name = None;
        let mut exec = None;
        let mut type_ = None;
        let mut try_exec = None;
        let mut categories = Vec::new();
        let mut no_display = false;
        let mut hidden = false;

        for line in contents.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                // Group header -- only the first [Desktop Entry] matters;
                // "Desktop Action Foo" groups and the rest are ignored.
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry || line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "Name" => name = Some(value.to_string()),
                "Exec" => exec = Some(value.to_string()),
                "Type" => type_ = Some(value.to_string()),
                "TryExec" => try_exec = Some(value.to_string()),
                "NoDisplay" => no_display = value == "true",
                "Hidden" => hidden = value == "true",
                "Categories" => {
                    categories = value.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect();
                }
                // Localized keys ("Name[de]"), OnlyShowIn/NotShowIn,
                // Icon, Keywords, etc. -- deliberately not consulted.
                _ => {}
            }
        }

        if hidden || no_display {
            return None;
        }
        if type_.as_deref().is_some_and(|t| t != "Application") {
            return None;
        }
        if let Some(bin) = try_exec.as_deref().filter(|s| !s.is_empty()) {
            if !binary_exists(bin) {
                return None;
            }
        }
        let name = name.filter(|s| !s.is_empty())?;
        let exec = strip_field_codes(&exec?);
        if exec.is_empty() {
            return None;
        }
        Some(DesktopApp { name, exec, categories })
    }
}

/// Strips the argument placeholders out of a Desktop Entry `Exec`
/// string so what's left just launches the app: the spec's `%` field
/// codes (`%f`, `%F`, `%u`, `%U`, `%i`, `%c`, `%k`, and any other
/// single letter -- `%%` unescapes to a literal `%`), and Flatpak's
/// own `@@` / `@@u` wrappers (which real Flatpak-generated files put
/// around the `%` codes). Whitespace the removals leave behind is
/// collapsed. The result is handed to `sh -c`, exactly as olvwm's own
/// `DIRMENU` did with its `exec ...` strings -- quoting subtleties in
/// the original `Exec` aren't preserved, which is fine for launching an
/// app with no arguments.
fn strip_field_codes(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|tok| !is_flatpak_placeholder(tok))
        .map(strip_percent_codes)
        .filter(|tok| !tok.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A standalone Flatpak argument wrapper: `@@` or `@@` plus one letter.
fn is_flatpak_placeholder(tok: &str) -> bool {
    tok == "@@" || (tok.len() == 3 && tok.starts_with("@@") && tok.as_bytes()[2].is_ascii_alphabetic())
}

/// Drops `%` field codes from one token, keeping a literal `%` for each
/// `%%` and ignoring a stray trailing `%`.
fn strip_percent_codes(tok: &str) -> String {
    let mut out = String::with_capacity(tok.len());
    let mut chars = tok.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
        } else if chars.next() == Some('%') {
            out.push('%');
        }
    }
    out
}

/// Whether `name` names an existing file: an absolute path checked
/// directly, otherwise searched for on `$PATH`. The executable bit
/// isn't checked -- `is_file` is a close-enough, lenient proxy, matching
/// this module's general approach.
fn binary_exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file();
    }
    let Some(env_path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&env_path).any(|dir| dir.join(name).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, contents: &str) {
        std::fs::write(dir.join(file), contents).unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("olwc-appmenu-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Pulls the flat list of (section, item labels) out of a generated
    /// APPMENU node for easy assertions.
    fn sections(node: &MenuNode) -> Vec<(String, Vec<String>)> {
        let MenuNode::Submenu { items, .. } = node else { panic!("expected submenu") };
        items
            .iter()
            .map(|section| {
                let MenuNode::Submenu { label, items, .. } = section else { panic!("expected section submenu") };
                (label.clone(), items.iter().map(|i| i.label().to_string()).collect())
            })
            .collect()
    }

    #[test]
    fn groups_sorts_and_strips() {
        let dir = temp_dir("basic");
        write(&dir, "gimp.desktop", "[Desktop Entry]\nType=Application\nName=GIMP\nExec=gimp %U\nCategories=Graphics;2DGraphics;\n");
        write(&dir, "inkscape.desktop", "[Desktop Entry]\nType=Application\nName=Inkscape\nExec=inkscape %F\nCategories=Graphics;VectorGraphics;\n");
        write(&dir, "firefox.desktop", "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nCategories=Network;WebBrowser;\n");
        write(&dir, "weird.desktop", "[Desktop Entry]\nType=Application\nName=Widget\nExec=widget\n");

        let node = generate_from(&[dir], "Programs".into());
        let s = sections(&node);
        assert_eq!(
            s,
            vec![
                ("Graphics".to_string(), vec!["GIMP".to_string(), "Inkscape".to_string()]),
                ("Internet".to_string(), vec!["Firefox".to_string()]),
                ("Other".to_string(), vec!["Widget".to_string()]),
            ]
        );

        // Field codes are gone from the commands.
        let MenuNode::Submenu { items, .. } = &node else { unreachable!() };
        let MenuNode::Submenu { items: graphics, .. } = &items[0] else { unreachable!() };
        let MenuNode::Item { command, .. } = &graphics[0] else { unreachable!() };
        assert_eq!(command, "gimp");
    }

    #[test]
    fn skips_hidden_nodisplay_and_wrong_type() {
        let dir = temp_dir("skip");
        write(&dir, "a.desktop", "[Desktop Entry]\nType=Application\nName=Shown\nExec=a\nCategories=Utility;\n");
        write(&dir, "b.desktop", "[Desktop Entry]\nType=Application\nName=NoShow\nExec=b\nNoDisplay=true\nCategories=Utility;\n");
        write(&dir, "c.desktop", "[Desktop Entry]\nType=Application\nName=Gone\nExec=c\nHidden=true\nCategories=Utility;\n");
        write(&dir, "d.desktop", "[Desktop Entry]\nType=Link\nName=Bookmark\nURL=http://example\nCategories=Utility;\n");

        let s = sections(&generate_from(&[dir], "Programs".into()));
        assert_eq!(s, vec![("Accessories".to_string(), vec!["Shown".to_string()])]);
    }

    #[test]
    fn tryexec_missing_binary_is_skipped() {
        let dir = temp_dir("tryexec");
        write(
            &dir,
            "ghost.desktop",
            "[Desktop Entry]\nType=Application\nName=Ghost\nExec=ghost\nTryExec=/nonexistent/olwc/ghost\nCategories=Utility;\n",
        );
        let MenuNode::Submenu { items, .. } = generate_from(&[dir], "Programs".into()) else { unreachable!() };
        assert!(items.is_empty());
    }

    #[test]
    fn earlier_dir_wins_by_id() {
        let first = temp_dir("first");
        let second = temp_dir("second");
        write(&first, "editor.desktop", "[Desktop Entry]\nType=Application\nName=Real Editor\nExec=real\nCategories=Development;\n");
        write(&second, "editor.desktop", "[Desktop Entry]\nType=Application\nName=Shadowed\nExec=shadow\nCategories=Development;\n");

        let s = sections(&generate_from(&[first, second], "Programs".into()));
        assert_eq!(s, vec![("Development".to_string(), vec!["Real Editor".to_string()])]);
    }

    #[test]
    fn empty_when_nothing_found() {
        let dir = temp_dir("empty");
        let MenuNode::Submenu { label, items, default } = generate_from(&[dir], "Programs".into()) else {
            unreachable!()
        };
        assert_eq!(label, "Programs");
        assert!(items.is_empty());
        assert_eq!(default, None);
    }

    #[test]
    fn strip_field_codes_cases() {
        assert_eq!(strip_field_codes("foo %U"), "foo");
        assert_eq!(strip_field_codes("foo %F --bar"), "foo --bar");
        assert_eq!(strip_field_codes("foo %%bar"), "foo %bar");
        assert_eq!(strip_field_codes("env A=1 foo %f"), "env A=1 foo");
        // Flatpak's @@ / @@u wrappers around the % codes are dropped too.
        assert_eq!(strip_field_codes("/usr/bin/flatpak run --command=x org.x.X @@u %U @@"), "/usr/bin/flatpak run --command=x org.x.X");
        assert_eq!(strip_field_codes("app foo@@bar"), "app foo@@bar");
    }
}
