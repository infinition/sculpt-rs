//! Mesh import/export: OBJ, PLY, STL and a native scene file.

use crate::brush::{BlendMode, Brush, BrushKind, Falloff, FillScope};
use crate::mesh::{Mesh, Vertex};
use crate::primitives::weld;
use crate::scene::{Object, Scene, Transform};
use glam::{Quat, Vec3};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub type IoResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Extensions accepted by [`load`], for file dialogs.
pub const IMPORT_EXTENSIONS: &[&str] = &["obj", "ply", "stl", "sculpt"];
/// Extensions accepted by [`save`].
pub const EXPORT_EXTENSIONS: &[&str] = &["obj", "ply", "stl", "sculpt"];

// ---------------------------------------------------------------------------
// OBJ
// ---------------------------------------------------------------------------

/// Writes an OBJ. Vertex colours ride along on the `v` lines, an extension
/// MeshLab and Blender both read back.
pub fn write_obj(mesh: &Mesh, path: &Path, with_colors: bool) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "# exported by sculpt-rs").map_err(err)?;
    for v in mesh.vertices() {
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
    for n in &mesh.nrm {
        writeln!(w, "vn {:.6} {:.6} {:.6}", n.x, n.y, n.z).map_err(err)?;
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
    for (i, c) in cols.into_iter().enumerate() {
        mesh.set_col(i as u32, c);
    }
    Ok(mesh)
}

// ---------------------------------------------------------------------------
// PLY
// ---------------------------------------------------------------------------

/// Writes a binary little-endian PLY with positions, normals and colours.
pub fn write_ply(mesh: &Mesh, path: &Path) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "ply").map_err(err)?;
    writeln!(w, "format binary_little_endian 1.0").map_err(err)?;
    writeln!(w, "comment created by sculpt-rs").map_err(err)?;
    writeln!(w, "element vertex {}", mesh.pos.len()).map_err(err)?;
    for p in ["x", "y", "z", "nx", "ny", "nz"] {
        writeln!(w, "property float {p}").map_err(err)?;
    }
    for p in ["red", "green", "blue"] {
        writeln!(w, "property uchar {p}").map_err(err)?;
    }
    writeln!(w, "element face {}", mesh.faces.len()).map_err(err)?;
    writeln!(w, "property list uchar uint vertex_indices").map_err(err)?;
    writeln!(w, "end_header").map_err(err)?;

    for v in mesh.vertices() {
        for c in [v.pos.x, v.pos.y, v.pos.z, v.nrm.x, v.nrm.y, v.nrm.z] {
            w.write_all(&c.to_le_bytes()).map_err(err)?;
        }
        for c in [v.col.x, v.col.y, v.col.z] {
            w.write_all(&[(c.clamp(0.0, 1.0) * 255.0).round() as u8]).map_err(err)?;
        }
    }
    for t in &mesh.faces {
        w.write_all(&[3u8]).map_err(err)?;
        for &i in t {
            w.write_all(&i.to_le_bytes()).map_err(err)?;
        }
    }
    w.flush().map_err(err)?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum PlyType {
    F32,
    F64,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
}

impl PlyType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "float" | "float32" => PlyType::F32,
            "double" | "float64" => PlyType::F64,
            "uchar" | "uint8" => PlyType::U8,
            "char" | "int8" => PlyType::I8,
            "ushort" | "uint16" => PlyType::U16,
            "short" | "int16" => PlyType::I16,
            "uint" | "uint32" => PlyType::U32,
            "int" | "int32" => PlyType::I32,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            PlyType::U8 | PlyType::I8 => 1,
            PlyType::U16 | PlyType::I16 => 2,
            PlyType::F32 | PlyType::U32 | PlyType::I32 => 4,
            PlyType::F64 => 8,
        }
    }

    fn read(self, b: &[u8], o: usize, le: bool) -> f64 {
        macro_rules! num {
            ($t:ty, $n:expr) => {{
                let mut a = [0u8; $n];
                a.copy_from_slice(&b[o..o + $n]);
                if le {
                    <$t>::from_le_bytes(a) as f64
                } else {
                    <$t>::from_be_bytes(a) as f64
                }
            }};
        }
        match self {
            PlyType::U8 => b[o] as f64,
            PlyType::I8 => b[o] as i8 as f64,
            PlyType::U16 => num!(u16, 2),
            PlyType::I16 => num!(i16, 2),
            PlyType::U32 => num!(u32, 4),
            PlyType::I32 => num!(i32, 4),
            PlyType::F32 => num!(f32, 4),
            PlyType::F64 => num!(f64, 8),
        }
    }
}

struct PlyProp {
    name: String,
    ty: PlyType,
    /// `Some(count_type)` when the property is a list.
    list: Option<PlyType>,
}

/// Reads ASCII or binary PLY, keeping positions and colours.
pub fn read_ply(path: &Path) -> IoResult<Mesh> {
    let mut buf = Vec::new();
    File::open(path).map_err(err)?.read_to_end(&mut buf).map_err(err)?;

    let header_end = find_subslice(&buf, b"end_header")
        .ok_or_else(|| "PLY header not terminated".to_string())?;
    let after = header_end + b"end_header".len();
    let body_start = after + if buf.get(after) == Some(&b'\r') { 2 } else { 1 };
    let header = String::from_utf8_lossy(&buf[..header_end]).to_string();

    let mut format = "ascii".to_string();
    let mut elements: Vec<(String, usize, Vec<PlyProp>)> = Vec::new();
    for line in header.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("format") => format = it.next().unwrap_or("ascii").to_string(),
            Some("element") => {
                let name = it.next().unwrap_or("").to_string();
                let count = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                elements.push((name, count, Vec::new()));
            }
            Some("property") => {
                let Some(last) = elements.last_mut() else { continue };
                let first = it.next().unwrap_or("");
                if first == "list" {
                    let count_ty = PlyType::parse(it.next().unwrap_or("")).unwrap_or(PlyType::U8);
                    let ty = PlyType::parse(it.next().unwrap_or("")).unwrap_or(PlyType::U32);
                    let name = it.next().unwrap_or("").to_string();
                    last.2.push(PlyProp { name, ty, list: Some(count_ty) });
                } else {
                    let ty = PlyType::parse(first).unwrap_or(PlyType::F32);
                    let name = it.next().unwrap_or("").to_string();
                    last.2.push(PlyProp { name, ty, list: None });
                }
            }
            _ => {}
        }
    }

    let mut pos: Vec<Vec3> = Vec::new();
    let mut cols: Vec<Vec3> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();

    if format == "ascii" {
        let body = String::from_utf8_lossy(&buf[body_start..]).to_string();
        let mut tokens = body.split_whitespace();
        for (name, count, props) in &elements {
            for _ in 0..*count {
                let mut vals: Vec<f64> = Vec::new();
                let mut list_vals: Vec<u32> = Vec::new();
                for p in props {
                    if p.list.is_some() {
                        let n: usize = tokens.next().and_then(|t| t.parse().ok()).unwrap_or(0);
                        for _ in 0..n {
                            list_vals.push(tokens.next().and_then(|t| t.parse().ok()).unwrap_or(0));
                        }
                    } else {
                        vals.push(tokens.next().and_then(|t| t.parse().ok()).unwrap_or(0.0));
                    }
                }
                collect(name, props, &vals, &list_vals, &mut pos, &mut cols, &mut faces);
            }
        }
    } else {
        let le = format.contains("little");
        let mut o = body_start;
        for (name, count, props) in &elements {
            for _ in 0..*count {
                let mut vals: Vec<f64> = Vec::new();
                let mut list_vals: Vec<u32> = Vec::new();
                for p in props {
                    if let Some(ct) = p.list {
                        if o + ct.size() > buf.len() {
                            return Err("PLY truncated".into());
                        }
                        let n = ct.read(&buf, o, le) as usize;
                        o += ct.size();
                        for _ in 0..n {
                            if o + p.ty.size() > buf.len() {
                                return Err("PLY truncated".into());
                            }
                            list_vals.push(p.ty.read(&buf, o, le) as u32);
                            o += p.ty.size();
                        }
                    } else {
                        if o + p.ty.size() > buf.len() {
                            return Err("PLY truncated".into());
                        }
                        vals.push(p.ty.read(&buf, o, le));
                        o += p.ty.size();
                    }
                }
                collect(name, props, &vals, &list_vals, &mut pos, &mut cols, &mut faces);
            }
        }
    }

    if pos.is_empty() || faces.is_empty() {
        return Err("no geometry found in PLY".into());
    }
    let n = pos.len() as u32;
    faces.retain(|t| t.iter().all(|&i| i < n));
    let mut mesh = Mesh::from_soup(&pos, &faces);
    for (i, c) in cols.into_iter().enumerate() {
        mesh.set_col(i as u32, c);
    }
    Ok(mesh)
}

/// Turns one decoded PLY element into geometry.
fn collect(
    element: &str,
    props: &[PlyProp],
    vals: &[f64],
    list_vals: &[u32],
    pos: &mut Vec<Vec3>,
    cols: &mut Vec<Vec3>,
    faces: &mut Vec<[u32; 3]>,
) {
    if element == "vertex" {
        // `vals` holds the scalar properties in declaration order.
        let scalars: Vec<&PlyProp> = props.iter().filter(|p| p.list.is_none()).collect();
        let get = |want: &str| -> Option<f64> {
            scalars
                .iter()
                .position(|p| p.name == want)
                .and_then(|i| vals.get(i).copied())
        };
        pos.push(Vec3::new(
            get("x").unwrap_or(0.0) as f32,
            get("y").unwrap_or(0.0) as f32,
            get("z").unwrap_or(0.0) as f32,
        ));

        // Colours are bytes in most files but floats in some; normalise both.
        let channel = |name: &str| -> f32 {
            let Some(raw) = get(name) else { return 0.85 };
            let float = scalars
                .iter()
                .find(|p| p.name == name)
                .map(|p| matches!(p.ty, PlyType::F32 | PlyType::F64))
                .unwrap_or(false);
            if float { raw as f32 } else { raw as f32 / 255.0 }
        };
        cols.push(Vec3::new(channel("red"), channel("green"), channel("blue")));
    } else if element == "face" && list_vals.len() >= 3 {
        for k in 1..list_vals.len() - 1 {
            faces.push([list_vals[0], list_vals[k], list_vals[k + 1]]);
        }
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// STL
// ---------------------------------------------------------------------------

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
            let p = mesh.pos[v as usize];
            for c in [p.x, p.y, p.z] {
                w.write_all(&c.to_le_bytes()).map_err(err)?;
            }
        }
        w.write_all(&0u16.to_le_bytes()).map_err(err)?;
    }
    w.flush().map_err(err)?;
    Ok(())
}

/// Reads binary or ASCII STL and welds the soup back into a connected mesh.
pub fn read_stl(path: &Path) -> IoResult<Mesh> {
    let mut buf = Vec::new();
    File::open(path).map_err(err)?.read_to_end(&mut buf).map_err(err)?;
    if buf.len() < 15 {
        return Err("STL too short".into());
    }

    let looks_ascii = buf.starts_with(b"solid")
        && find_subslice(&buf[..buf.len().min(512)], b"facet").is_some();
    let (pos, faces) = if looks_ascii {
        read_stl_ascii(&buf)?
    } else {
        read_stl_binary(&buf)?
    };
    let (p, f) = weld(&pos, &faces, 1e-5);
    Ok(Mesh::from_soup(&p, &f))
}

fn read_stl_binary(buf: &[u8]) -> IoResult<(Vec<Vec3>, Vec<[u32; 3]>)> {
    if buf.len() < 84 {
        return Err("STL too short".into());
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
    Ok((pos, faces))
}

fn read_stl_ascii(buf: &[u8]) -> IoResult<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let text = String::from_utf8_lossy(buf);
    let mut pos = Vec::new();
    let mut faces = Vec::new();
    let mut pending: Vec<Vec3> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if it.next() != Some("vertex") {
            continue;
        }
        let c: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
        if c.len() < 3 {
            continue;
        }
        pending.push(Vec3::new(c[0], c[1], c[2]));
        if pending.len() == 3 {
            let b = pos.len() as u32;
            pos.extend(pending.drain(..));
            faces.push([b, b + 1, b + 2]);
        }
    }
    if faces.is_empty() {
        return Err("no facets found in ASCII STL".into());
    }
    Ok((pos, faces))
}

// ---------------------------------------------------------------------------
// Native scene format
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 8] = b"SCULPTRS";
/// What we write. Version 1 is still read.
const VERSION: u32 = 2;

/// One channel of one mesh, on disk.
///
/// The uncompressed size is never stored: it is the count times the size of an
/// element, both of which the reader already knows. Only what the block cost
/// after compression is written, which is what lets a dormant channel take
/// four bytes and a compressible one take what it deserves.
mod block {
    /// The compressed bytes follow.
    pub const COMPRESSED: u8 = 1 << 0;
    /// Nothing follows: every entry is the channel's default.
    pub const DORMANT: u8 = 1 << 1;
}

/// Writes one channel, compressed, or writes that there is nothing to write.
///
/// Compression is skipped when it does not pay. Positions are floats whose low
/// bits are noise and they barely shrink, so paying to compress and then to
/// decompress a hundred megabytes of them on every open would be a poor trade;
/// face indices and quantised channels shrink a great deal.
fn write_block<W: Write>(w: &mut W, raw: Option<&[u8]>) -> IoResult<()> {
    let Some(raw) = raw else {
        w.write_all(&[block::DORMANT]).map_err(err)?;
        w.write_all(&0u32.to_le_bytes()).map_err(err)?;
        return Ok(());
    };
    let packed = lz4_flex::block::compress(raw);
    // A tenth off is the least worth the time it takes to unpack again.
    if packed.len() < raw.len() - raw.len() / 10 {
        w.write_all(&[block::COMPRESSED]).map_err(err)?;
        w.write_all(&(packed.len() as u32).to_le_bytes()).map_err(err)?;
        w.write_all(&packed).map_err(err)?;
    } else {
        w.write_all(&[0u8]).map_err(err)?;
        w.write_all(&(raw.len() as u32).to_le_bytes()).map_err(err)?;
        w.write_all(raw).map_err(err)?;
    }
    Ok(())
}

/// Reads one channel back, or reports that it was never written.
///
/// `expect` is what the caller knows the block has to expand to, so a file that
/// claims otherwise is refused here rather than corrupting a mesh.
fn read_block(buf: &[u8], o: &mut usize, expect: usize) -> IoResult<Option<Vec<u8>>> {
    if *o + 5 > buf.len() {
        return Err("scene file truncated in a channel header".into());
    }
    let flags = buf[*o];
    *o += 1;
    let len = u32::from_le_bytes([buf[*o], buf[*o + 1], buf[*o + 2], buf[*o + 3]]) as usize;
    *o += 4;
    if flags & block::DORMANT != 0 {
        return Ok(None);
    }
    if *o + len > buf.len() {
        return Err("scene file truncated in a channel".into());
    }
    let raw = &buf[*o..*o + len];
    *o += len;
    let out = if flags & block::COMPRESSED != 0 {
        lz4_flex::block::decompress(raw, expect)
            .map_err(|e| format!("a channel would not decompress: {e}"))?
    } else {
        raw.to_vec()
    };
    if out.len() != expect {
        return Err(format!(
            "a channel expanded to {} bytes where {expect} were expected",
            out.len()
        ));
    }
    Ok(Some(out))
}

/// Writes the whole scene, transforms and per-vertex attributes included.
pub fn write_scene(scene: &Scene, path: &Path) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    w.write_all(MAGIC).map_err(err)?;
    w.write_all(&VERSION.to_le_bytes()).map_err(err)?;
    w.write_all(&(scene.objects.len() as u32).to_le_bytes()).map_err(err)?;
    w.write_all(&(scene.active as u32).to_le_bytes()).map_err(err)?;

    for o in &scene.objects {
        let name = o.name.as_bytes();
        w.write_all(&(name.len() as u32).to_le_bytes()).map_err(err)?;
        w.write_all(name).map_err(err)?;
        let t = &o.transform;
        for c in [
            t.position.x, t.position.y, t.position.z,
            t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w,
            t.scale.x, t.scale.y, t.scale.z,
        ] {
            w.write_all(&c.to_le_bytes()).map_err(err)?;
        }
        w.write_all(&[o.visible as u8]).map_err(err)?;
        w.write_all(&(o.mesh.pos.len() as u32).to_le_bytes()).map_err(err)?;
        w.write_all(&(o.mesh.faces.len() as u32).to_le_bytes()).map_err(err)?;
        // Channel by channel, in the order the reader expects them, each one
        // compressed on its own or written as nothing at all. A model nobody
        // has painted writes four bytes for its colour, four for its mask and
        // four for each material value.
        let m = &o.mesh;
        write_block(&mut w, Some(bytemuck::cast_slice(&m.pos)))?;
        write_block(&mut w, Some(bytemuck::cast_slice(&m.nrm)))?;
        write_block(&mut w, m.col_channel().as_slice().map(bytemuck::cast_slice))?;
        write_block(&mut w, m.mask_channel().as_slice().map(bytemuck::cast_slice))?;
        write_block(&mut w, m.rough_channel().as_slice())?;
        write_block(&mut w, m.metal_channel().as_slice())?;
        write_block(&mut w, Some(bytemuck::cast_slice(&m.faces)))?;
    }
    w.flush().map_err(err)?;
    Ok(())
}

pub fn read_scene(path: &Path) -> IoResult<Scene> {
    let mut buf = Vec::new();
    File::open(path).map_err(err)?.read_to_end(&mut buf).map_err(err)?;
    if buf.len() < 16 || &buf[..8] != MAGIC {
        return Err("not a sculpt-rs scene file".into());
    }
    let mut o = 8;
    let u32_at = |o: &mut usize| -> u32 {
        let v = u32::from_le_bytes([buf[*o], buf[*o + 1], buf[*o + 2], buf[*o + 3]]);
        *o += 4;
        v
    };
    let version = u32_at(&mut o);
    if version != 1 && version != VERSION {
        return Err(format!("unsupported scene version {version}"));
    }
    let count = u32_at(&mut o) as usize;
    let active = u32_at(&mut o) as usize;

    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = u32_at(&mut o) as usize;
        if o + name_len > buf.len() {
            return Err("scene file truncated".into());
        }
        let name = String::from_utf8_lossy(&buf[o..o + name_len]).to_string();
        o += name_len;

        let f32_at = |o: &mut usize| -> f32 {
            let v = f32::from_le_bytes([buf[*o], buf[*o + 1], buf[*o + 2], buf[*o + 3]]);
            *o += 4;
            v
        };
        let position = Vec3::new(f32_at(&mut o), f32_at(&mut o), f32_at(&mut o));
        let rotation = Quat::from_xyzw(
            f32_at(&mut o),
            f32_at(&mut o),
            f32_at(&mut o),
            f32_at(&mut o),
        );
        let scale = Vec3::new(f32_at(&mut o), f32_at(&mut o), f32_at(&mut o));
        let visible = buf[o] != 0;
        o += 1;

        let nv = u32_at(&mut o) as usize;
        let nf = u32_at(&mut o) as usize;
        let vbytes = nv * std::mem::size_of::<Vertex>();
        let fbytes = nf * std::mem::size_of::<[u32; 3]>();

        // Nothing here is aligned: the payload starts wherever the
        // variable-length name left off, so every array is read element by
        // element rather than cast in place.
        let unpack = |b: &[u8]| -> Vec<[u32; 3]> {
            b.chunks_exact(12).map(bytemuck::pod_read_unaligned).collect()
        };
        let mesh = if version == 1 {
            if o + vbytes + fbytes > buf.len() {
                return Err("scene file truncated".into());
            }
            let verts: Vec<Vertex> = buf[o..o + vbytes]
                .chunks_exact(std::mem::size_of::<Vertex>())
                .map(bytemuck::pod_read_unaligned)
                .collect();
            o += vbytes;
            let faces = unpack(&buf[o..o + fbytes]);
            o += fbytes;
            let mut mesh =
                Mesh::from_soup(&verts.iter().map(|v| v.pos).collect::<Vec<_>>(), &faces);
            for (i, v) in verts.iter().enumerate() {
                mesh.write_vertex(i as u32, v);
            }
            mesh
        } else {
            let read3 = |b: &[u8]| -> Vec<Vec3> {
                b.chunks_exact(12)
                    .map(|c| {
                        let a: [f32; 3] = bytemuck::pod_read_unaligned(c);
                        Vec3::from(a)
                    })
                    .collect()
            };
            let pos = read_block(&buf, &mut o, nv * 12)?
                .ok_or_else(|| "a scene without positions".to_string())?;
            let nrm = read_block(&buf, &mut o, nv * 12)?;
            let col = read_block(&buf, &mut o, nv * 3)?;
            let mask = read_block(&buf, &mut o, nv * 2)?;
            let rough = read_block(&buf, &mut o, nv)?;
            let metal = read_block(&buf, &mut o, nv)?;
            let faces = read_block(&buf, &mut o, fbytes)?
                .ok_or_else(|| "a scene without faces".to_string())?;

            let mut mesh = Mesh::from_soup(&read3(&pos), &unpack(&faces));
            if let Some(n) = nrm {
                mesh.nrm = read3(&n);
            }
            // A channel that was never written stays asleep, which is the whole
            // point: a model nobody painted costs nothing to hold.
            if let Some(c) = col {
                mesh.col_channel_mut()
                    .make_mut()
                    .copy_from_slice(&c.chunks_exact(3).map(|x| [x[0], x[1], x[2]]).collect::<Vec<_>>());
            }
            if let Some(m) = mask {
                let v: Vec<u16> = m
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                mesh.mask_channel_mut().make_mut().copy_from_slice(&v);
            }
            if let Some(r) = rough {
                mesh.rough_channel_mut().make_mut().copy_from_slice(&r);
            }
            if let Some(m) = metal {
                mesh.metal_channel_mut().make_mut().copy_from_slice(&m);
            }
            mesh
        };
        let mut obj = Object::new(name, mesh);
        obj.transform = Transform { position, rotation, scale };
        obj.visible = visible;
        objects.push(obj);
    }

    Ok(Scene { active: active.min(objects.len().saturating_sub(1)), objects })
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn ext(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Loads a single mesh. Scene files load their first object.
pub fn load(path: &Path) -> IoResult<Mesh> {
    match ext(path).as_deref() {
        Some("obj") => read_obj(path),
        Some("ply") => read_ply(path),
        Some("stl") => read_stl(path),
        Some("sculpt") => read_scene(path)?
            .objects
            .into_iter()
            .next()
            .map(|o| o.mesh)
            .ok_or_else(|| "scene file has no objects".to_string()),
        other => Err(format!("unsupported format: {}", other.unwrap_or("(none)"))),
    }
}

pub fn save(mesh: &Mesh, path: &Path) -> IoResult<()> {
    match ext(path).as_deref() {
        Some("obj") => write_obj(mesh, path, true),
        Some("ply") => write_ply(mesh, path),
        Some("stl") => write_stl(mesh, path),
        Some("sculpt") => write_scene(&Scene::with_object(Object::new("mesh", mesh.clone())), path),
        other => Err(format!("unsupported format: {}", other.unwrap_or("(none)"))),
    }
}

// ---------------------------------------------------------------------------
// brush presets
// ---------------------------------------------------------------------------

/// A brush someone set up and gave a name to.
#[derive(Clone, Debug)]
pub struct NamedBrush {
    pub name: String,
    pub brush: Brush,
}

/// Extension for a saved set of brushes.
pub const BRUSH_EXTENSION: &str = "brushes";

/// Writes a set of brushes as plain text, one `key value` line per setting.
///
/// Text rather than a packed format on purpose: a brush is a handful of
/// numbers, the file is something an artist may well want to read, diff or fix
/// by hand, and a format nobody can open is a format nobody trusts.
///
/// The alpha is stored by name. Indices mean nothing across sessions, since the
/// library is rebuilt from whatever has been loaded this time.
pub fn write_brushes(brushes: &[NamedBrush], alphas: &[String], path: &Path) -> IoResult<()> {
    let f = File::create(path).map_err(err)?;
    let mut w = BufWriter::new(f);
    writeln!(w, "sculpt-brushes 1").map_err(err)?;
    for nb in brushes {
        let b = &nb.brush;
        // Names run to the end of their line and are never quoted, so a newline
        // in one would split the file. Nothing else can get in.
        writeln!(w, "brush {}", nb.name.replace(['\n', '\r'], " ")).map_err(err)?;
        writeln!(w, "  kind {}", b.kind.label()).map_err(err)?;
        writeln!(w, "  radius {:.6}", b.radius).map_err(err)?;
        writeln!(w, "  strength {:.6}", b.strength).map_err(err)?;
        writeln!(w, "  negative {}", b.negative).map_err(err)?;
        writeln!(w, "  falloff {}", b.falloff.label()).map_err(err)?;
        writeln!(w, "  culling {}", b.culling).map_err(err)?;
        writeln!(w, "  lock_plane {}", b.lock_plane).map_err(err)?;
        writeln!(w, "  auto_smooth {:.6}", b.auto_smooth).map_err(err)?;
        writeln!(w, "  pressure_radius {}", b.pressure_radius).map_err(err)?;
        writeln!(w, "  pressure_strength {}", b.pressure_strength).map_err(err)?;
        writeln!(
            w,
            "  color {:.6} {:.6} {:.6}",
            b.paint_color.x, b.paint_color.y, b.paint_color.z
        )
        .map_err(err)?;
        writeln!(w, "  rough {:.6}", b.paint_rough).map_err(err)?;
        writeln!(w, "  metal {:.6}", b.paint_metal).map_err(err)?;
        writeln!(w, "  paint_albedo {}", b.paint_albedo).map_err(err)?;
        writeln!(w, "  paint_material {}", b.paint_material).map_err(err)?;
        writeln!(w, "  blend {}", b.blend.label()).map_err(err)?;
        writeln!(w, "  flow {:.6}", b.flow).map_err(err)?;
        writeln!(w, "  fill_scope {}", b.fill_scope.label()).map_err(err)?;
        writeln!(w, "  fill_angle {:.6}", b.fill_angle).map_err(err)?;
        writeln!(w, "  smudge_pickup {:.6}", b.smudge_pickup).map_err(err)?;
        writeln!(w, "  alpha_angle {:.6}", b.alpha_angle).map_err(err)?;
        writeln!(w, "  alpha_follow {}", b.alpha_follow).map_err(err)?;
        if let Some(name) = b.alpha.and_then(|i| alphas.get(i as usize)) {
            writeln!(w, "  alpha {}", name.replace(['\n', '\r'], " ")).map_err(err)?;
        }
        writeln!(w, "end").map_err(err)?;
    }
    w.flush().map_err(err)?;
    Ok(())
}

/// Reads a set of brushes back.
///
/// `alphas` is the current library, by name, so a brush finds its stamp again.
/// A brush whose alpha is not loaded keeps everything else and comes back
/// round: losing a whole brush because one image is missing would be worse than
/// losing the image.
///
/// Unknown keys are skipped rather than refused, so a file written by a later
/// version still opens with whatever this one understands.
pub fn read_brushes(path: &Path, alphas: &[String]) -> IoResult<Vec<NamedBrush>> {
    let f = File::open(path).map_err(err)?;
    let reader = BufReader::new(f);
    let mut out: Vec<NamedBrush> = Vec::new();
    let mut current: Option<NamedBrush> = None;

    for line in reader.lines() {
        let line = line.map_err(err)?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        match key {
            "sculpt-brushes" => continue,
            "brush" => {
                if let Some(nb) = current.take() {
                    out.push(nb);
                }
                current = Some(NamedBrush {
                    name: if rest.is_empty() { "Brush".to_string() } else { rest.to_string() },
                    brush: Brush::default(),
                });
            }
            "end" => {
                if let Some(nb) = current.take() {
                    out.push(nb);
                }
            }
            _ => {
                let Some(nb) = current.as_mut() else { continue };
                let b = &mut nb.brush;
                let num = || rest.parse::<f32>().ok();
                let flag = || matches!(rest, "true" | "1" | "yes");
                match key {
                    "kind" => {
                        if let Some(k) = BrushKind::from_label(rest) {
                            b.kind = k;
                        }
                    }
                    "radius" => b.radius = num().unwrap_or(b.radius),
                    "strength" => b.strength = num().unwrap_or(b.strength),
                    "negative" => b.negative = flag(),
                    "falloff" => {
                        if let Some(f) = Falloff::from_label(rest) {
                            b.falloff = f;
                        }
                    }
                    "culling" => b.culling = flag(),
                    "lock_plane" => b.lock_plane = flag(),
                    "auto_smooth" => b.auto_smooth = num().unwrap_or(b.auto_smooth),
                    "pressure_radius" => b.pressure_radius = flag(),
                    "pressure_strength" => b.pressure_strength = flag(),
                    "color" => {
                        let v: Vec<f32> =
                            rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                        if v.len() == 3 {
                            b.paint_color = Vec3::new(v[0], v[1], v[2]);
                        }
                    }
                    "rough" => b.paint_rough = num().unwrap_or(b.paint_rough),
                    "metal" => b.paint_metal = num().unwrap_or(b.paint_metal),
                    "paint_albedo" => b.paint_albedo = flag(),
                    "paint_material" => b.paint_material = flag(),
                    "blend" => {
                        if let Some(m) = BlendMode::from_label(rest) {
                            b.blend = m;
                        }
                    }
                    "flow" => b.flow = num().unwrap_or(b.flow),
                    "fill_scope" => {
                        if let Some(s) = FillScope::from_label(rest) {
                            b.fill_scope = s;
                        }
                    }
                    "fill_angle" => b.fill_angle = num().unwrap_or(b.fill_angle),
                    "smudge_pickup" => b.smudge_pickup = num().unwrap_or(b.smudge_pickup),
                    "alpha_angle" => b.alpha_angle = num().unwrap_or(b.alpha_angle),
                    "alpha_follow" => b.alpha_follow = flag(),
                    "alpha" => b.alpha = alphas.iter().position(|n| n == rest).map(|i| i as u32),
                    _ => {}
                }
            }
        }
    }
    if let Some(nb) = current.take() {
        out.push(nb);
    }
    Ok(out)
}

#[cfg(test)]
mod scene_format_tests {
    use super::*;
    use crate::primitives;

    /// Un canal endormi doit coûter presque rien sur le disque, et un maillage
    /// peint doit revenir intact.
    #[test]
    fn the_scene_file_leaves_out_what_nobody_touched() {
        let dir = std::env::temp_dir().join("sculpt-rs-tests");
        std::fs::create_dir_all(&dir).unwrap();

        let plain = primitives::icosphere(4);
        let n = plain.vert_count();
        let mut painted = plain.clone();
        for v in 0..n as u32 {
            painted.set_col(v, Vec3::new(0.9, 0.1, 0.1));
            painted.set_mask(v, 0.5);
        }

        let a = dir.join("plain.sculpt");
        let b = dir.join("painted.sculpt");
        write_scene(&Scene::with_object(Object::new("a", plain)), &a).unwrap();
        write_scene(&Scene::with_object(Object::new("b", painted.clone())), &b).unwrap();

        let plain_size = std::fs::metadata(&a).unwrap().len();
        let painted_size = std::fs::metadata(&b).unwrap().len();
        assert!(
            plain_size < painted_size,
            "un maillage non peint ({plain_size}) devrait peser moins qu'un peint ({painted_size})"
        );
        // Douze octets de position et douze de normale par sommet, plus les
        // faces: un canal absent ne doit rien ajouter de sensible.
        assert!(
            plain_size < (n * 25 + plain_size as usize / 2) as u64,
            "le fichier non peint est trop gros: {plain_size}"
        );

        // Et l'aller-retour rend ce qu'on a mis.
        let back = read_scene(&b).unwrap();
        let m = &back.objects[0].mesh;
        assert_eq!(m.vert_count(), n);
        assert!(m.col(3).distance(Vec3::new(0.9, 0.1, 0.1)) < 0.01);
        assert!((m.mask(3) - 0.5).abs() < 0.001);
        assert!(m.pos[3].distance(painted.pos[3]) < 1e-6);

        // Le maillage non peint revient avec ses canaux toujours endormis.
        let back = read_scene(&a).unwrap();
        for (name, awake) in back.objects[0].mesh.awake_channels() {
            assert!(!awake, "le canal {name} s'est réveillé à la relecture");
        }

        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// Un fichier tronqué doit être refusé avec un message, jamais paniquer.
    #[test]
    fn a_broken_scene_file_is_refused_not_fatal() {
        let dir = std::env::temp_dir().join("sculpt-rs-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("broken.sculpt");
        write_scene(
            &Scene::with_object(Object::new("x", primitives::icosphere(2))),
            &p,
        )
        .unwrap();
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.truncate(bytes.len() * 2 / 3);
        std::fs::write(&p, &bytes).unwrap();
        assert!(read_scene(&p).is_err(), "un fichier coupé a été accepté");
        let _ = std::fs::remove_file(&p);
    }
}
