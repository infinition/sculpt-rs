//! Versioned, portable workspace settings. Saved explicitly or on clean exit.
//! One atomic file contains the layout, brush library and imported stamps.
use crate::ui::{Anchor, Harmony, MatcapSource, Tab, UiState};
use crate::{hud::Pod, input::Bindings, matcap, renderer::FrameSettings, theme::UiTheme};
use sculpt_core::{Alpha, Sculptor, io};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use winit::keyboard::KeyCode;

#[derive(Serialize, Deserialize)]
struct Stamp {
    name: String,
    width: u32,
    height: u32,
    values: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Workspace {
    version: u32,
    theme: UiTheme,
    settings: FrameSettings,
    tab: Tab,
    floating: Vec<Tab>,
    panels: [bool; 4],
    pods: Vec<Pod>,
    bindings: Bindings,
    wheel_key: KeyCode,
    anchor: Anchor,
    harmony: Harmony,
    preview_opacity: f32,
    brush_reach: f32,
    samples: u32,
    adaptive_msaa: bool,
    nav_size: f32,
    nav_buttons: bool,
    matcap: matcap::Preset,
    matcap_source: MatcapSource,
    lightcap: matcap::Lightcap,
    matcap_image: Option<(u32, Vec<u8>)>,
    brushes: String,
    materials: Vec<crate::materials::Material>,
    swatches: Vec<[f32; 3]>,
    wheel_size: f32,
    wheel_colors: bool,
    wheel_grayscale: bool,
    current_brush: String,
    tool_presets: String,
    stamps: Vec<Stamp>,
    memory: egui::Memory,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::capture(
            &UiState::default(),
            &Sculptor::new(sculpt_core::Mesh::new()),
            &egui::Context::default(),
        )
        .expect("default brushes are serializable")
    }
}

impl Workspace {
    fn capture(ui: &UiState, s: &Sculptor, ctx: &egui::Context) -> Result<Self, String> {
        let names: Vec<_> = s.alphas.iter().map(|a| a.name.clone()).collect();
        let encode = |brushes: &[io::NamedBrush]| {
            let mut bytes = Vec::new();
            io::write_brushes_to(brushes, &names, &mut bytes)?;
            String::from_utf8(bytes).map_err(|e| e.to_string())
        };
        Ok(Self {
            version: 1,
            theme: ui.theme,
            settings: ui.settings,
            tab: ui.tab,
            floating: ui.floating.clone(),
            panels: [ui.show_panel, ui.show_rail, ui.show_stats, ui.show_nav],
            pods: ui.hud.pods.clone(),
            bindings: ui.bindings,
            wheel_key: ui.wheel_key,
            anchor: ui.anchor,
            harmony: ui.harmony,
            preview_opacity: ui.preview_opacity,
            brush_reach: ui.brush_reach,
            samples: ui.sample_count,
            adaptive_msaa: ui.adaptive_msaa,
            nav_size: ui.nav.size,
            nav_buttons: ui.nav.show_buttons,
            matcap: ui.matcap,
            matcap_source: ui.matcap_source,
            lightcap: ui.lightcap,
            matcap_image: ui.matcap_image.clone(),
            brushes: encode(&ui.brushes)?,
            materials: ui.materials.clone(),
            swatches: ui.wheel.swatches.clone(),
            wheel_size: ui.wheel.size,
            wheel_colors: ui.wheel.show_colors,
            wheel_grayscale: ui.wheel.grayscale,
            current_brush: encode(&[io::NamedBrush {
                name: "Current".into(),
                brush: s.brush,
                icon: None,
            }])?,
            tool_presets: encode(
                &s.brush_presets()
                    .into_iter()
                    .map(|brush| io::NamedBrush {
                        name: brush.kind.label().into(),
                        brush,
                        icon: None,
                    })
                    .collect::<Vec<_>>(),
            )?,
            // Built-ins are generated identically at startup; save imported data only.
            stamps: s
                .alphas
                .iter()
                .skip(sculpt_core::alpha::Shape::ALL.len())
                .map(|a| Stamp {
                    name: a.name.clone(),
                    width: a.w,
                    height: a.h,
                    values: a.data.clone(),
                })
                .collect(),
            memory: ctx.memory(Clone::clone),
        })
    }

    fn apply(self, ui: &mut UiState, s: &mut Sculptor, ctx: &egui::Context) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("unsupported workspace version {}", self.version));
        }
        // Validate before changing the live library or handing data to the GPU.
        for stamp in &self.stamps {
            if stamp.width == 0
                || stamp.height == 0
                || stamp.width > 2048
                || stamp.height > 2048
                || stamp.values.len() != stamp.width as usize * stamp.height as usize
            {
                return Err("invalid saved alpha dimensions".into());
            }
        }
        if let Some((size, pixels)) = &self.matcap_image {
            if *size == 0 || *size > 1024 || pixels.len() != *size as usize * *size as usize * 4 {
                return Err("invalid saved matcap dimensions".into());
            }
        }
        s.alphas.truncate(sculpt_core::alpha::Shape::ALL.len());
        s.alphas.extend(
            self.stamps
                .into_iter()
                .map(|a| Arc::new(Alpha::new(a.name, a.width, a.height, a.values))),
        );
        let names: Vec<_> = s.alphas.iter().map(|a| a.name.clone()).collect();
        ui.brushes = io::read_brushes_from(self.brushes.as_bytes(), &names)?;
        let presets: Vec<_> = io::read_brushes_from(self.tool_presets.as_bytes(), &names)?
            .into_iter()
            .map(|b| b.brush)
            .collect();
        ui.materials = self.materials;
        ui.wheel.swatches = self.swatches;
        ui.wheel.size = self.wheel_size.clamp(100.0, 400.0);
        ui.wheel.show_colors = self.wheel_colors;
        ui.wheel.grayscale = self.wheel_grayscale;
        if let Some(current) = io::read_brushes_from(self.current_brush.as_bytes(), &names)?.first()
        {
            s.set_brush_kind(current.brush.kind);
            s.brush = current.brush;
        }
        s.restore_brush_presets(&presets);
        ui.theme = self.theme;
        ui.theme.ui_scale = ui.theme.ui_scale.clamp(0.6, 2.2);
        ui.theme.panel_width = ui.theme.panel_width.clamp(180.0, 800.0);
        ui.theme.tool_size = ui.theme.tool_size.clamp(24.0, 96.0);
        ui.theme.row_height = ui.theme.row_height.clamp(20.0, 72.0);
        ui.ui_scale_draft = ui.theme.ui_scale;
        ui.settings = self.settings;
        ui.settings.wireframe &= ui.wireframe_available;
        ui.tab = self.tab;
        ui.floating = self.floating;
        let mut unique = std::collections::HashSet::new();
        ui.floating.retain(|tab| unique.insert(*tab));
        [ui.show_panel, ui.show_rail, ui.show_stats, ui.show_nav] = self.panels;
        for saved in self.pods {
            if let Some(pod) = ui.hud.pods.iter_mut().find(|pod| pod.kind == saved.kind) {
                *pod = saved;
                pod.at = pod.at.clamp(egui::Vec2::ZERO, egui::Vec2::splat(1.0));
                pod.size = pod.size.clamp(24.0, 160.0);
            }
        }
        ui.bindings = self.bindings;
        ui.wheel_key = self.wheel_key;
        ui.anchor = self.anchor;
        ui.harmony = self.harmony;
        ui.preview_opacity = self.preview_opacity.clamp(0.0, 0.8);
        ui.brush_reach = self.brush_reach.clamp(0.0, 100.0);
        ui.sample_count = [8, 4, 2, 1]
            .into_iter()
            .find(|n| *n <= self.samples.min(ui.max_samples))
            .unwrap_or(1);
        ui.adaptive_msaa = self.adaptive_msaa;
        ui.nav.size = self.nav_size.clamp(60.0, 200.0);
        ui.nav.show_buttons = self.nav_buttons;
        ui.matcap = self.matcap;
        ui.matcap_source = self.matcap_source;
        ui.lightcap = self.lightcap;
        ui.matcap_image = self.matcap_image;
        ctx.memory_mut(|memory| *memory = self.memory);
        Ok(())
    }
}

pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SCULPT_RS_CONFIG") {
        return Some(path.into());
    }
    #[cfg(target_os = "windows")]
    let root = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let root =
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")));
    root.map(|p| p.join("sculpt-rs/workspace.json"))
}

pub fn load(ui: &mut UiState, s: &mut Sculptor, ctx: &egui::Context) -> Result<(), String> {
    let Some(path) = path().filter(|p| p.exists()) else {
        return Ok(());
    };
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<Workspace>(&bytes)
                .map_err(|e| e.to_string())?
                .apply(ui, s, ctx)
        });
    if result.is_err() {
        // Preserve an incompatible or damaged file before clean exit writes defaults.
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup = path.with_extension(format!("invalid-{suffix}.json"));
        std::fs::copy(&path, backup).map_err(|e| format!("cannot back up workspace: {e}"))?;
    }
    result
}

pub fn save(ui: &UiState, s: &Sculptor, ctx: &egui::Context) -> Result<(), String> {
    let path = path().ok_or("configuration directory unavailable")?;
    save_to(&path, &Workspace::capture(ui, s, ctx)?)
}

fn save_to(path: &std::path::Path, workspace: &Workspace) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(&temporary).map_err(|e| e.to_string())?);
    serde_json::to_writer(&mut file, workspace).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.get_ref().sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_restores_brush_assets_bindings_and_layout() {
        let mut ui = UiState::default();
        let mut s = Sculptor::new(sculpt_core::Mesh::new());
        let ctx = egui::Context::default();
        s.brush.strength = 0.81;
        s.set_brush_kind(sculpt_core::BrushKind::Paint);
        ui.theme.ui_scale = 1.4;
        ui.floating = vec![Tab::Brush, Tab::View];
        ui.bindings.touch = crate::input::Role::Pan;
        s.alphas.push(Arc::new(Alpha::new(
            "Imported",
            2,
            2,
            vec![0.0, 0.2, 0.7, 1.0],
        )));
        s.brush.alpha = Some((s.alphas.len() - 1) as u32);
        ui.brushes.push(io::NamedBrush {
            name: "My clay".into(),
            brush: s.brush,
            icon: Some("Smooth".into()),
        });
        let bytes = serde_json::to_vec(&Workspace::capture(&ui, &s, &ctx).unwrap()).unwrap();
        let mut restored = UiState::default();
        let mut engine = Sculptor::new(sculpt_core::Mesh::new());
        serde_json::from_slice::<Workspace>(&bytes)
            .unwrap()
            .apply(&mut restored, &mut engine, &ctx)
            .unwrap();
        assert_eq!(restored.theme.ui_scale, 1.4);
        assert_eq!(restored.floating, ui.floating);
        assert_eq!(restored.bindings.touch, crate::input::Role::Pan);
        assert_eq!(restored.brushes[0].icon.as_deref(), Some("Smooth"));
        assert_eq!(
            engine.alphas[engine.brush.alpha.unwrap() as usize].data,
            vec![0.0, 0.2, 0.7, 1.0]
        );
        engine.set_brush_kind(sculpt_core::BrushKind::Clay);
        assert!((engine.brush.strength - 0.81).abs() < 1e-5);
    }
}
