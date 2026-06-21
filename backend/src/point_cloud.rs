use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Point3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub latitude: f32,
    pub longitude: f32,
    pub depth: f32,
    pub intensity: f32,
    pub reflectivity: f32,
    pub beam_index: u32,
    pub ping_number: u32,
    pub quality: u8,
    pub artifact_flag: u8,
    pub seep_hint: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointBatch {
    pub points: Vec<Point3D>,
    pub batch_id: u64,
    pub timestamp: f64,
    pub ping_start: u32,
    pub ping_end: u32,
    pub depth_min: f32,
    pub depth_max: f32,
}

impl PointBatch {
    pub fn new(points: Vec<Point3D>, batch_id: u64) -> Self {
        let mut depth_min = f32::INFINITY;
        let mut depth_max = f32::NEG_INFINITY;
        let mut ping_start = u32::MAX;
        let mut ping_end = u32::MIN;
        for p in &points {
            if p.depth < depth_min {
                depth_min = p.depth;
            }
            if p.depth > depth_max {
                depth_max = p.depth;
            }
            if p.ping_number < ping_start {
                ping_start = p.ping_number;
            }
            if p.ping_number > ping_end {
                ping_end = p.ping_number;
            }
        }
        Self {
            points,
            batch_id,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            ping_start,
            ping_end,
            depth_min: if depth_min == f32::INFINITY { 0.0 } else { depth_min },
            depth_max: if depth_max == f32::NEG_INFINITY { 0.0 } else { depth_max },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LutColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub struct BathymetryLut {
    stops: Vec<(f32, LutColor)>,
}

impl BathymetryLut {
    pub fn ocean_default() -> Self {
        Self {
            stops: vec![
                (0.0, LutColor { r: 255, g: 255, b: 220 }),
                (0.05, LutColor { r: 199, g: 233, b: 180 }),
                (0.15, LutColor { r: 127, g: 205, b: 187 }),
                (0.25, LutColor { r: 65, g: 182, b: 196 }),
                (0.35, LutColor { r: 44, g: 162, b: 196 }),
                (0.45, LutColor { r: 29, g: 140, b: 192 }),
                (0.55, LutColor { r: 34, g: 113, b: 179 }),
                (0.65, LutColor { r: 37, g: 87, b: 158 }),
                (0.75, LutColor { r: 38, g: 64, b: 132 }),
                (0.85, LutColor { r: 32, g: 44, b: 102 }),
                (0.95, LutColor { r: 24, g: 28, b: 70 }),
                (1.0, LutColor { r: 16, g: 14, b: 48 }),
            ],
        }
    }

    pub fn seep_highlight() -> Self {
        Self {
            stops: vec![
                (0.0, LutColor { r: 255, g: 60, b: 60 }),
                (0.3, LutColor { r: 255, g: 160, b: 40 }),
                (0.5, LutColor { r: 255, g: 230, b: 120 }),
                (0.7, LutColor { r: 140, g: 210, b: 180 }),
                (1.0, LutColor { r: 30, g: 60, b: 150 }),
            ],
        }
    }

    pub fn sample(&self, t: f32) -> LutColor {
        let t = t.clamp(0.0, 1.0);
        if self.stops.len() < 2 {
            return self.stops.first().map(|s| s.1.clone()).unwrap_or(LutColor { r: 0, g: 0, b: 0 });
        }
        for window in self.stops.windows(2) {
            let (t0, c0) = &window[0];
            let (t1, c1) = &window[1];
            if t <= *t1 {
                let span = t1 - t0;
                let alpha = if span.abs() < 1e-6 { 0.0 } else { (t - t0) / span };
                return LutColor {
                    r: (c0.r as f32 + (c1.r as f32 - c0.r as f32) * alpha).clamp(0.0, 255.0) as u8,
                    g: (c0.g as f32 + (c1.g as f32 - c0.g as f32) * alpha).clamp(0.0, 255.0) as u8,
                    b: (c0.b as f32 + (c1.b as f32 - c0.b as f32) * alpha).clamp(0.0, 255.0) as u8,
                };
            }
        }
        self.stops.last().map(|s| s.1.clone()).unwrap_or(LutColor { r: 0, g: 0, b: 0 })
    }
}

impl Default for BathymetryLut {
    fn default() -> Self {
        Self::ocean_default()
    }
}
