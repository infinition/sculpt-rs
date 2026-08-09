//! Mesh import/export: OBJ and binary STL.

use crate::mesh::Mesh;
use crate::primitives::weld;
use glam::Vec3;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub type IoResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Writes an OBJ. Vertex colours ride along on the `v` lines, an extension
/// MeshLab and Blender both read back.
pub fn write_obj(mesh: &Mesh, path: &Path, with_colors: bool) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "# exported by sculpt-rs").map_err(err)?;
    for v in &mesh.verts {
        if with_colors {
            writeln!(
                w,
                "v {:.6} {:.6} {:.6} {:.4} {:.4} {:.4}",
                v.pos.x, v.pos.y, v.pos.z, v.col.x, v.col.y, v.col.z
            )
        } else {
            writeln!(w, "v {:.6} {:.6} {:.6}", v.pos.x, v.pos.y, v.pos.z)
        }
        .map_err(err)?;
    }
    for v in &mesh.verts {
        writeln!(w, "vn {:.6} {:.6} {:.6}", v.nrm.x, v.nrm.y, v.nrm.z).map_err(err)?;
    }
    for t in &mesh.faces {
        writeln!(
            w,
            "f {0}//{0} {1}//{1} {2}//{2}",
            t[0] + 1,
            t[1] + 1,
            t[2] + 1
        )
        .map_err(err)?;
    }
    w.flush().map_err(err)?;
    Ok(())
}

/// Reads an OBJ. Polygons are fanned into triangles; texture and normal
/// indices are ignored since the sculptor recomputes normals anyway.
pub fn read_obj(path: &Path) -> IoResult<Mesh> {
    let f = File::open(path).map_err(err)?;
    let r = BufReader::new(f);
    let mut pos: Vec<Vec3> = Vec::new();
    let mut cols: Vec<Vec3> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();

    for line in r.lines() {
        let line = line.map_err(err)?;
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                if c.len() < 3 {
                    continue;
                }
                pos.push(Vec3::new(c[0], c[1], c[2]));
                cols.push(if c.len() >= 6 {
                    Vec3::new(c[3], c[4], c[5])
                } else {
                    Vec3::splat(0.85)
                });
            }
            Some("f") => {
                let idx: Vec<i64> = it
                    .filter_map(|tok| tok.split('/').next())
                    .filter_map(|x| x.parse::<i64>().ok())
                    .collect();
                if idx.len() < 3 {
                    continue;
                }
                let resolve = |i: i64| -> u32 {
                    if i > 0 {
                        (i - 1) as u32
                    } else {
                        (pos.len() as i64 + i) as u32
                    }
                };
                for k in 1..idx.len() - 1 {
                    faces.push([resolve(idx[0]), resolve(idx[k]), resolve(idx[k + 1])]);
                }
            }
            _ => {}
        }
    }

    if pos.is_empty() || faces.is_empty() {
        return Err("no geometry found in OBJ".into());
    }
    let n = pos.len() as u32;
    faces.retain(|t| t.iter().all(|&i| i < n));
    let mut mesh = Mesh::from_soup(&pos, &faces);
    for (v, c) in mesh.verts.iter_mut().zip(cols) {
        v.col = c;
    }
    Ok(mesh)
}

pub fn write_stl(mesh: &Mesh, path: &Path) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    let mut header = [0u8; 80];
    let tag = b"sculpt-rs binary STL";
    header[..tag.len()].copy_from_slice(tag);
    w.write_all(&header).map_err(err)?;
    w.write_all(&(mesh.faces.len() as u32).to_le_bytes()).map_err(err)?;
    for (fi, tri) in mesh.faces.iter().enumerate() {
        let n = mesh.face_normal(fi as u32).normalize_or(Vec3::Y);
        for c in [n.x, n.y, n.z] {
            w.write_all(&c.to_le_bytes()).map_err(err)?;
        }
        for &v in tri {
            let p = mesh.verts[v as usize].pos;
            for c in [p.x, p.y, p.z] {
                w.write_all(&c.to_le_bytes()).map_err(err)?;
            }
        }
        w.write_all(&0u16.to_le_bytes()).map_err(err)?;
    }
    w.flush().map_err(err)?;
    Ok(())
}

/// Reads a binary STL and welds the soup back into a connected mesh.
pub fn read_stl(path: &Path) -> IoResult<Mesh> {
    let mut buf = Vec::new();
    File::open(path).map_err(err)?.read_to_end(&mut buf).map_err(err)?;
    if buf.len() < 84 {
        return Err("STL too short".into());
    }
    if buf.starts_with(b"solid") && !buf[..80.min(buf.len())].contains(&0) {
        return Err("ASCII STL is not supported, re-export as binary".into());
    }
    let count = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]) as usize;
    if buf.len() < 84 + count * 50 {
        return Err("STL truncated".into());
    }
    let rd = |o: usize| f32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);

    let mut pos = Vec::with_capacity(count * 3);
    let mut faces = Vec::with_capacity(count);
    for i in 0..count {
        let base = 84 + i * 50 + 12; // skip the per-facet normal
        for k in 0..3 {
            let o = base + k * 12;
            pos.push(Vec3::new(rd(o), rd(o + 4), rd(o + 8)));
        }
        let b = (i * 3) as u32;
        faces.push([b, b + 1, b + 2]);
    }
    let (p, f) = weld(&pos, &faces, 1e-5);
    Ok(Mesh::from_soup(&p, &f))
}

/// Dispatches on file extension.
pub fn load(path: &Path) -> IoResult<Mesh> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("obj") => read_obj(path),
        Some("stl") => read_stl(path),
        other => Err(format!("unsupported format: {}", other.unwrap_or("(none)"))),
    }
}

pub fn save(mesh: &Mesh, path: &Path) -> IoResult<()> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("obj") => write_obj(mesh, path, true),
        Some("stl") => write_stl(mesh, path),
        other => Err(format!("unsupported format: {}", other.unwrap_or("(none)"))),
    }
}
