//! Dessine une image sans ouvrir de fenêtre, et vérifie ce qu'elle contient.
//!
//! `cargo run --release -p sculpt-app --example offscreen`
//!
//! Le reste des tests s'arrête au bord de la carte: ils vérifient le maillage,
//! pas ce qui en est affiché. Or une erreur dans la disposition d'un sommet ne
//! se voit qu'à l'exécution, et sur ce chemin la fenêtre se ferme sans rien
//! écrire. Ici les pipelines sont réellement construits, une sphère est
//! réellement dessinée, et les pixels sont relus et contrôlés.
//!
//! Quatre contrôles. En mode normales, le pixel du centre doit porter la
//! normale qui regarde la caméra: c'est le décodage octaédrique qui est vérifié
//! là, et une erreur y donne une couleur quelconque. En mode matcap, le centre
//! doit être nettement plus clair que le coin, ce qui dit que la sphère est
//! arrivée là où on l'attendait. Puis un coup de brosse et un coup de peinture
//! passés par la mise à jour creuse, qui est le chemin de tous les traits: le
//! premier ne touche que le flux chaud, le second que le froid, et chacun doit
//! se voir sans déranger l'autre.

#![allow(dead_code)]

#[path = "../src/gpu_vertex.rs"]
mod gpu_vertex;
#[path = "../src/matcap.rs"]
mod matcap;
#[path = "../src/renderer.rs"]
mod renderer;

use glam::{Mat4, Vec3};
use renderer::{FrameSettings, Renderer, Shading};
use sculpt_core::{primitives, BrushKind, Object, Scene, Sculptor, StrokeInput};

const SIZE: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn main() {
    let instance = wgpu::Instance::new(
        wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
    );
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("aucune carte utilisable");
    println!("carte: {}", adapter.get_info().name);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("offscreen"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("device refusé");
    // Une erreur de validation tue le processus là où elle arrive, sans rien
    // dire. Celle-ci aura un nom.
    device.on_uncaptured_error(std::sync::Arc::new(|e| {
        eprintln!("\n=== erreur GPU ===\n{e}\n");
        std::process::exit(2);
    }));

    let mut r = Renderer::new(&device, &queue, FORMAT, SIZE, SIZE, false, 1);
    let scene = Scene::with_object(Object::new("sphere", primitives::icosphere(5)));
    r.sync(&device, &queue, &scene, None);

    // Caméra sur +Z qui regarde l'origine, sphère de rayon 1.
    let eye = Vec3::new(0.0, 0.0, 3.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(0.9, 1.0, 0.05, 100.0);

    let normals = draw(&device, &queue, &r, &scene, Shading::Normals, view, proj, eye);
    let matcap = draw(&device, &queue, &r, &scene, Shading::Matcap, view, proj, eye);

    let centre = pixel(&normals, SIZE / 2, SIZE / 2);
    println!("centre en mode normales: {centre:?}   (attendu proche de [196, 196, 255])");
    let (rr, gg, bb) = (centre[0] as i32, centre[1] as i32, centre[2] as i32);
    // Au centre d'une sphère vue de face la normale monde vaut +Z, donc
    // l'encodage n * 0.5 + 0.5 donne (0.5, 0.5, 1.0), soit environ 196, 196,
    // 255 une fois passé en sRGB par la carte.
    check(bb >= 235, "le bleu du centre devrait être saturé, la normale ne regarde pas la caméra");
    check((rr - gg).abs() <= 25, "rouge et vert devraient être proches au centre");
    check((160..=225).contains(&rr), "le rouge du centre est hors de ce qu'une normale +Z donne");

    let centre = pixel(&matcap, SIZE / 2, SIZE / 2);
    let coin = pixel(&matcap, 3, 3);
    let lum = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
    println!("centre en matcap: {centre:?}   coin: {coin:?}");
    check(
        lum(centre) > lum(coin) + 120,
        "le centre n'est pas plus clair que le fond, rien n'a été dessiné là",
    );

    // Le chemin creux, celui qu'emprunte chaque coup de brosse: seuls les
    // sommets touchés partent, et une passe de calcul les range.
    let mut s = Sculptor::with_scene(scene);
    s.symmetry = false;
    s.dyntopo_enabled = false;
    // Un maillage neuf se déclare entièrement changé, ce qu'il est: la sync
    // complète plus haut s'en est chargée, et la marque est consommée ici pour
    // que ce qui suit ne parle que du trait.
    let _ = s.take_dirty();

    // Un coup de brosse au pôle avant. Il ne touche que le flux chaud.
    s.brush.kind = BrushKind::Draw;
    s.brush.radius = 0.35;
    s.brush.strength = 1.0;
    s.begin_stroke();
    s.stroke(&StrokeInput { point: Vec3::Z, normal: Vec3::Z, ..Default::default() });
    s.end_stroke();
    let sculpted = sparse(&device, &queue, &mut r, &mut s, Shading::Normals, view, proj, eye);
    // Au sommet du renflement la normale regarde toujours la caméra, alors le
    // pixel du centre bouge à peine. C'est sur le flanc que la pente change, et
    // c'est pourquoi on compte les pixels plutôt que d'en regarder un.
    let bougés = normals
        .chunks_exact(4)
        .zip(sculpted.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 4))
        .count();
    println!("pixels changés par le coup de brosse: {bougés}");
    check(
        bougés > 500,
        "le coup de brosse n'est pas arrivé sur la carte par la mise à jour creuse",
    );

    // Et de la peinture au même endroit, qui ne touche que le flux froid.
    s.brush.kind = BrushKind::Paint;
    s.brush.paint_color = Vec3::new(1.0, 0.0, 0.0);
    s.brush.strength = 1.0;
    s.begin_stroke();
    for _ in 0..6 {
        s.stroke(&StrokeInput { point: Vec3::Z, normal: Vec3::Z, ..Default::default() });
    }
    s.end_stroke();
    let peint = sparse(&device, &queue, &mut r, &mut s, Shading::Unlit, view, proj, eye);
    let centre = pixel(&peint, SIZE / 2, SIZE / 2);
    println!("centre peint, en mode sans éclairage: {centre:?}");
    check(
        centre[0] > 200 && centre[1] < 90 && centre[2] < 90,
        "la couleur peinte n'est pas arrivée: le flux froid ne suit pas",
    );

    // La courbe de tonalité. Poussée de trois diaphragmes, une vue éclairée
    // sans courbe part au blanc pur et la forme disparaît; avec, elle doit
    // rester une forme.
    let brûlé = draw_with(
        &device,
        &queue,
        &r,
        &s.scene,
        FrameSettings { shading: Shading::Pbr, grid: false, tone: false, exposure: 3.0, ..Default::default() },
        view,
        proj,
        eye,
    );
    let tenu = draw_with(
        &device,
        &queue,
        &r,
        &s.scene,
        FrameSettings { shading: Shading::Pbr, grid: false, tone: true, exposure: 3.0, ..Default::default() },
        view,
        proj,
        eye,
    );
    let blancs = |img: &[u8]| {
        img.chunks_exact(4)
            .filter(|p| p[0] == 255 && p[1] == 255 && p[2] == 255)
            .count()
    };
    println!(
        "pixels au blanc pur, sans courbe: {}   avec: {}",
        blancs(&brûlé),
        blancs(&tenu)
    );
    check(
        blancs(&brûlé) > 200,
        "la scène de contrôle ne brûle pas, le test ne vérifie rien",
    );
    check(
        blancs(&tenu) * 4 < blancs(&brûlé),
        "la courbe ne retient pas les hautes lumières",
    );

    // L'occlusion. Sur une sphère seule, convexe de partout, rien ne peut
    // s'occulter: elle doit rester claire. Entre deux sphères qui se touchent
    // il y a une gorge, et c'est là qu'elle doit assombrir.
    let seule = Scene::with_object(Object::new("a", primitives::icosphere(5)));
    let mut paire = seule.clone();
    let mut b = Object::new("b", primitives::icosphere(5));
    b.transform.position = Vec3::new(1.45, 0.0, 0.0);
    paire.add(b);

    let sans = ao_render(&device, &queue, &mut r, &seule, false, view, proj, eye);
    let avec = ao_render(&device, &queue, &mut r, &seule, true, view, proj, eye);
    let convexe = darkened_share(&avec, &sans);
    println!("sphère seule, part assombrie: {:.3}", convexe);
    check(
        convexe < 0.02,
        "une sphère convexe est assombrie alors que rien ne l'occulte",
    );

    let eye2 = Vec3::new(0.72, 0.0, 4.2);
    let view2 = glam::camera::rh::view::look_at_mat4(eye2, Vec3::new(0.72, 0.0, 0.0), Vec3::Y);
    let sans = ao_render(&device, &queue, &mut r, &paire, false, view2, proj, eye2);
    let avec = ao_render(&device, &queue, &mut r, &paire, true, view2, proj, eye2);
    let creux = darkened_share(&avec, &sans);
    println!("gorge entre deux sphères, part assombrie: {:.3}", creux);
    // Une gorge est étroite par nature: elle n'occupe qu'un pour cent du
    // modèle visible, et c'est exactement ce qu'on veut. Ce qui compte est
    // qu'elle existe là et nulle part sur le convexe.
    check(
        creux > 0.005 && creux > convexe * 5.0 + 0.004,
        "la gorge entre deux sphères ne s'assombrit pas plus que le convexe",
    );

    // Faces arrière. Sur un volume fermé, chacune est derrière une autre: en
    // les jetant, l'image doit être exactement la même. Si elle ne l'est pas,
    // c'est un trou.
    let gardees = ao_render_with(&device, &queue, &mut r, &seule, false, view, proj, eye);
    let jetees = ao_render_with(&device, &queue, &mut r, &seule, true, view, proj, eye);
    let differents = gardees
        .chunks_exact(4)
        .zip(jetees.chunks_exact(4))
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x != y))
        .count();
    println!("pixels changés en jetant les faces arrière: {differents}");
    check(
        differents == 0,
        "jeter les faces arrière change l'image d'un volume fermé: il y a un trou",
    );

    // Et ce que tout cela coûte réellement, sur une cible assez grande pour
    // que le travail des fragments compte, et un maillage assez dense pour que
    // les triangles descendent sous le pixel. C'est le chiffre qui décide s'il
    // faut un niveau de détail ou pas.
    println!();
    println!("coût d'une image, 1024x1024, sur {}", adapter.get_info().name);
    for level in [7u32, 8, 9] {
        bench(&device, &queue, level);
    }

    println!();
    println!("les pipelines se construisent, la sphère arrive à l'écran, une mise");
    println!("à jour creuse porte le relief comme la couleur, la courbe de");
    println!("tonalité retient ce qui brûlait, et les creux s'assombrissent.");
}

/// La part du modèle que l'occlusion assombrit d'au moins un dixième.
///
/// Une moyenne ne dit rien ici: un creux occupe quelques pour cent de l'image
/// et disparaît dans le reste, qui est convexe et doit rester intact. Ce qu'on
/// veut savoir est s'il existe des pixels réellement assombris, et combien.
fn darkened_share(a: &[u8], b: &[u8]) -> f32 {
    let mut dark = 0u32;
    let mut n = 0u32;
    for (x, y) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        let lb = y[0] as f32 + y[1] as f32 + y[2] as f32;
        // Le fond du dégradé est sombre; on ne compare que le modèle.
        if lb < 200.0 {
            continue;
        }
        let la = x[0] as f32 + x[1] as f32 + x[2] as f32;
        if la / lb < 0.9 {
            dark += 1;
        }
        n += 1;
    }
    if n == 0 { 0.0 } else { dark as f32 / n as f32 }
}

/// Une image du modèle en argile, avec ou sans élimination des faces arrière.
#[allow(clippy::too_many_arguments)]
fn ao_render_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut Renderer,
    scene: &Scene,
    backface_cull: bool,
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
) -> Vec<u8> {
    r.sync(device, queue, scene, None);
    let settings = FrameSettings {
        shading: Shading::Clay,
        grid: false,
        occlusion: false,
        backface_cull,
        ..Default::default()
    };
    r.set_uniforms(queue, scene, view, proj, eye, &settings);
    draw_settings(device, queue, r, scene, &settings)
}

/// Une image du modèle en argile, avec ou sans occlusion.
#[allow(clippy::too_many_arguments)]
fn ao_render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut Renderer,
    scene: &Scene,
    occlusion: bool,
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
) -> Vec<u8> {
    r.sync(device, queue, scene, None);
    let settings = FrameSettings {
        shading: Shading::Clay,
        grid: false,
        occlusion,
        ..Default::default()
    };
    r.set_uniforms(queue, scene, view, proj, eye, &settings);
    draw_settings(device, queue, r, scene, &settings)
}

/// Une image avec des réglages donnés, plutôt que les réglages par défaut.
#[allow(clippy::too_many_arguments)]
fn draw_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &Renderer,
    scene: &Scene,
    settings: FrameSettings,
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
) -> Vec<u8> {
    r.set_uniforms(queue, scene, view, proj, eye, &settings);
    draw_settings(device, queue, r, scene, &settings)
}

/// Envoie ce que le dernier trait a touché, puis dessine.
#[allow(clippy::too_many_arguments)]
fn sparse(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &mut Renderer,
    s: &mut Sculptor,
    shading: Shading,
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
) -> Vec<u8> {
    let (mut verts, mut faces, full) = s.take_dirty();
    check(!full, "le trait a marqué tout le maillage, il n'y a rien de creux à vérifier");
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("creux") });
    let ok = r.sync_sparse(device, queue, &mut encoder, &s.scene, 0, &mut verts, &mut faces);
    check(ok, "la mise à jour creuse a renoncé, le contrôle ne veut plus rien dire");
    queue.submit([encoder.finish()]);
    draw(device, queue, r, &s.scene, shading, view, proj, eye)
}

fn check(ok: bool, why: &str) {
    if !ok {
        eprintln!("ÉCHEC: {why}");
        std::process::exit(1);
    }
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let o = ((y * SIZE + x) * 4) as usize;
    [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
}

/// Une image, rendue puis relue.
#[allow(clippy::too_many_arguments)]
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &Renderer,
    scene: &Scene,
    shading: Shading,
    view: Mat4,
    proj: Mat4,
    eye: Vec3,
) -> Vec<u8> {
    let settings = FrameSettings { shading, grid: false, ..Default::default() };
    r.set_uniforms(queue, scene, view, proj, eye, &settings);
    draw_settings(device, queue, r, scene, &settings)
}

/// Le rendu proprement dit, une fois les uniformes posés.
fn draw_settings(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    r: &Renderer,
    scene: &Scene,
    settings: &FrameSettings,
) -> Vec<u8> {

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cible"),
        size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view_tex = target.create_view(&wgpu::TextureViewDescriptor::default());

    // La copie depuis une texture veut des lignes alignées sur 256 octets.
    let row = (SIZE * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("relecture"),
        size: (row * SIZE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("offscreen") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scène"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view_tex,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: r.depth_view(),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        r.draw(&mut pass, scene, settings, &[None]);
    }
    // La même passe que dans l'application, au même moment: après que la passe
    // de scène est close et que sa profondeur est lisible.
    r.draw_occlusion(&mut encoder, &view_tex, settings);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .expect("attente du GPU");
    let padded = slice.get_mapped_range().expect("tampon non lisible").to_vec();

    // On enlève le remplissage de fin de ligne.
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        let start = (y * row) as usize;
        out.extend_from_slice(&padded[start..start + (SIZE * 4) as usize]);
    }
    out
}

/// Ce qu'une image coûte vraiment, à densité élevée.
///
/// Le GPU travaille derrière le CPU, donc mesurer autour de `submit` ne dit
/// rien. On attend la fin de chaque image avant de mesurer la suivante: ce
/// n'est pas ce que fait l'application, mais c'est le seul moyen d'attribuer
/// un temps à un réglage plutôt qu'à la file d'attente.
fn bench(device: &wgpu::Device, queue: &wgpu::Queue, level: u32) {
    const SIDE: u32 = 1024;
    const FRAMES: u32 = 24;

    let mut r = Renderer::new(device, queue, FORMAT, SIDE, SIDE, false, 1);
    let mesh = primitives::icosphere(level);
    let faces = mesh.face_count();
    let scene = Scene::with_object(Object::new("dense", mesh));
    r.sync(device, queue, &scene, None);

    let eye = Vec3::new(0.0, 0.0, 2.4);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(0.9, 1.0, 0.05, 100.0);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench"),
        size: wgpu::Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view_tex = target.create_view(&wgpu::TextureViewDescriptor::default());

    let time = |label: &str, settings: FrameSettings| {
        r.set_uniforms(queue, &scene, view, proj, eye, &settings);
        let mut total_ms = 0.0;
        // Une image pour chauffer, puis on mesure.
        for warm in 0..=FRAMES {
            let started = std::time::Instant::now();
            let mut e = device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench") });
            {
                let mut pass = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bench scene"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_tex,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: r.depth_view(),
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                r.draw(&mut pass, &scene, &settings, &[None]);
            }
            r.draw_occlusion(&mut e, &view_tex, &settings);
            queue.submit(Some(e.finish()));
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
            if warm > 0 {
                total_ms += started.elapsed().as_secs_f64() * 1000.0;
            }
        }
        let _ = label;
        total_ms / FRAMES as f64
    };

    let base = FrameSettings { shading: Shading::Clay, grid: false, occlusion: false,
                               backface_cull: false, ..Default::default() };
    let sans = time("", base);
    let cull = time("", FrameSettings { backface_cull: true, ..base });
    let ao = time("", FrameSettings { backface_cull: true, occlusion: true, ..base });
    println!(
        "  {faces:>9} faces   tout {sans:>6.2} ms   sans les arrières {cull:>6.2} ms            avec occlusion {ao:>6.2} ms",
    );
}
