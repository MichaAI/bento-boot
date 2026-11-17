//! The orbits: every pending unit is a dot on the ring of how deep it
//! waits, the goal (the default target) is the fox in the middle. Dots
//! glide on springs when the tree reshapes, finished units fall into the
//! fox, failed ones stay red on their ring for a while.

use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, TAU};

use bento_boot_model::{Boot, FailKind, NodeState, Usec, orbit};
use tiny_skia::{
    FillRule, GradientStop, LineCap, LinearGradient, Paint, PathBuilder, Pixmap, Point, Shader, SpreadMode, Stroke,
    StrokeDash, Transform,
};

use crate::color::{Palette, Rgb};

/// Where the orbit square sits, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// Side of the square.
    pub size: f32,
    /// Radius of the first ring and of the outermost one.
    pub inner: f32,
    pub outer: f32,
    /// Physical pixels per logical pixel.
    pub scale: f32,
    /// Dither cell, physical pixels.
    pub dither: u32,
}

impl Geometry {
    fn center(&self) -> (f32, f32) {
        (self.size / 2.0, self.size / 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Waiting,
    Running,
    Failed { dependency: bool },
}

#[derive(Debug, Clone, Copy, Default)]
struct Spring {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

impl Spring {
    fn at(x: f32, y: f32) -> Self {
        Self { x, y, vx: 0.0, vy: 0.0 }
    }

    /// Damped spring towards `(tx, ty)`; `zeta` below 1 overshoots a little.
    fn step(&mut self, tx: f32, ty: f32, dt: f32, stiffness: f32, zeta: f32) {
        let damping = 2.0 * stiffness.sqrt() * zeta;
        // Small sub-steps keep it stable when a frame comes late.
        let n = (dt / 0.008).ceil().max(1.0);
        let h = dt / n;
        for _ in 0..n as u32 {
            self.vx += (-stiffness * (self.x - tx) - damping * self.vx) * h;
            self.vy += (-stiffness * (self.y - ty) - damping * self.vy) * h;
            self.x += self.vx * h;
            self.y += self.vy * h;
        }
    }
}

#[derive(Debug, Clone)]
struct Sprite {
    pos: Spring,
    alpha: f32,
    look: Look,
    target: bool,
    parent: Option<String>,
    on_path: bool,
    since: Usec,
    timeout: Usec,
    label: String,
    /// When it left the tree; failed sprites stay a while after that.
    left_at: Option<Usec>,
}

/// A finished unit on its way into the fox.
#[derive(Debug, Clone)]
struct Falling {
    pos: Spring,
    from: (f32, f32),
    alpha: f32,
}

/// A label for a dot, in window logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelOut {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub color: Rgb,
    pub opacity: f32,
    pub side: i32,
}

#[derive(Default)]
pub struct Scene {
    sprites: HashMap<String, Sprite>,
    falling: Vec<Falling>,
    rings: f32,
    seen_failures: usize,
}

const FAILED_STAYS: Usec = 8_000_000;
const MAX_LABELS: usize = 12;

fn short(name: &str) -> String {
    name.strip_suffix(".service").unwrap_or(name).to_owned()
}

impl Scene {
    /// Moves everything one frame towards the current tree.
    pub fn update(&mut self, boot: &Boot, now: Usec, dt: f32, g: &Geometry, bounce: f32) {
        let tree = boot.tree();
        let place = orbit::layout(&tree, -FRAC_PI_2);
        let rings = tree.max_depth().max(1);
        let r = self.rings.max(1.0);
        self.rings = r + (rings as f32 - r) * (1.0 - (-dt * 4.0).exp());
        let (cx, cy) = g.center();
        let zeta = 1.0 - bounce.clamp(0.0, 1.0) * 0.6;

        // The critical path: from the goal to the unit running longest.
        let mut on_path: HashSet<usize> = HashSet::new();
        let oldest = tree
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.state == NodeState::Running)
            .filter_map(|(i, n)| n.job_id.and_then(|id| boot.jobs.get(&id)).map(|j| (i, boot.running_since(j))))
            .min_by_key(|&(_, since)| since);
        if let Some((i, _)) = oldest {
            on_path.extend(tree.path_to(i));
        }

        let mut present: HashSet<&str> = HashSet::new();
        for (i, node) in tree.nodes.iter().enumerate().skip(1) {
            present.insert(&node.unit);
            let p = place[i];
            let r = orbit::ring_radius(p.ring, rings, g.inner, g.outer);
            let (tx, ty) = (cx + r * p.angle.cos(), cy + r * p.angle.sin());
            let parent = node.parent.map(|pi| tree.nodes[pi].unit.clone()).filter(|_| node.parent != Some(0));
            let job = node.job_id.and_then(|id| boot.jobs.get(&id));
            let unit = boot.units.get(&node.unit);
            let look = if node.state == NodeState::Running { Look::Running } else { Look::Waiting };
            let spawn_at = parent
                .as_ref()
                .and_then(|pu| self.sprites.get(pu))
                .map_or_else(|| (cx + g.inner * p.angle.cos(), cy + g.inner * p.angle.sin()), |s| (s.pos.x, s.pos.y));
            let s = self.sprites.entry(node.unit.clone()).or_insert_with(|| Sprite {
                pos: Spring::at(spawn_at.0, spawn_at.1),
                alpha: 0.0,
                look,
                target: false,
                parent: None,
                on_path: false,
                since: now,
                timeout: 0,
                label: String::new(),
                left_at: None,
            });
            s.pos.step(tx, ty, dt, 70.0, zeta);
            s.alpha = (s.alpha + dt * 3.0).min(1.0);
            s.look = look;
            s.target = node.unit.ends_with(".target");
            s.parent = parent;
            s.on_path = on_path.contains(&i);
            s.since = job.map_or(now, |j| boot.running_since(j));
            s.timeout = unit.and_then(|u| u.facts.as_ref()).map_or(0, |f| f.timeout);
            s.label = unit.map_or_else(|| short(&node.unit), |u| short(u.label(&node.unit)));
            s.left_at = None;
        }

        // New failures turn their sprite red where it stands.
        for f in &boot.failures[self.seen_failures.min(boot.failures.len())..] {
            if let Some(s) = self.sprites.get_mut(&f.unit) {
                s.look = Look::Failed { dependency: f.kind == FailKind::Dependency };
                s.left_at = Some(now);
            }
        }
        self.seen_failures = boot.failures.len();

        // Units that left the tree: finished ones fall into the fox,
        // failed ones stay put and fade out later.
        let gone: Vec<String> = self.sprites.keys().filter(|k| !present.contains(k.as_str())).cloned().collect();
        for name in gone {
            let s = self.sprites.get_mut(&name).expect("listed above");
            match s.look {
                Look::Failed { .. } => {
                    let left = *s.left_at.get_or_insert(now);
                    s.pos.step(s.pos.x, s.pos.y, dt, 70.0, zeta);
                    if now.saturating_sub(left) > FAILED_STAYS {
                        s.alpha -= dt * 1.5;
                    }
                    if s.alpha <= 0.0 {
                        self.sprites.remove(&name);
                    }
                }
                Look::Waiting | Look::Running => {
                    let s = self.sprites.remove(&name).expect("listed above");
                    if s.alpha > 0.05 {
                        self.falling.push(Falling { pos: s.pos, from: (s.pos.x, s.pos.y), alpha: s.alpha });
                    }
                }
            }
        }

        for f in &mut self.falling {
            f.pos.step(cx, cy, dt, 18.0, 1.0);
            let d = (f.pos.x - cx).hypot(f.pos.y - cy);
            if d < g.inner * 0.9 {
                f.alpha -= dt * 4.0;
            }
        }
        self.falling.retain(|f| f.alpha > 0.0);
    }

    /// Draws the orbits into `pix`, which is `g.size` square.
    pub fn paint(&self, pix: &mut Pixmap, pal: &Palette, now: Usec, g: &Geometry) {
        pix.fill(tiny_skia::Color::TRANSPARENT);
        let (cx, cy) = g.center();
        let t = now as f32 / 1e6;
        let s = g.scale;

        // A soft dithered glow behind the fox.
        dither_disc(pix, cx, cy, g.inner * 1.15, pal.accent, 0.35, g.dither);

        // Rings
        let rings = self.rings.round().max(1.0) as u32;
        for ring in 1..=rings {
            let r = orbit::ring_radius(ring, rings, g.inner, g.outer);
            let mut pb = PathBuilder::new();
            pb.push_circle(cx, cy, r);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(pal.rings.skia(0.9));
                paint.anti_alias = true;
                let stroke = Stroke {
                    width: 1.5 * s,
                    line_cap: LineCap::Round,
                    dash: StrokeDash::new(vec![1.0 * s, 9.0 * s], t * 6.0 * s),
                    ..Stroke::default()
                };
                pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }

        // Edges, the critical path last so it lies on top
        let mut path_edges = Vec::new();
        for sp in self.sprites.values() {
            let from = sp.parent.as_ref().and_then(|p| self.sprites.get(p)).map(|p| (p.pos.x, p.pos.y));
            let to = (sp.pos.x, sp.pos.y);
            let from = from.unwrap_or_else(|| {
                let a = (to.1 - cy).atan2(to.0 - cx);
                (cx + g.inner * 0.92 * a.cos(), cy + g.inner * 0.92 * a.sin())
            });
            if sp.on_path {
                path_edges.push((from, to, sp.alpha));
            } else {
                edge(pix, from, to, (cx, cy), pal.overlay.skia(0.35 * sp.alpha), None, 1.6 * s);
            }
        }
        for (from, to, a) in path_edges {
            let grad = LinearGradient::new(
                Point::from_xy(from.0, from.1),
                Point::from_xy(to.0, to.1),
                vec![GradientStop::new(0.0, pal.accent2.skia(a)), GradientStop::new(1.0, pal.accent.skia(a))],
                SpreadMode::Pad,
                Transform::identity(),
            );
            edge(pix, from, to, (cx, cy), pal.accent.skia(a), grad, 3.2 * s);
        }

        // Finished units falling in, with a short trail
        for f in &self.falling {
            let (dx, dy) = (f.pos.x - f.from.0, f.pos.y - f.from.1);
            let len = dx.hypot(dy).max(1.0);
            let tail = (f.pos.x - dx / len * 18.0 * s, f.pos.y - dy / len * 18.0 * s);
            line(pix, tail, (f.pos.x, f.pos.y), pal.green.skia(0.5 * f.alpha), 2.0 * s);
            disc(pix, f.pos.x, f.pos.y, 3.5 * s, pal.green.skia(f.alpha));
        }

        // Dots
        let mut order: Vec<&Sprite> = self.sprites.values().collect();
        order.sort_by_key(|sp| match sp.look {
            Look::Waiting => 0,
            Look::Running => 1,
            Look::Failed { .. } => 2,
        });
        for sp in order {
            let (x, y, a) = (sp.pos.x, sp.pos.y, sp.alpha);
            match sp.look {
                Look::Waiting => {
                    if sp.target {
                        diamond(pix, x, y, 7.0 * s, pal.surface.skia(a), Some((pal.subtext.skia(a), 2.0 * s)));
                    } else {
                        disc(pix, x, y, 5.5 * s, pal.base.skia(a));
                        ring(pix, x, y, 5.5 * s, pal.overlay.skia(a), 2.0 * s);
                    }
                }
                Look::Running => {
                    let pulse = 0.5 + 0.5 * (t * 3.0 + x * 0.01).sin();
                    dither_disc(pix, x, y, (20.0 + 6.0 * pulse) * s, pal.accent, 0.55 * a, g.dither);
                    if sp.target {
                        diamond(pix, x, y, 8.0 * s, pal.accent.skia(a), None);
                    } else {
                        disc(pix, x, y, 7.0 * s, pal.accent.skia(a));
                    }
                    let elapsed = now.saturating_sub(sp.since);
                    if sp.timeout > 0 {
                        let frac = (elapsed as f32 / sp.timeout as f32).min(1.0);
                        let c = if frac > 0.75 { pal.red } else { pal.accent2 };
                        arc(pix, x, y, 13.0 * s, -FRAC_PI_2, frac * TAU, c.skia(a), 3.0 * s);
                    } else {
                        let start = t * 4.0;
                        arc(pix, x, y, 13.0 * s, start, 1.4, pal.accent2.skia(a * 0.9), 3.0 * s);
                    }
                }
                Look::Failed { dependency } => {
                    let c = if dependency { pal.red.mix(pal.base, 0.55) } else { pal.red };
                    if !dependency {
                        dither_disc(pix, x, y, 22.0 * s, pal.red, 0.5 * a, g.dither);
                    }
                    disc(pix, x, y, 8.0 * s, c.skia(a));
                    let k = 3.5 * s;
                    line(pix, (x - k, y - k), (x + k, y + k), pal.base.skia(a), 2.4 * s);
                    line(pix, (x - k, y + k), (x + k, y - k), pal.base.skia(a), 2.4 * s);
                }
            }
        }
    }

    /// Labels for the dots worth naming: failures, then running units
    /// (longest first), then targets on the first ring.
    pub fn labels(&self, pal: &Palette, now: Usec, g: &Geometry, origin: (f32, f32)) -> Vec<LabelOut> {
        let (cx, _) = g.center();
        let mut picked: Vec<(&Sprite, u8)> = Vec::new();
        for sp in self.sprites.values() {
            let rank = match sp.look {
                Look::Failed { dependency: false } => 0,
                Look::Running => 1,
                Look::Waiting if sp.target && sp.parent.is_none() => 2,
                _ => continue,
            };
            picked.push((sp, rank));
        }
        picked.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.since.cmp(&b.0.since)).then(a.0.label.cmp(&b.0.label)));
        picked.truncate(MAX_LABELS);
        let mut out: Vec<LabelOut> = picked
            .into_iter()
            .map(|(sp, rank)| {
                let secs = now.saturating_sub(sp.since) as f32 / 1e6;
                let text =
                    if rank == 1 && secs >= 1.0 { format!("{} · {:.0} с", sp.label, secs) } else { sp.label.clone() };
                let color = match rank {
                    0 => pal.red,
                    1 => pal.text,
                    _ => pal.subtext,
                };
                LabelOut {
                    x: origin.0 + (sp.pos.x - g.size / 2.0) / g.scale,
                    y: origin.1 + (sp.pos.y - g.size / 2.0) / g.scale,
                    text,
                    color,
                    opacity: sp.alpha,
                    side: if sp.pos.x < cx { -1 } else { 1 },
                }
            })
            .collect();
        spread(&mut out, 20.0);
        out
    }
}

/// Moves labels on the same side apart vertically so no two are closer
/// than `gap`, keeping their order and centre of mass roughly in place.
fn spread(labels: &mut [LabelOut], gap: f32) {
    for side in [-1, 1] {
        let mut idx: Vec<usize> = (0..labels.len()).filter(|&i| labels[i].side == side).collect();
        idx.sort_by(|&a, &b| labels[a].y.total_cmp(&labels[b].y));
        for _ in 0..4 {
            for w in idx.windows(2) {
                let (a, b) = (w[0], w[1]);
                let overlap = gap - (labels[b].y - labels[a].y);
                if overlap > 0.0 {
                    labels[a].y -= overlap / 2.0;
                    labels[b].y += overlap / 2.0;
                }
            }
        }
    }
}

fn paint_of(c: tiny_skia::Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn disc(pix: &mut Pixmap, x: f32, y: f32, r: f32, c: tiny_skia::Color) {
    if let Some(path) = PathBuilder::from_circle(x, y, r) {
        pix.fill_path(&path, &paint_of(c), FillRule::Winding, Transform::identity(), None);
    }
}

fn ring(pix: &mut Pixmap, x: f32, y: f32, r: f32, c: tiny_skia::Color, w: f32) {
    if let Some(path) = PathBuilder::from_circle(x, y, r) {
        let stroke = Stroke { width: w, ..Stroke::default() };
        pix.stroke_path(&path, &paint_of(c), &stroke, Transform::identity(), None);
    }
}

fn diamond(pix: &mut Pixmap, x: f32, y: f32, r: f32, fill: tiny_skia::Color, outline: Option<(tiny_skia::Color, f32)>) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y - r);
    pb.line_to(x + r, y);
    pb.line_to(x, y + r);
    pb.line_to(x - r, y);
    pb.close();
    if let Some(path) = pb.finish() {
        pix.fill_path(&path, &paint_of(fill), FillRule::Winding, Transform::identity(), None);
        if let Some((c, w)) = outline {
            let stroke = Stroke { width: w, ..Stroke::default() };
            pix.stroke_path(&path, &paint_of(c), &stroke, Transform::identity(), None);
        }
    }
}

fn line(pix: &mut Pixmap, a: (f32, f32), b: (f32, f32), c: tiny_skia::Color, w: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(a.0, a.1);
    pb.line_to(b.0, b.1);
    if let Some(path) = pb.finish() {
        let stroke = Stroke { width: w, line_cap: LineCap::Round, ..Stroke::default() };
        pix.stroke_path(&path, &paint_of(c), &stroke, Transform::identity(), None);
    }
}

#[allow(clippy::too_many_arguments)]
fn arc(pix: &mut Pixmap, x: f32, y: f32, r: f32, start: f32, sweep: f32, c: tiny_skia::Color, w: f32) {
    if sweep <= 0.001 {
        return;
    }
    let steps = ((sweep / TAU) * 64.0).ceil().max(2.0) as u32;
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = start + sweep * i as f32 / steps as f32;
        let (px, py) = (x + r * a.cos(), y + r * a.sin());
        if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
    }
    if let Some(path) = pb.finish() {
        let stroke = Stroke { width: w, line_cap: LineCap::Round, ..Stroke::default() };
        pix.stroke_path(&path, &paint_of(c), &stroke, Transform::identity(), None);
    }
}

/// A spiral from `a` to `b` around `center`: the angle turns evenly while
/// the radius eases from one ring to the next, so links follow the orbits
/// and never cut across the middle.
fn edge(
    pix: &mut Pixmap,
    a: (f32, f32),
    b: (f32, f32),
    center: (f32, f32),
    c: tiny_skia::Color,
    shader: Option<Shader<'static>>,
    w: f32,
) {
    let ra = (a.0 - center.0).hypot(a.1 - center.1);
    let rb = (b.0 - center.0).hypot(b.1 - center.1);
    let aa = (a.1 - center.1).atan2(a.0 - center.0);
    let ab = (b.1 - center.1).atan2(b.0 - center.0);
    let mut turn = ab - aa;
    while turn > std::f32::consts::PI {
        turn -= TAU;
    }
    while turn < -std::f32::consts::PI {
        turn += TAU;
    }
    let steps = ((turn.abs() * ra.max(rb)) / 6.0 + (rb - ra).abs() / 12.0).ceil().clamp(2.0, 96.0) as u32;
    let mut pb = PathBuilder::new();
    pb.move_to(a.0, a.1);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        // Leave along the radius, then swing round: the radius moves early,
        // the angle late.
        let tr = 1.0 - (1.0 - t).powi(2);
        let ta = t * t * (3.0 - 2.0 * t);
        let r = ra + (rb - ra) * tr;
        let ang = aa + turn * ta;
        pb.line_to(center.0 + r * ang.cos(), center.1 + r * ang.sin());
    }
    if let Some(path) = pb.finish() {
        let mut paint = paint_of(c);
        if let Some(sh) = shader {
            paint.shader = sh;
        }
        let stroke = Stroke { width: w, line_cap: LineCap::Round, ..Stroke::default() };
        pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

const BAYER: [[f32; 4]; 4] =
    [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

/// A glow in hard pixels: ordered (Bayer 4×4) dithering of a soft falloff,
/// in cells of `cell` pixels, like the fox.
fn dither_disc(pix: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Rgb, strength: f32, cell: u32) {
    let cell = cell.max(1) as i32;
    let (w, h) = (pix.width() as i32, pix.height() as i32);
    let (x0, x1) = (((cx - radius) as i32 / cell).max(0), ((cx + radius) as i32 / cell + 1).min(w / cell));
    let (y0, y1) = (((cy - radius) as i32 / cell).max(0), ((cy + radius) as i32 / cell + 1).min(h / cell));
    let alpha = 0.85;
    let src = [f32::from(color.0) * alpha, f32::from(color.1) * alpha, f32::from(color.2) * alpha, 255.0 * alpha];
    let pixels = pix.pixels_mut();
    for gy in y0..y1 {
        for gx in x0..x1 {
            let (px, py) = ((gx * cell) as f32 + cell as f32 / 2.0, (gy * cell) as f32 + cell as f32 / 2.0);
            let d = (px - cx).hypot(py - cy) / radius;
            if d >= 1.0 {
                continue;
            }
            let v = strength * (1.0 - d) * (1.0 - d);
            let threshold = (BAYER[(gy & 3) as usize][(gx & 3) as usize] + 0.5) / 16.0;
            if v <= threshold {
                continue;
            }
            for y in gy * cell..((gy + 1) * cell).min(h) {
                for x in gx * cell..((gx + 1) * cell).min(w) {
                    let p = &mut pixels[(y * w + x) as usize];
                    let keep = 1.0 - alpha;
                    let out = [
                        src[0] + f32::from(p.red()) * keep,
                        src[1] + f32::from(p.green()) * keep,
                        src[2] + f32::from(p.blue()) * keep,
                        src[3] + f32::from(p.alpha()) * keep,
                    ];
                    let a = out[3].min(255.0) as u8;
                    if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(
                        (out[0] as u8).min(a),
                        (out[1] as u8).min(a),
                        (out[2] as u8).min(a),
                        a,
                    ) {
                        *p = c;
                    }
                }
            }
        }
    }
}
