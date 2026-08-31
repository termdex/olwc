// Icon lookup for a toplevel's app_id: find its .desktop file, read the
// Icon= key, then resolve that name to an actual PNG file via a bounded
// subset of the freedesktop Icon Theme spec. Deliberately a lenient
// subset throughout, not a full reimplementation -- same spirit as
// menu.rs's own .openwin-menu parser, and see docs/DESIGN.md's icon-
// thumbnail-glyph entry for the authenticity reasoning behind using an
// app's own icon at all (real OPEN LOOK icons were app-supplied bitmaps,
// not a generic glyph).

use std::path::{Path, PathBuf};

/// Icon theme roots to search, in priority order -- not the user's
/// actually-configured theme (detecting that means parsing GTK/KDE
/// settings olshell has no other reason to touch), just a fixed,
/// practical list: `hicolor` (every conformant icon-theme install ships
/// it, the spec's own universal fallback) plus two GNOME-family themes
/// that, on this project's own dev system, turned out to carry PNG
/// copies of icons hicolor itself didn't (e.g. Konsole's own
/// `utilities-terminal`, and the `application-x-executable` fallback
/// this module's own caller uses). PNG-only (see docs/DESIGN.md): themes
/// that ship only SVG for a given icon (Breeze, current non-Legacy
/// Adwaita) simply won't resolve here.
const ICON_THEMES: &[&str] = &["hicolor", "AdwaitaLegacy", "Adwaita"];

/// Sizes to look for, largest first -- downscaling a big icon for the
/// tray looks better than upscaling a small one.
const ICON_SIZES: &[u32] = &[128, 64, 48, 32, 24, 22, 16];

fn xdg_data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let raw = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    raw.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
}

/// Every `applications` directory to search, most-preferred first.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = xdg_data_home().into_iter().collect();
    dirs.extend(xdg_data_dirs());
    dirs.into_iter().map(|d| d.join("applications")).collect()
}

/// Every icon theme base directory to search (each has ICON_THEMES'
/// members as its own immediate subdirectories), most-preferred first.
fn icon_base_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = xdg_data_home().into_iter().collect();
    dirs.extend(xdg_data_dirs());
    dirs.into_iter().map(|d| d.join("icons")).collect()
}

/// Finds `<app_id>.desktop` and returns its `Icon=` value, if any -- a
/// bare name to resolve via find_icon_file, or (less commonly) an
/// absolute path, which find_icon_file handles the same way either way.
/// Deliberately exact-match only: no fuzzy/case-insensitive matching, no
/// searching subdirectories of `applications` (some distros nest vendor
/// desktop files there) -- a real gap for an app_id that doesn't exactly
/// match its own desktop file's basename, but an accepted, widespread
/// limitation every desktop shell's naive lookup shares, not something
/// worth a more elaborate search for.
pub fn desktop_icon_name(app_id: &str) -> Option<String> {
    desktop_icon_name_in(&application_dirs(), app_id)
}

fn desktop_icon_name_in(app_dirs: &[PathBuf], app_id: &str) -> Option<String> {
    for dir in app_dirs {
        let path = dir.join(format!("{app_id}.desktop"));
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("Icon=") {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Resolves an icon name (as from a .desktop file's Icon= key, or a
/// fallback name like "application-x-executable") to an actual PNG file.
/// An absolute path is used directly, existence-checked, no searching.
/// Otherwise searches ICON_THEMES at ICON_SIZES across every icon base
/// directory, then a flat `/usr/share/pixmaps/<name>.png` as a last
/// resort (where some older/simpler apps install a single icon
/// directly). Deliberately not full Icon Theme spec compliance: no
/// index.theme parsing (so no theme-inheritance chains, no exact size/
/// context matching beyond the fixed list above) -- a bounded, practical
/// search, not a spec-complete resolver.
pub fn find_icon_file(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    find_icon_file_in(&icon_base_dirs(), Path::new("/usr/share/pixmaps"), name)
}

fn find_icon_file_in(icon_base_dirs: &[PathBuf], pixmaps_dir: &Path, name: &str) -> Option<PathBuf> {
    for base in icon_base_dirs {
        for theme in ICON_THEMES {
            let theme_dir = base.join(theme);
            if let Some(found) = find_in_theme(&theme_dir, name) {
                return Some(found);
            }
        }
    }
    let direct = pixmaps_dir.join(format!("{name}.png"));
    direct.is_file().then_some(direct)
}

/// Bounded search within one theme root: real icon themes lay out
/// category (apps/mimetypes/actions/...) and size directories in either
/// order -- hicolor uses `<size>x<size>/<category>/`, most others use
/// `<category>/<size>/` -- confirmed by finding both conventions in use
/// on the same system while researching this (Konsole's own
/// `utilities-terminal` under `AdwaitaLegacy/48x48/legacy/`,
/// `application-x-executable` under `AdwaitaLegacy/48x48/mimetypes/`).
/// Rather than parsing the theme's own index.theme to know which
/// category names exist, just checks both directory orderings directly
/// at each size (largest first), enumerating whichever level's
/// subdirectories aren't already known instead of hardcoding a category
/// name list.
fn find_in_theme(theme_dir: &Path, name: &str) -> Option<PathBuf> {
    if !theme_dir.is_dir() {
        return None;
    }
    let filename = format!("{name}.png");
    for &size in ICON_SIZES {
        // hicolor-style: <theme>/<size>x<size>/<category>/<name>.png
        let size_dir = theme_dir.join(format!("{size}x{size}"));
        if let Some(found) = find_in_size_dir(&size_dir, &filename) {
            return Some(found);
        }
        // breeze/Adwaita-style: <theme>/<category>/<size>/<name>.png --
        // category unknown, so check every immediate child of the theme
        // root for a <size> subdirectory.
        if let Ok(entries) = std::fs::read_dir(theme_dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(size.to_string()).join(&filename);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Checks `size_dir` itself, then every immediate child directory of it,
/// for `filename` -- covers both a flat `<size_dir>/<name>.png` layout
/// and the category-subdirectory one (`<size_dir>/<category>/<name>.png`)
/// without needing to know the category name in advance.
fn find_in_size_dir(size_dir: &Path, filename: &str) -> Option<PathBuf> {
    let direct = size_dir.join(filename);
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(size_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("olwc-icon-theme-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_desktop_icon_name() {
        let dir = scratch_dir("desktop");
        std::fs::write(
            dir.join("org.example.Foo.desktop"),
            "[Desktop Entry]\nName=Foo\nIcon=utilities-terminal\nExec=foo\n",
        )
        .unwrap();
        assert_eq!(
            desktop_icon_name_in(&[dir], "org.example.Foo").as_deref(),
            Some("utilities-terminal")
        );
    }

    #[test]
    fn missing_desktop_file_returns_none() {
        let dir = scratch_dir("missing");
        assert_eq!(desktop_icon_name_in(&[dir], "nonexistent"), None);
    }

    #[test]
    fn finds_hicolor_style_layout() {
        let base = scratch_dir("hicolor-style");
        let theme_dir = base.join("hicolor").join("48x48").join("apps");
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(theme_dir.join("myapp.png"), b"not a real png, just a marker").unwrap();

        let found = find_icon_file_in(&[base], Path::new("/nonexistent"), "myapp");
        assert_eq!(found, Some(theme_dir.join("myapp.png")));
    }

    #[test]
    fn finds_category_first_style_layout() {
        let base = scratch_dir("category-style");
        let theme_dir = base.join("AdwaitaLegacy").join("legacy").join("48");
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(theme_dir.join("utilities-terminal.png"), b"marker").unwrap();

        let found = find_icon_file_in(&[base], Path::new("/nonexistent"), "utilities-terminal");
        assert_eq!(found, Some(theme_dir.join("utilities-terminal.png")));
    }

    #[test]
    fn falls_back_to_pixmaps() {
        let base = scratch_dir("pixmaps-base");
        let pixmaps = scratch_dir("pixmaps-dir");
        std::fs::write(pixmaps.join("standalone.png"), b"marker").unwrap();

        let found = find_icon_file_in(&[base], &pixmaps, "standalone");
        assert_eq!(found, Some(pixmaps.join("standalone.png")));
    }

    #[test]
    fn absolute_path_used_directly() {
        let dir = scratch_dir("absolute");
        let file = dir.join("icon.png");
        std::fs::write(&file, b"marker").unwrap();
        assert_eq!(find_icon_file(file.to_str().unwrap()), Some(file));
    }
}
