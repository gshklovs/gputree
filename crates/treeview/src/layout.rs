//! Squarified treemap layout.
//!
//! Adapted from disktree's `disktree-core/src/treemap.rs`
//! (https://github.com/tobi/disktree), Copyright (c) 2026 Tobi Lütke, MIT
//! License. The algorithm, the header bands and the merged tail are theirs;
//! here the tree is a generic [`VNode`] with two metrics instead of a scanned
//! directory, and tiles are addressed by stable string ids so they can be
//! animated across refreshes.
//!
//! Bruls, Huizing and van Wijk's squarified algorithm: grow a row of tiles
//! while the worst aspect ratio inside it keeps improving, then start a new
//! row in the remaining space.

use crate::model::VNode;

/// An axis-aligned rectangle in viewport pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    /// Shrink on every side, never past empty.
    #[must_use]
    pub fn inset(&self, padding: f32) -> Self {
        let w = padding.mul_add(-2.0, self.w).max(0.0);
        let h = padding.mul_add(-2.0, self.h).max(0.0);
        Self::new(self.x + padding, self.y + padding, w, h)
    }
    /// Linear interpolation, for animating a tile between two layouts.
    #[must_use]
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let l = |a: f32, b: f32| (b - a).mul_add(t, a);
        Self::new(l(self.x, to.x), l(self.y, to.y), l(self.w, to.w), l(self.h, to.h))
    }
}

/// One rectangle of the mosaic.
#[derive(Clone, Debug)]
pub struct Tile {
    /// The node's id, or `None` for a merged tail.
    pub id: Option<String>,
    /// The id of the node this tile belongs to (its parent for a tail).
    pub owner: String,
    /// Tail tiles: how many children were merged.
    pub others: usize,
    pub rect: Rect,
    /// Nesting level: 0 tiles are children of the current root.
    pub depth: u32,
    /// The band a subdivided node keeps for its own name.
    pub header: Option<Rect>,
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub max_depth: u32,
    pub padding: f32,
    pub padding_outer: f32,
    pub min_tile: f32,
    pub max_children: usize,
    pub header: f32,
    pub header_inner: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self { max_depth: 3, padding: 1.0, min_tile: 5.0, max_children: 64, padding_outer: 3.0, header: 20.0, header_inner: 15.0 }
    }
}

/// Lay out everything beneath `root` inside `area`, sized by `values[metric]`.
pub fn layout(root: &VNode, area: Rect, metric: usize, options: &LayoutOptions) -> Vec<Tile> {
    let mut tiles = Vec::new();
    place_children(root, area, metric, options, 0, &mut tiles);
    tiles
}

fn place_children(node: &VNode, area: Rect, metric: usize, options: &LayoutOptions, depth: u32, out: &mut Vec<Tile>) {
    if node.children.is_empty() || area.w <= 0.0 || area.h <= 0.0 {
        return;
    }
    let mut ranked: Vec<(usize, f64)> = node
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.values[metric]))
        .filter(|(_, v)| *v > 0.0 && v.is_finite())
        .collect();
    if ranked.is_empty() {
        return;
    }
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let kept = ranked.len().min(options.max_children);
    let mut values: Vec<f64> = ranked[..kept].iter().map(|(_, v)| *v).collect();
    let mut sources: Vec<Option<usize>> = ranked[..kept].iter().map(|(i, _)| Some(*i)).collect();
    let tail = ranked.len() - kept;
    if tail > 0 {
        values.push(ranked[kept..].iter().map(|(_, v)| *v).sum());
        sources.push(None);
    }
    for (slot, raw) in squarify(&values, area).iter().enumerate() {
        let rect = raw.inset(if depth == 0 { options.padding_outer } else { options.padding });
        if rect.w < options.min_tile || rect.h < options.min_tile {
            continue;
        }
        let Some(index) = sources[slot] else {
            out.push(Tile { id: None, owner: node.id.clone(), others: tail, rect, depth, header: None });
            continue;
        };
        let child = &node.children[index];
        let subdividable = !child.children.is_empty() && depth + 1 < options.max_depth;
        let header = subdividable.then(|| header_band(rect, options, depth)).flatten();
        out.push(Tile { id: Some(child.id.clone()), owner: child.id.clone(), others: 0, rect, depth, header });
        if let Some(header) = header {
            let body = Rect::new(rect.x, header.bottom(), rect.w, rect.bottom() - header.bottom());
            place_children(child, body, metric, options, depth + 1, out);
        }
    }
}

fn header_band(rect: Rect, options: &LayoutOptions, depth: u32) -> Option<Rect> {
    let height = if depth == 0 { options.header } else { options.header_inner };
    let body = rect.h - height;
    if rect.w < 44.0 || body < options.min_tile * 3.0 {
        return None;
    }
    Some(Rect::new(rect.x, rect.y, rect.w, height))
}

/// Split `area` into one rectangle per value, proportional to it; returned in
/// the order of `values`.
pub fn squarify(values: &[f64], area: Rect) -> Vec<Rect> {
    let mut rects = vec![Rect::default(); values.len()];
    let total: f64 = values.iter().filter(|v| **v > 0.0).sum();
    if total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return rects;
    }
    let mut order: Vec<usize> = (0..values.len()).filter(|i| values[*i] > 0.0).collect();
    order.sort_by(|l, r| values[*r].total_cmp(&values[*l]));
    let scale = f64::from(area.w) * f64::from(area.h) / total;
    let areas: Vec<f64> = order.iter().map(|i| values[*i] * scale).collect();

    let mut free = area;
    let mut start = 0;
    while start < areas.len() {
        let side = f64::from(free.w.min(free.h));
        let mut end = start + 1;
        let mut row_sum = areas[start];
        let mut row_worst = worst_ratio(&areas[start..end], row_sum, side);
        while end < areas.len() {
            let cand_sum = row_sum + areas[end];
            let cand_worst = worst_ratio(&areas[start..=end], cand_sum, side);
            if cand_worst > row_worst {
                break;
            }
            row_sum = cand_sum;
            row_worst = cand_worst;
            end += 1;
        }
        if free.w >= free.h {
            let strip_w = ((row_sum / f64::from(free.h)) as f32).min(free.w);
            let mut y = free.y;
            for index in start..end {
                let h = if strip_w > 0.0 { (areas[index] / f64::from(strip_w)) as f32 } else { 0.0 };
                let h = h.min(free.bottom() - y).max(0.0);
                rects[order[index]] = Rect::new(free.x, y, strip_w, h);
                y += h;
            }
            free.x += strip_w;
            free.w -= strip_w;
        } else {
            let strip_h = ((row_sum / f64::from(free.w)) as f32).min(free.h);
            let mut x = free.x;
            for index in start..end {
                let w = if strip_h > 0.0 { (areas[index] / f64::from(strip_h)) as f32 } else { 0.0 };
                let w = w.min(free.right() - x).max(0.0);
                rects[order[index]] = Rect::new(x, free.y, w, strip_h);
                x += w;
            }
            free.y += strip_h;
            free.h -= strip_h;
        }
        start = end;
    }
    rects
}

fn worst_ratio(areas: &[f64], row_sum: f64, side: f64) -> f64 {
    if row_sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let thickness = row_sum / side;
    areas.iter().fold(0.0_f64, |worst, area| {
        if *area <= 0.0 || thickness <= 0.0 {
            return worst;
        }
        let other = area / thickness;
        worst.max((thickness / other).max(other / thickness))
    })
}

/// The deepest tile containing a point (children follow their parent).
pub fn hit(tiles: &[Tile], x: f32, y: f32) -> Option<&Tile> {
    tiles.iter().rev().find(|t| t.rect.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, v: f64) -> VNode {
        VNode::new(id, id, "other", [v, v])
    }

    #[test]
    fn squarify_fills_the_area() {
        let area = Rect::new(0.0, 0.0, 800.0, 500.0);
        let rects = squarify(&[40.0, 30.0, 20.0, 5.0, 3.0, 2.0], area);
        let covered: f32 = rects.iter().map(|r| r.w * r.h).sum();
        assert!((covered - 400_000.0).abs() < 1.0);
    }

    #[test]
    fn layout_nests_and_hits_the_deepest() {
        let mut big = leaf("big", 150.0);
        big.children = vec![leaf("big/a", 100.0), leaf("big/b", 50.0)];
        let mut root = leaf("root", 160.0);
        root.children = vec![big, leaf("small", 10.0)];
        let tiles = layout(&root, Rect::new(0.0, 0.0, 800.0, 500.0), 0, &LayoutOptions::default());
        let ids: Vec<_> = tiles.iter().map(|t| t.id.clone().unwrap()).collect();
        assert_eq!(ids, vec!["big", "big/a", "big/b", "small"]);
        let a = &tiles[1];
        let found = hit(&tiles, a.rect.x + a.rect.w / 2.0, a.rect.y + a.rect.h / 2.0).unwrap();
        assert_eq!(found.id.as_deref(), Some("big/a"));
    }

    #[test]
    fn zero_values_get_no_tile() {
        let mut root = leaf("root", 1.0);
        root.children = vec![leaf("x", 0.0), leaf("y", 1.0)];
        let tiles = layout(&root, Rect::new(0.0, 0.0, 400.0, 300.0), 0, &LayoutOptions::default());
        assert_eq!(tiles.len(), 1);
    }
}
