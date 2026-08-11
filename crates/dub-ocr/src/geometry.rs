//! Геометрия для DBNet-постпроцесса: связные компоненты бинарной карты, минимальный
//! ограничивающий прямоугольник (rotating calipers) и unclip (расширение полигона на
//! unclip_ratio * area / perimeter — как pyclipper в PP-OCR DBPostProcess).

pub type Point = (f32, f32);

/// Ограничивающий прямоугольник детекции: 4 угла (по часовой от левого-верхнего) в пикселях.
#[derive(Clone, Copy, Debug)]
pub struct Quad {
    pub pts: [Point; 4],
}

impl Quad {
    /// Осевой bbox (x0,y0,x1,y1).
    pub fn aabb(&self) -> (f32, f32, f32, f32) {
        // Один проход по 4 точкам вместо четырёх отдельных fold.
        let (mut x0, mut y0, mut x1, mut y1) =
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in &self.pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0, y0, x1, y1)
    }
    /// Короткая сторона прямоугольника (для min_size-фильтра как в PP-OCR).
    pub fn min_side(&self) -> f32 {
        let mut m = f32::INFINITY;
        for i in 0..4 {
            let a = self.pts[i];
            let b = self.pts[(i + 1) % 4];
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
            m = m.min(d);
        }
        m
    }
}

/// Связные компоненты (4-связность) бинарной карты `bitmap` (row-major, w*h), значение != 0.
/// Возвращает списки индексов пикселей каждой компоненты.
pub fn connected_components(bitmap: &[u8], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut label = vec![0u32; w * h];
    let mut comps: Vec<Vec<usize>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if bitmap[start] == 0 || label[start] != 0 {
            continue;
        }
        let id = comps.len() as u32 + 1;
        let mut comp: Vec<usize> = Vec::new();
        stack.push(start);
        label[start] = id;
        while let Some(idx) = stack.pop() {
            comp.push(idx);
            let x = idx % w;
            let y = idx / w;
            let push = |nx: usize, ny: usize, stack: &mut Vec<usize>, label: &mut [u32]| {
                let ni = ny * w + nx;
                if bitmap[ni] != 0 && label[ni] == 0 {
                    label[ni] = id;
                    stack.push(ni);
                }
            };
            if x > 0 {
                push(x - 1, y, &mut stack, &mut label);
            }
            if x + 1 < w {
                push(x + 1, y, &mut stack, &mut label);
            }
            if y > 0 {
                push(x, y - 1, &mut stack, &mut label);
            }
            if y + 1 < h {
                push(x, y + 1, &mut stack, &mut label);
            }
        }
        comps.push(comp);
    }
    comps
}

/// Выпуклая оболочка (Эндрю monotone chain).
fn convex_hull(mut pts: Vec<Point>) -> Vec<Point> {
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Point, a: Point, b: Point| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Минимальный ограничивающий прямоугольник по выпуклой оболочке (rotating calipers).
/// Возвращает 4 угла. Пустой ввод -> нулевой Quad.
pub fn min_area_rect(points: &[Point]) -> Quad {
    let hull = convex_hull(points.to_vec());
    if hull.len() < 3 {
        // вырожденный случай — просто осевой bbox
        let (mut x0, mut y0, mut x1, mut y1) =
            (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &(x, y) in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        if !x0.is_finite() {
            return Quad { pts: [(0.0, 0.0); 4] };
        }
        return Quad { pts: [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] };
    }
    let n = hull.len();
    let mut best_area = f32::INFINITY;
    let mut best: Quad = Quad { pts: [(0.0, 0.0); 4] };
    for i in 0..n {
        let a = hull[i];
        let b = hull[(i + 1) % n];
        let (mut ex, mut ey) = (b.0 - a.0, b.1 - a.1);
        let len = (ex * ex + ey * ey).sqrt();
        if len < 1e-6 {
            continue;
        }
        ex /= len;
        ey /= len;
        // нормаль
        let (nx, ny) = (-ey, ex);
        let (mut min_u, mut max_u, mut min_v, mut max_v) =
            (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for &p in &hull {
            let u = (p.0 - a.0) * ex + (p.1 - a.1) * ey;
            let v = (p.0 - a.0) * nx + (p.1 - a.1) * ny;
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if area < best_area {
            best_area = area;
            let corner = |u: f32, v: f32| (a.0 + ex * u + nx * v, a.1 + ey * u + ny * v);
            best = Quad {
                pts: [
                    corner(min_u, min_v),
                    corner(max_u, min_v),
                    corner(max_u, max_v),
                    corner(min_u, max_v),
                ],
            };
        }
    }
    best
}

/// Средний score карты вероятностей внутри осевого bbox полигона (box_score_fast из PP-OCR).
pub fn box_score_fast(prob: &[f32], w: usize, h: usize, quad: &Quad) -> f32 {
    let (x0, y0, x1, y1) = quad.aabb();
    let xmin = x0.floor().max(0.0) as usize;
    let xmax = (x1.ceil() as isize).clamp(0, w as isize - 1) as usize;
    let ymin = y0.floor().max(0.0) as usize;
    let ymax = (y1.ceil() as isize).clamp(0, h as isize - 1) as usize;
    if xmax < xmin || ymax < ymin {
        return 0.0;
    }
    let mut sum = 0.0f32;
    for y in ymin..=ymax {
        for x in xmin..=xmax {
            sum += prob[y * w + x];
        }
    }
    // после guard (xmax>=xmin, ymax>=ymin) диапазон непуст -> cnt >= 1, деление безопасно.
    let cnt = (ymax - ymin + 1) * (xmax - xmin + 1);
    sum / cnt as f32
}

/// Unclip: расширить прямоугольник наружу на distance = area * ratio / perimeter (Vatti/pyclipper
/// в PP-OCR DBPostProcess). Каждое ребро сдвигается наружу на dist вдоль ВНЕШНЕЙ нормали, новые
/// углы — пересечение соседних сдвинутых рёбер. Для тонких широких боксов (сабы) это равномерно
/// наращивает и высоту, и ширину — радиальный сдвиг от центроида этого НЕ делает (почти не двигает
/// верх/низ) и обрезал глифы по высоте.
pub fn unclip(quad: &Quad, ratio: f32) -> Quad {
    let pts = &quad.pts;
    let mut area = 0.0f32;
    let mut peri = 0.0f32;
    for i in 0..4 {
        let a = pts[i];
        let b = pts[(i + 1) % 4];
        area += a.0 * b.1 - b.0 * a.1;
        peri += ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    }
    let signed_area = area / 2.0; // знак задаёт направление обхода
    area = area.abs() / 2.0;
    if peri < 1e-6 || area < 1e-6 {
        return *quad;
    }
    let dist = area * ratio / peri;
    // ориентация: для обхода против часовой (signed_area>0) внешняя нормаль ребра (dx,dy) = (dy,-dx)/|e|
    // после нормализации; для по часовой — противоположная. Приводим к «наружу».
    let sign = if signed_area >= 0.0 { 1.0 } else { -1.0 };

    // Смещённые рёбра как (точка на ребре, направление ребра). Ребро i идёт pts[i]->pts[i+1].
    struct Edge {
        p: Point,
        d: Point,
    }
    let mut edges: Vec<Edge> = Vec::with_capacity(4);
    for i in 0..4 {
        let a = pts[i];
        let b = pts[(i + 1) % 4];
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let l = (ex * ex + ey * ey).sqrt();
        if l < 1e-6 {
            edges.push(Edge { p: a, d: (1.0, 0.0) });
            continue;
        }
        let (ux, uy) = (ex / l, ey / l);
        // внешняя нормаль
        let (nx, ny) = (uy * sign, -ux * sign);
        let p = (a.0 + nx * dist, a.1 + ny * dist);
        edges.push(Edge { p, d: (ux, uy) });
    }

    // пересечение линий (p0 + t*d0) и (p1 + s*d1)
    let intersect = |e0: &Edge, e1: &Edge| -> Point {
        let (p0, d0) = (e0.p, e0.d);
        let (p1, d1) = (e1.p, e1.d);
        let denom = d0.0 * d1.1 - d0.1 * d1.0;
        if denom.abs() < 1e-6 {
            return p1; // почти параллельны — берём точку второго ребра
        }
        let t = ((p1.0 - p0.0) * d1.1 - (p1.1 - p0.1) * d1.0) / denom;
        (p0.0 + t * d0.0, p0.1 + t * d0.1)
    };

    // новый угол i = пересечение ребра (i-1) и ребра i
    let mut out = [(0.0, 0.0); 4];
    for i in 0..4 {
        let prev = (i + 3) % 4;
        out[i] = intersect(&edges[prev], &edges[i]);
    }
    Quad { pts: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ключевой фикс качества B: edge-normal unclip растит ТОНКИЙ ШИРОКИЙ бокс (саб) по высоте,
    /// а не только по ширине. Радиальный сдвиг от центроида почти не двигал верх/низ -> кроп резал
    /// глифы по высоте (11px вместо ~25) -> rec шумел. Проверяем прирост высоты edge-normal-offset'ом.
    #[test]
    fn unclip_grows_thin_wide_box_height() {
        // осевой прямоугольник 200x10 (тонкая широкая строка)
        let q = Quad { pts: [(0.0, 0.0), (200.0, 0.0), (200.0, 10.0), (0.0, 10.0)] };
        let e = unclip(&q, 1.6);
        let (x0, y0, x1, y1) = e.aabb();
        let gw = (x1 - x0) - 200.0;
        let gh = (y1 - y0) - 10.0;
        // dist = area*ratio/peri = 2000*1.6/420 ≈ 7.62; высота растёт на ~2*dist по обе стороны.
        assert!(gh > 12.0, "высота должна заметно вырасти, gh={gh}");
        assert!(gw > 12.0, "ширина тоже растёт, gw={gw}");
        // прирост высоты и ширины одного порядка (равномерный edge-offset), НЕ гипертрофия ширины.
        assert!((gh - gw).abs() < 2.0, "прирост равномерный: gw={gw} gh={gh}");
    }

    #[test]
    fn min_area_rect_axis_aligned() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 4.0), (0.0, 4.0), (5.0, 2.0)];
        let q = min_area_rect(&pts);
        let (x0, y0, x1, y1) = q.aabb();
        assert!((x1 - x0 - 10.0).abs() < 1e-3, "w=10");
        assert!((y1 - y0 - 4.0).abs() < 1e-3, "h=4");
    }
}
