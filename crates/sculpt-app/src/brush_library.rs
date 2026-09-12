//! Portable brush packs include the actual stamp data, not external paths.
use sculpt_core::{Alpha, io};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

#[derive(Serialize, Deserialize)]
struct Stamp {
    name: String,
    width: u32,
    height: u32,
    data: Vec<f32>,
}
#[derive(Serialize, Deserialize)]
struct Pack {
    version: u32,
    brushes: String,
    stamps: Vec<Stamp>,
}

pub fn save(path: &Path, brushes: &[io::NamedBrush], library: &[Arc<Alpha>]) -> Result<(), String> {
    use std::io::Write;
    let mut ids: Vec<_> = brushes.iter().filter_map(|b| b.brush.alpha).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.retain(|i| (*i as usize) < library.len());
    let mut brushes = brushes.to_vec();
    for brush in &mut brushes {
        brush.brush.alpha = brush
            .brush
            .alpha
            .and_then(|id| ids.binary_search(&id).ok())
            .map(|i| i as u32);
    }
    let names: Vec<_> = (0..ids.len()).map(|i| format!("stamp{i}")).collect();
    let mut text = Vec::new();
    io::write_brushes_to(&brushes, &names, &mut text)?;
    let pack = Pack {
        version: 1,
        brushes: String::from_utf8(text).map_err(|e| e.to_string())?,
        stamps: ids
            .into_iter()
            .map(|i| {
                let a = &library[i as usize];
                Stamp {
                    name: a.name.clone(),
                    width: a.w,
                    height: a.h,
                    data: a.data.clone(),
                }
            })
            .collect(),
    };
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(&temporary).map_err(|e| e.to_string())?);
    serde_json::to_writer(&mut file, &pack).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.get_ref().sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

pub fn load(path: &Path, library: &mut Vec<Arc<Alpha>>) -> Result<Vec<io::NamedBrush>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let pack: Pack =
        serde_json::from_reader(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    if pack.version != 1 {
        return Err("unsupported brush pack version".into());
    }
    for stamp in &pack.stamps {
        if stamp.width == 0
            || stamp.height == 0
            || stamp.width > 2048
            || stamp.height > 2048
            || stamp.data.len() != stamp.width as usize * stamp.height as usize
            || stamp.data.iter().any(|v| !v.is_finite())
        {
            return Err("invalid brush stamp".into());
        }
    }
    let names: Vec<_> = (0..pack.stamps.len())
        .map(|i| format!("stamp{i}"))
        .collect();
    let mut brushes = io::read_brushes_from(pack.brushes.as_bytes(), &names)?;
    let mut remap = Vec::new();
    for stamp in pack.stamps {
        if let Some(index) = library
            .iter()
            .position(|a| a.w == stamp.width && a.h == stamp.height && a.data == stamp.data)
        {
            remap.push(index as u32);
            continue;
        }
        let base = stamp.name.replace(['\n', '\r'], " ");
        let mut name = base.clone();
        let mut suffix = 2;
        while library.iter().any(|a| a.name == name) {
            name = format!("{base} ({suffix})");
            suffix += 1;
        }
        remap.push(library.len() as u32);
        library.push(Arc::new(Alpha::new(
            name,
            stamp.width,
            stamp.height,
            stamp.data,
        )));
    }
    for brush in &mut brushes {
        brush.brush.alpha = brush
            .brush
            .alpha
            .and_then(|i| remap.get(i as usize).copied());
    }
    Ok(brushes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pack_carries_stamps_and_resolves_name_collisions() {
        let source = vec![Arc::new(Alpha::new("Stone", 2, 1, vec![0.0, 1.0]))];
        let mut brush = sculpt_core::Brush::default();
        brush.alpha = Some(0);
        let named = io::NamedBrush {
            name: "Carver".into(),
            brush,
            icon: Some("Crease".into()),
        };
        let path = std::env::temp_dir().join(format!(
            "sculpt-pack-test-{}.sculptbrush",
            std::process::id()
        ));
        save(&path, &[named], &source).unwrap();
        let mut target = vec![Arc::new(Alpha::new("Stone", 2, 1, vec![1.0, 0.0]))];
        let loaded = load(&path, &mut target).unwrap();
        assert_eq!(loaded[0].brush.alpha, Some(1));
        assert_eq!(target[1].name, "Stone (2)");
        assert_eq!(target[1].data, source[0].data);
        assert_eq!(loaded[0].icon.as_deref(), Some("Crease"));
        load(&path, &mut target).unwrap();
        assert_eq!(target.len(), 2);
        let _ = std::fs::remove_file(path);
    }
}
