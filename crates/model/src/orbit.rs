//! Radial layout: the goal in the centre, each level of waiting on its own
//! ring further out. A node's share of its parent's angle is proportional
//! to how many leaves hang under it, so busy branches get room and the
//! units actually holding the boot up end on the outermost rings.

use crate::Tree;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Radians, 0 pointing right, growing clockwise on screen (y down).
    pub angle: f32,
    /// Start and end of the slice this node owns.
    pub span: (f32, f32),
    pub ring: u32,
}

/// Where each node of `tree` goes, by index. `start` is the angle the
/// first slice begins at; `-π/2` puts the first branch straight up.
#[must_use]
pub fn layout(tree: &Tree, start: f32) -> Vec<Placement> {
    let n = tree.nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let mut leaves = vec![0u32; n];
    // Children always come after their parent, so one backward pass sums
    // the leaves bottom-up.
    for i in (0..n).rev() {
        let own = if tree.nodes[i].children.is_empty() { 1 } else { 0 };
        let sum: u32 = tree.nodes[i].children.iter().map(|&c| leaves[c]).sum();
        leaves[i] = own + sum;
    }

    let full = std::f32::consts::TAU;
    let mut out = vec![Placement { angle: start, span: (start, start + full), ring: 0 }; n];
    for i in 0..n {
        let (from, to) = out[i].span;
        let total = leaves[i].max(1) as f32;
        let mut at = from;
        for &c in &tree.nodes[i].children {
            let width = (to - from) * leaves[c] as f32 / total;
            out[c] = Placement { angle: at + width / 2.0, span: (at, at + width), ring: tree.nodes[c].depth };
            at += width;
        }
    }
    out
}

/// Radius of `ring` when `rings` rings fit between `inner` and `outer`.
/// Ring 0 is the centre.
#[must_use]
pub fn ring_radius(ring: u32, rings: u32, inner: f32, outer: f32) -> f32 {
    if ring == 0 {
        return 0.0;
    }
    if rings <= 1 {
        return inner;
    }
    inner + (outer - inner) * (ring - 1) as f32 / (rings - 1) as f32
}
