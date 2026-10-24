use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use eframe::egui;
use egui_file_dialog::FileDialog;



#[derive(Clone)]
struct DesktopEntry {
    path: PathBuf,
    app_id: String,
    name: String,
    icon: String,
    exec: String,
    system_wide: bool,
}

struct IconSwapper {
    entries: Vec<DesktopEntry>,
    selected: Option<usize>,
    search: String,
    file_dialog: FileDialog,
    pending_image: Option<PathBuf>,
    status: String,
    textures: HashMap<String, egui::TextureHandle>,
}

impl IconSwapper {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let entries = load_all_entries().unwrap_or_default();
        Self {
            entries,
            selected: None,
            search: String::new(),
            file_dialog: FileDialog::new()
                .add_file_filter(
                    "Images",
                    Arc::new(|path| {
                        path.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| matches!(e.to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp"))
                            .unwrap_or(false)
                    }),
                )
                .default_file_filter("Images"),
            pending_image: None,
            status: String::new(),
            textures: HashMap::new(),
        }
    }

    fn ensure_preview(&mut self, ctx: &egui::Context, entry: &DesktopEntry) {
        let key = entry.path.to_string_lossy().to_string();
        if self.textures.contains_key(&key) {
            return;
        }
        if let Some(path) = resolve_icon_path(&entry.icon) {
            if let Some(tex) = load_texture_from_path(ctx, &path) {
                self.textures.insert(key, tex);
            }
        }
    }

    fn apply_new_icon(&mut self, entry: &DesktopEntry, image_path: &Path) -> Result<PathBuf> {
        let icon_name = icon_name_for(&entry.app_id);
        install_app_icon(&entry.app_id, &icon_name, image_path)?;

        let target_path = if entry.system_wide {
            let user_path = dirs::data_dir()
                .context("could not find user data directory")?
                .join("applications")
                .join(format!("{}.desktop", entry.app_id));
            std::fs::create_dir_all(user_path.parent().unwrap())?;
            std::fs::copy(&entry.path, &user_path)
                .with_context(|| format!("failed to copy {} to user apps", entry.path.display()))?;
            user_path
        } else {
            entry.path.clone()
        };

        update_desktop_icon(&target_path, &icon_name)?;
        Ok(target_path)
    }
}

fn icon_name_for(app_id: &str) -> String {
    if system_theme_has_icon(app_id) {
        format!("{app_id}-iconswapper")
    } else {
        app_id.to_string()
    }
}

fn system_theme_has_icon(icon_name: &str) -> bool {
    let system_dirs = ["/usr/share/icons", "/usr/local/share/icons"];
    let exts = ["png", "svg", "xpm"];
    for dir in system_dirs {
        if !Path::new(dir).exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(dir).follow_links(true) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != icon_name {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if exts.contains(&ext.to_lowercase().as_str()) {
                    return true;
                }
            }
        }
    }
    false
}

fn install_app_icon(app_id: &str, icon_name: &str, image_path: &Path) -> Result<PathBuf> {
    let data_dir = dirs::data_dir().context("could not find user data directory")?;
    let hicolor = data_dir.join("icons/hicolor");

    // Remove any previous user icon for this app so old sizes/formats don't
    // shadow the new one. Clean both the base app_id and the derived icon_name.
    if hicolor.exists() {
        for entry in walkdir::WalkDir::new(&hicolor).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem == app_id || stem == icon_name {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let img = image::open(image_path)
        .with_context(|| format!("failed to open image {}", image_path.display()))?;

    let mut last_target = None;
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let dir = hicolor.join(format!("{size}x{size}/apps"));
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(format!("{icon_name}.png"));
        let resized = img.resize(size, size, image::imageops::FilterType::Lanczos3);
        resized.save(&target)?;
        last_target = Some(target);
    }

    let _ = std::process::Command::new("gtk-update-icon-cache")
        .arg("-f")
        .arg("-t")
        .arg(&hicolor)
        .status();

    let _ = std::process::Command::new("update-desktop-database")
        .arg(data_dir.join("applications"))
        .status();

    last_target.context("no icon size was generated")
}

impl eframe::App for IconSwapper {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.file_dialog.update(ctx);

        if let Some(path) = self.file_dialog.take_selected() {
            self.pending_image = Some(path);
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.heading("Application Icon Swapper");
            ui.label("Pick an application, then choose a new image to use as its icon.");
            ui.separator();
        });

        egui::SidePanel::left("app_list")
            .resizable(true)
            .min_width(280.0)
            .show(ctx, |ui| {
                ui.label("Applications");
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search apps…")
                        .desired_width(f32::INFINITY),
                );

                let query = self.search.to_lowercase();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let mut matched = false;
                        for (idx, entry) in self.entries.iter().enumerate() {
                            if !query.is_empty()
                                && !entry.name.to_lowercase().contains(&query)
                            {
                                continue;
                            }
                            matched = true;
                            let selected = self.selected == Some(idx);
                            let label = if entry.system_wide {
                                format!("{} (system)", entry.name)
                            } else {
                                entry.name.clone()
                            };
                            let response = ui.selectable_label(selected, label);
                            if response.clicked() {
                                self.selected = Some(idx);
                                self.status.clear();
                            }
                        }
                        if !matched && !query.is_empty() {
                            ui.label("No applications match your search.");
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(idx) = self.selected {
                let entry = self.entries[idx].clone();
                self.ensure_preview(ctx, &entry);

                ui.label(format!("Name: {}", entry.name));
                ui.label(format!("Command: {}", entry.exec));
                ui.label(format!("Desktop file: {}", entry.path.display()));
                if entry.system_wide {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 0), "System app — a user override will be created");
                }
                ui.label(format!("Current icon: {}", entry.icon));

                if let Some(tex) = self.textures.get(&entry.path.to_string_lossy().to_string()) {
                    ui.image(tex);
                } else {
                    ui.label("(no preview available)");
                }

                ui.separator();

                if ui.button("Choose new image…").clicked() {
                    self.file_dialog.select_file();
                }

                if let Some(pending) = self.pending_image.clone() {
                    ui.label(format!("Selected image: {}", pending.display()));
                    if ui.button("Apply new icon").clicked() {
                        match self.apply_new_icon(&entry, &pending) {
                            Ok(target_path) => {
                                self.status = format!("Icon updated for {}", entry.name);
                                self.pending_image = None;
                                // Refresh entry and texture
                                if let Some(updated) = load_single_entry(&target_path, false) {
                                    self.entries[idx] = updated.clone();
                                    self.textures.remove(&entry.path.to_string_lossy().to_string());
                                    self.ensure_preview(ctx, &updated);
                                }
                            }
                            Err(e) => self.status = format!("Error: {e:#}"),
                        }
                    }
                }
            } else {
                ui.label("Select an application from the list.");
            }

            if !self.status.is_empty() {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(0, 128, 0), &self.status);
            }
        });
    }
}

fn load_all_entries() -> Result<Vec<DesktopEntry>> {
    let mut seen = HashMap::new();

    // User entries take precedence, so load them first.
    let user_dir = dirs::data_dir()
        .context("could not find user data directory")?
        .join("applications");

    if user_dir.exists() {
        for entry in std::fs::read_dir(&user_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                if let Some(d) = load_single_entry(&path, false) {
                    seen.insert(d.app_id.clone(), d);
                }
            }
        }
    }

    let system_dirs: Vec<PathBuf> = vec![
        PathBuf::from("/usr/local/share/applications"),
        PathBuf::from("/usr/share/applications"),
    ];

    for dir in system_dirs {
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("desktop") {
                if let Some(d) = load_single_entry(&path, true) {
                    // Only add system entry if a user override doesn't already exist.
                    seen.entry(d.app_id.clone()).or_insert(d);
                }
            }
        }
    }

    let mut entries: Vec<DesktopEntry> = seen.into_values().collect();
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

fn load_single_entry(path: &Path, system_wide: bool) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let app_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut name = app_id.clone();
    let mut icon = String::new();
    let mut exec = String::new();

    let mut in_desktop_entry = false;
    for line in content.lines() {
        if line.trim() == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }
        if line.starts_with('[') {
            in_desktop_entry = false;
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        if let Some(value) = line.strip_prefix("Name=") {
            name = value.to_string();
        } else if let Some(value) = line.strip_prefix("Icon=") {
            icon = value.to_string();
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec = value.to_string();
        }
    }

    Some(DesktopEntry {
        path: path.to_path_buf(),
        app_id,
        name,
        icon,
        exec,
        system_wide,
    })
}

fn update_desktop_icon(path: &Path, icon_name: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut in_desktop_entry = false;
    let mut found_icon = false;
    let mut new_lines = Vec::new();

    for line in content.lines() {
        if line.trim() == "[Desktop Entry]" {
            in_desktop_entry = true;
            new_lines.push(line.to_string());
            continue;
        }
        if line.starts_with('[') {
            if in_desktop_entry && !found_icon {
                new_lines.push(format!("Icon={icon_name}"));
                found_icon = true;
            }
            in_desktop_entry = false;
            new_lines.push(line.to_string());
            continue;
        }
        if in_desktop_entry && line.starts_with("Icon=") {
            new_lines.push(format!("Icon={icon_name}"));
            found_icon = true;
            continue;
        }
        new_lines.push(line.to_string());
    }

    if in_desktop_entry && !found_icon {
        new_lines.push(format!("Icon={icon_name}"));
    }

    std::fs::write(path, new_lines.join("\n"))?;
    Ok(())
}

fn resolve_icon_path(icon: &str) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }

    // Absolute or relative path
    if icon.starts_with('/') || icon.starts_with("./") || icon.starts_with("../") {
        let p = PathBuf::from(icon);
        if p.exists() {
            return Some(p);
        }
        return None;
    }

    // Theme icon name lookup
    let home = dirs::home_dir()?;
    let data_dir = dirs::data_dir()?;

    let search_roots: Vec<PathBuf> = vec![
        home.join(".icons"),
        data_dir.join("icons"),
        PathBuf::from("/usr/share/icons"),
    ];

    let exts = ["png", "svg", "xpm"];

    for root in &search_roots {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root).follow_links(true) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if stem != icon {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if exts.contains(&ext.to_lowercase().as_str()) {
                    return Some(path.to_path_buf());
                }
            }
        }
    }

    None
}

fn load_texture_from_path(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    if path.extension().and_then(|e| e.to_str()) == Some("svg") {
        return None;
    }
    let img = image::open(path).ok()?;
    let img = img.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img.as_raw();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels);
    Some(ctx.load_texture(
        path.to_string_lossy().to_string(),
        color_image,
        egui::TextureOptions::default(),
    ))
}

fn main() -> Result<()> {
    // Force X11/XWayland backend; egui/eframe's native Wayland backend can be
    // unreliable on some compositors, while XWayland works everywhere.
    std::env::remove_var("WAYLAND_DISPLAY");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Icon Swapper",
        options,
        Box::new(|cc| Ok(Box::new(IconSwapper::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_and_update_desktop_icon() {
        let tmp = std::env::temp_dir().join("icon-swapper-test.desktop");
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(
            b"[Desktop Entry]\nName=Test App\nExec=test-app\nIcon=old-icon\nType=Application\n",
        )
        .unwrap();
        drop(f);

        let entry = load_single_entry(&tmp, false).expect("parse entry");
        assert_eq!(entry.name, "Test App");
        assert_eq!(entry.icon, "old-icon");

        update_desktop_icon(&tmp, "new-icon").unwrap();
        let content = std::fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("Icon=new-icon"));
        assert!(!content.contains("Icon=old-icon"));

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn install_icon_resizes_image() {
        let tmp = std::env::temp_dir();
        let src = tmp.join("icon-swapper-test-src.png");

        // Create a 512x512 red PNG
        let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([255, 0, 0, 255]));
        img.save(&src).unwrap();

        let installed = install_app_icon("test-app-icon", "test-app-icon", &src).unwrap();
        assert!(installed.exists());

        let loaded = image::open(&installed).unwrap();
        assert_eq!(loaded.width(), 256);
        assert_eq!(loaded.height(), 256);

        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&installed).ok();
    }
}
